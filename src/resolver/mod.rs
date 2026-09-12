mod lwjgl;
pub mod patches;

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::foundation::maven::MavenCoordinate;
use crate::foundation::options::LaunchOptions;
use crate::foundation::os::{self, Arch, Platform};
use crate::network::{self, DownloadItem, HttpClient};
use crate::providers::launchermeta::{self, LauncherMeta, VersionJson};

pub const LABEL_LIBRARIES: &str = "Libraries";
pub const LABEL_NATIVE: &str = "Native";
pub const LABEL_LOG_CONFIGS: &str = "Log_configs";
pub const LABEL_ASSETS: &str = "Assets";
pub const LABEL_JAVA: &str = "Java";

const DEFAULT_JAVA_COMPONENT: &str = "jre-legacy";

#[derive(Debug, Clone)]
pub struct Resolver {
    meta: LauncherMeta,
    http: HttpClient,
    platform: Platform,
    arch: Arch,
}

#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    pub id: String,
    pub json: VersionJson,
    pub raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct GameFiles {
    pub downloads: Vec<DownloadItem>,
    pub content: Vec<ContentFile>,
    pub natives: Vec<PathBuf>,
    pub java_executable: Option<PathBuf>,
    pub azul: Option<AzulRequest>,
    pub asset_index_id: Option<String>,
    pub legacy_assets: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzulRequest {
    pub major_version: String,
    pub package_type: String,
}

impl AzulRequest {
    pub fn new(major_version: impl Into<String>, package_type: Option<String>) -> Self {
        Self {
            major_version: major_version.into(),
            package_type: package_type.unwrap_or_else(|| "jre".to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFile {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CustomFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Meta(#[from] launchermeta::Error),
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("Minecraft {0} is not found.")]
    VersionNotFound(String),
    #[error("no mojang java runtime for this platform")]
    UnsupportedPlatform,
    #[error("java runtime component {component} is not available for {platform}")]
    JavaRuntimeUnavailable { component: String, platform: String },
    #[error("java executable not found in runtime manifest for {0}")]
    JavaExecutableMissing(String),
    #[error("version json has no client download")]
    MissingClient,
    #[error("custom files url {url} answered status {status}")]
    CustomFiles { url: String, status: u16 },
}

pub fn is_old(json: &VersionJson) -> bool {
    matches!(json.assets.as_deref(), Some("legacy") | Some("pre-1.6"))
}

impl Resolver {
    pub fn new(http: HttpClient) -> Self {
        Self {
            meta: LauncherMeta::new(http.clone()),
            http,
            platform: Platform::current(),
            arch: Arch::current(),
        }
    }

    pub fn with_target(mut self, platform: Platform, arch: Arch) -> Self {
        self.platform = platform;
        self.arch = arch;
        self
    }

    pub fn with_intel_mac(mut self, enabled: bool) -> Self {
        self.arch = os::effective_arch(self.platform, self.arch, enabled);
        self
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn arch(&self) -> Arch {
        self.arch
    }

    pub fn meta(&self) -> &LauncherMeta {
        &self.meta
    }

    pub async fn resolve_version(&self, requested: &str) -> Result<ResolvedVersion, Error> {
        let manifest = self.meta.version_manifest().await?;
        let id = manifest.resolve_id(requested).to_owned();
        let entry = manifest
            .find(&id)
            .ok_or_else(|| Error::VersionNotFound(id.clone()))?;
        let (mut json, mut raw) = self.meta.version_json(&entry.url).await?;
        if self.platform == Platform::Linux
            && matches!(self.arch, Arch::Arm | Arch::Arm64)
            && let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw)
            && lwjgl::process(&mut value, self.arch)
            && let Ok(patched) = serde_json::from_value::<VersionJson>(value.clone())
            && let Ok(text) = serde_json::to_string(&value)
        {
            json = patched;
            raw = text;
        }
        json.libraries = patches::apply(&json.libraries, self.platform);
        Ok(ResolvedVersion { id, json, raw })
    }

    pub async fn resolve_files(
        &self,
        version: &ResolvedVersion,
        options: &LaunchOptions,
    ) -> Result<GameFiles, Error> {
        let root = options.root.as_path();
        let mut files = GameFiles::default();
        self.resolve_libraries(&version.json, root, &mut files);
        self.resolve_client(version, root, &mut files)?;
        self.resolve_logging(&version.json, root, &mut files);
        if let Some(url) = &options.url {
            self.resolve_custom_files(url, options.instance.as_deref(), root, &mut files)
                .await?;
        }
        self.resolve_assets(&version.json, root, &mut files).await?;
        if options.java.path.is_none() {
            match &options.java.version {
                Some(forced) => {
                    files.azul = Some(AzulRequest::new(forced.clone(), options.java.kind.clone()));
                }
                None => match self.resolve_java(&version.json, root, &mut files).await {
                    Ok(()) => {}
                    Err(
                        Error::UnsupportedPlatform
                        | Error::JavaRuntimeUnavailable { .. }
                        | Error::JavaExecutableMissing(_),
                    ) => {
                        files.java_executable = None;
                        files.downloads.retain(|item| item.label != LABEL_JAVA);
                        let major = version
                            .json
                            .java_version
                            .as_ref()
                            .map(|j| j.major_version.to_string())
                            .unwrap_or_else(|| "8".to_owned());
                        files.azul = Some(AzulRequest::new(major, options.java.kind.clone()));
                    }
                    Err(error) => return Err(error),
                },
            }
        }
        Ok(files)
    }

    pub fn resolve_libraries(&self, json: &VersionJson, root: &Path, files: &mut GameFiles) {
        let libraries_dir = root.join("libraries");
        let os_name = self.platform.mojang_name();
        for library in &json.libraries {
            let downloads = library.downloads.as_ref();
            let (artifact, label) = if let Some(natives) = &library.natives {
                let Some(native) = natives.get(os_name) else {
                    continue;
                };
                let classifier = native.replace("${arch}", self.arch.bits());
                let artifact = downloads
                    .and_then(|d| d.classifiers.as_ref())
                    .and_then(|c| c.get(&classifier));
                (artifact, LABEL_NATIVE)
            } else {
                if let Some(rules) = &library.rules
                    && let Some(first) = rules.first()
                    && let Some(os) = &first.os
                    && let Some(name) = &os.name
                    && name != os_name
                {
                    continue;
                }
                (downloads.and_then(|d| d.artifact.as_ref()), LABEL_LIBRARIES)
            };
            let Some(artifact) = artifact else {
                continue;
            };
            let relative = match &artifact.path {
                Some(path) => path.clone(),
                None => match library.name.parse::<MavenCoordinate>() {
                    Ok(coordinate) => coordinate.path(),
                    Err(_) => continue,
                },
            };
            let path = libraries_dir.join(relative);
            if label == LABEL_NATIVE {
                files.natives.push(path.clone());
            }
            files.downloads.push(DownloadItem {
                url: artifact.url.clone(),
                path,
                sha1: Some(artifact.sha1.clone()),
                size: Some(artifact.size),
                executable: false,
                label: label.to_owned(),
            });
        }
    }

    fn resolve_client(
        &self,
        version: &ResolvedVersion,
        root: &Path,
        files: &mut GameFiles,
    ) -> Result<(), Error> {
        let client = version
            .json
            .downloads
            .get("client")
            .ok_or(Error::MissingClient)?;
        let version_dir = root.join("versions").join(&version.id);
        files.downloads.push(DownloadItem {
            url: client.url.clone(),
            path: version_dir.join(format!("{}.jar", version.id)),
            sha1: Some(client.sha1.clone()),
            size: Some(client.size),
            executable: false,
            label: LABEL_LIBRARIES.to_owned(),
        });
        files.content.push(ContentFile {
            path: version_dir.join(format!("{}.json", version.id)),
            content: version.raw.clone(),
        });
        Ok(())
    }

    fn resolve_logging(&self, json: &VersionJson, root: &Path, files: &mut GameFiles) {
        let Some(client) = json.logging.as_ref().and_then(|l| l.client.as_ref()) else {
            return;
        };
        files.downloads.push(DownloadItem {
            url: client.file.url.clone(),
            path: root
                .join("assets")
                .join("log_configs")
                .join(&client.file.id),
            sha1: Some(client.file.sha1.clone()),
            size: Some(client.file.size),
            executable: false,
            label: LABEL_LOG_CONFIGS.to_owned(),
        });
    }

    async fn resolve_custom_files(
        &self,
        url: &str,
        instance: Option<&str>,
        root: &Path,
        files: &mut GameFiles,
    ) -> Result<(), Error> {
        let response = self.http.get(url, None).await?;
        if !response.is_success() {
            return Err(Error::CustomFiles {
                url: url.to_owned(),
                status: response.status,
            });
        }
        let items: Vec<CustomFile> = response.json()?;
        for item in items {
            if item.path.is_empty() {
                continue;
            }
            let label = item.path.split('/').next().unwrap_or_default().to_owned();
            let relative = match instance {
                Some(instance) => format!("instances/{instance}/{}", item.path),
                None => item.path.clone(),
            };
            files.downloads.push(DownloadItem {
                url: item.url,
                path: root.join(relative),
                sha1: item.hash,
                size: item.size,
                executable: false,
                label,
            });
        }
        Ok(())
    }

    async fn resolve_assets(
        &self,
        json: &VersionJson,
        root: &Path,
        files: &mut GameFiles,
    ) -> Result<(), Error> {
        let Some(index_ref) = &json.asset_index else {
            return Ok(());
        };
        let (index, raw) = self.meta.asset_index(&index_ref.url).await?;
        let assets_dir = root.join("assets");
        files.content.push(ContentFile {
            path: assets_dir
                .join("indexes")
                .join(format!("{}.json", index_ref.id)),
            content: raw,
        });
        files.asset_index_id = Some(index_ref.id.clone());
        files.legacy_assets = is_old(json);
        let objects_dir = assets_dir.join("objects");
        for object in index.objects.values() {
            files.downloads.push(DownloadItem {
                url: object.url(),
                path: objects_dir.join(&object.hash[..2]).join(&object.hash),
                sha1: Some(object.hash.clone()),
                size: Some(object.size),
                executable: false,
                label: LABEL_ASSETS.to_owned(),
            });
        }
        Ok(())
    }

    async fn resolve_java(
        &self,
        json: &VersionJson,
        root: &Path,
        files: &mut GameFiles,
    ) -> Result<(), Error> {
        let component = json
            .java_version
            .as_ref()
            .map(|j| j.component.clone())
            .unwrap_or_else(|| DEFAULT_JAVA_COMPONENT.to_owned());
        let platform_key = os::java_runtime_platform(self.platform, self.arch)
            .ok_or(Error::UnsupportedPlatform)?;
        let index = self.meta.java_runtime_index().await?;
        let entry = index
            .get(platform_key)
            .and_then(|components| components.get(&component))
            .and_then(|entries| entries.first())
            .ok_or_else(|| Error::JavaRuntimeUnavailable {
                component: component.clone(),
                platform: platform_key.to_owned(),
            })?;
        let manifest = self.meta.java_runtime_manifest(&entry.manifest.url).await?;

        let executable_suffix = self.platform.java_executable();
        let executable_key = manifest
            .files
            .keys()
            .filter(|key| key.ends_with(executable_suffix))
            .min_by_key(|key| key.len())
            .ok_or_else(|| Error::JavaExecutableMissing(component.clone()))?
            .clone();
        let prefix = executable_key
            .strip_suffix(executable_suffix)
            .unwrap_or_default()
            .to_owned();

        let runtime_dir = root.join("runtime").join(&component);
        for (relative, file) in &manifest.files {
            if file.kind != "file" {
                continue;
            }
            let Some(downloads) = &file.downloads else {
                continue;
            };
            let relative = relative.strip_prefix(prefix.as_str()).unwrap_or(relative);
            files.downloads.push(DownloadItem {
                url: downloads.raw.url.clone(),
                path: runtime_dir.join(relative),
                sha1: Some(downloads.raw.sha1.clone()),
                size: Some(downloads.raw.size),
                executable: file.executable,
                label: LABEL_JAVA.to_owned(),
            });
        }
        files.java_executable = Some(runtime_dir.join(executable_suffix));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::launchermeta::VersionJson;

    fn resolver(platform: Platform, arch: Arch) -> Resolver {
        Resolver::new(HttpClient::new().unwrap()).with_target(platform, arch)
    }

    fn version(json: &str) -> VersionJson {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn filters_libraries_like_node() {
        let json = version(
            r#"{
            "id": "1.21.1", "mainClass": "m",
            "downloads": {"client": {"sha1": "c", "size": 1, "url": "cu"}},
            "libraries": [
                {"name": "org.lwjgl:lwjgl:3.3.3", "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar", "sha1": "l", "size": 4, "url": "lu"}}},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-macos-arm64", "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-macos-arm64.jar", "sha1": "n", "size": 5, "url": "nu"}}, "rules": [{"action": "allow", "os": {"name": "osx"}}]},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows", "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar", "sha1": "w", "size": 5, "url": "wu"}}, "rules": [{"action": "allow", "os": {"name": "windows"}}]},
                {"name": "com.mojang:x:1", "downloads": {"artifact": {"path": "com/mojang/x/1/x-1.jar", "sha1": "x", "size": 1, "url": "xu"}}, "rules": [{"action": "allow"}, {"action": "disallow", "os": {"name": "osx"}}]}
            ]}"#,
        );
        let mut files = GameFiles::default();
        resolver(Platform::MacOs, Arch::Arm64).resolve_libraries(
            &json,
            Path::new("/root"),
            &mut files,
        );
        let names: Vec<String> = files
            .downloads
            .iter()
            .map(|d| d.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "lwjgl-3.3.3.jar",
                "lwjgl-3.3.3-natives-macos-arm64.jar",
                "x-1.jar"
            ]
        );
        assert!(files.natives.is_empty());
    }

    #[test]
    fn resolves_legacy_natives_classifiers() {
        let json = version(
            r#"{
            "id": "1.12.2", "mainClass": "m",
            "downloads": {"client": {"sha1": "c", "size": 1, "url": "cu"}},
            "libraries": [
                {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
                 "natives": {"linux": "natives-linux", "windows": "natives-windows-${arch}", "osx": "natives-osx"},
                 "downloads": {"classifiers": {
                    "natives-osx": {"path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-osx.jar", "sha1": "s", "size": 1, "url": "u"},
                    "natives-windows-64": {"path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-windows-64.jar", "sha1": "s", "size": 1, "url": "u"}
                 }}}
            ]}"#,
        );
        let mut mac = GameFiles::default();
        resolver(Platform::MacOs, Arch::X64).resolve_libraries(&json, Path::new("/root"), &mut mac);
        assert_eq!(mac.natives.len(), 1);
        assert!(mac.natives[0].ends_with("lwjgl-platform-2.9.4-natives-osx.jar"));
        assert_eq!(mac.downloads[0].label, LABEL_NATIVE);

        let mut win = GameFiles::default();
        resolver(Platform::Windows, Arch::X64).resolve_libraries(
            &json,
            Path::new("/root"),
            &mut win,
        );
        assert!(win.natives[0].ends_with("lwjgl-platform-2.9.4-natives-windows-64.jar"));
    }

    #[test]
    fn detects_old_versions() {
        let legacy =
            version(r#"{"id": "1.5.2", "assets": "legacy", "downloads": {}, "libraries": []}"#);
        assert!(is_old(&legacy));
        let modern = version(r#"{"id": "1.21", "assets": "17", "downloads": {}, "libraries": []}"#);
        assert!(!is_old(&modern));
    }
}
