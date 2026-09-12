use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::foundation::rules::Rule;
use crate::network::{self, HttpClient, Response};

pub const VERSION_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
pub const JAVA_RUNTIME_INDEX_URL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
pub const ASSETS_BASE_URL: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone)]
pub struct LauncherMeta {
    http: HttpClient,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

impl VersionManifest {
    pub fn resolve_id<'a>(&'a self, requested: &'a str) -> &'a str {
        match requested {
            "latest_release" | "release" | "latest" => &self.latest.release,
            "latest_snapshot" | "snapshot" => &self.latest.snapshot,
            other => other,
        }
    }

    pub fn find(&self, id: &str) -> Option<&VersionEntry> {
        self.versions.iter().find(|v| v.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub main_class: String,
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub downloads: HashMap<String, Artifact>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub logging: Option<Logging>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
    #[serde(rename = "default-user-jvm", default)]
    pub default_user_jvm: Vec<DefaultUserJvm>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultUserJvm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<Rule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ArgumentValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    One(String),
    Many(Vec<String>),
}

impl ArgumentValue {
    pub fn values(&self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value.clone()],
            Self::Many(values) => values.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
    #[serde(default)]
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Logging {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
    #[serde(rename = "virtual", default)]
    pub is_virtual: bool,
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

impl AssetObject {
    pub fn relative_path(&self) -> String {
        format!("{}/{}", &self.hash[..2], self.hash)
    }

    pub fn url(&self) -> String {
        format!("{ASSETS_BASE_URL}/{}", self.relative_path())
    }
}

pub type JavaRuntimeIndex = HashMap<String, HashMap<String, Vec<JavaRuntimeEntry>>>;

#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeEntry {
    pub manifest: JavaRuntimeManifestRef,
    pub version: JavaRuntimeVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeManifestRef {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeVersion {
    pub name: String,
    #[serde(default)]
    pub released: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeManifest {
    pub files: HashMap<String, JavaRuntimeFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeFile {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub executable: bool,
    #[serde(default)]
    pub downloads: Option<JavaRuntimeDownloads>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaRuntimeDownloads {
    pub raw: Artifact,
    #[serde(default)]
    pub lzma: Option<Artifact>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("unexpected response from {url} (status {status})")]
    Unexpected { url: String, status: u16 },
}

impl LauncherMeta {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub async fn version_manifest(&self) -> Result<VersionManifest, Error> {
        self.get_json(VERSION_MANIFEST_URL).await.map(|(v, _)| v)
    }

    pub async fn version_json(&self, url: &str) -> Result<(VersionJson, String), Error> {
        self.get_json(url).await
    }

    pub async fn asset_index(&self, url: &str) -> Result<(AssetIndex, String), Error> {
        self.get_json(url).await
    }

    pub async fn java_runtime_index(&self) -> Result<JavaRuntimeIndex, Error> {
        self.get_json(JAVA_RUNTIME_INDEX_URL).await.map(|(v, _)| v)
    }

    pub async fn java_runtime_manifest(&self, url: &str) -> Result<JavaRuntimeManifest, Error> {
        self.get_json(url).await.map(|(v, _)| v)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<(T, String), Error> {
        let response: Response = self.http.get(url, None).await?;
        if !response.is_success() {
            return Err(Error::Unexpected {
                url: url.to_owned(),
                status: response.status,
            });
        }
        let parsed = response.json()?;
        Ok((parsed, response.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_version_json() {
        let json = r#"{
            "id": "1.21.1",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "arguments": {
                "game": ["--username", "${auth_player_name}", {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"}],
                "jvm": [{"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]}, "-cp", "${classpath}"]
            },
            "assetIndex": {"id": "17", "sha1": "a", "size": 1, "url": "u", "totalSize": 2},
            "assets": "17",
            "downloads": {"client": {"sha1": "c", "size": 3, "url": "cu"}},
            "libraries": [
                {"name": "org.lwjgl:lwjgl:3.3.3", "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar", "sha1": "l", "size": 4, "url": "lu"}}},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-macos-arm64", "downloads": {"artifact": {"path": "p", "sha1": "n", "size": 5, "url": "nu"}}, "rules": [{"action": "allow", "os": {"name": "osx"}}]}
            ],
            "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
            "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "file": {"id": "client-1.12.xml", "sha1": "x", "size": 6, "url": "xu"}, "type": "log4j2-xml"}}
        }"#;
        let version: VersionJson = serde_json::from_str(json).unwrap();
        assert_eq!(version.main_class, "net.minecraft.client.main.Main");
        let args = version.arguments.unwrap();
        assert_eq!(args.game.len(), 3);
        assert!(matches!(&args.jvm[0], Argument::Conditional { .. }));
        assert_eq!(version.libraries[1].rules.as_ref().unwrap().len(), 1);
        assert_eq!(
            version.java_version.unwrap().component,
            "java-runtime-delta"
        );
        assert!(version.downloads.contains_key("client"));
    }

    #[test]
    fn parses_legacy_version_json() {
        let json = r#"{
            "id": "1.12.2",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "minecraftArguments": "--username ${auth_player_name} --version ${version_name}",
            "assets": "1.12",
            "downloads": {"client": {"sha1": "c", "size": 3, "url": "cu"}},
            "libraries": [
                {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
                 "natives": {"linux": "natives-linux", "windows": "natives-windows", "osx": "natives-osx"},
                 "downloads": {"classifiers": {"natives-osx": {"path": "p", "sha1": "s", "size": 1, "url": "u"}}},
                 "extract": {"exclude": ["META-INF/"]}}
            ]
        }"#;
        let version: VersionJson = serde_json::from_str(json).unwrap();
        assert!(version.arguments.is_none());
        assert!(version.minecraft_arguments.is_some());
        let lib = &version.libraries[0];
        assert_eq!(lib.natives.as_ref().unwrap()["osx"], "natives-osx");
        assert_eq!(lib.extract.as_ref().unwrap().exclude, vec!["META-INF/"]);
    }

    #[test]
    fn resolves_manifest_aliases() {
        let manifest = VersionManifest {
            latest: LatestVersions {
                release: "1.21.1".into(),
                snapshot: "24w33a".into(),
            },
            versions: vec![],
        };
        assert_eq!(manifest.resolve_id("latest_release"), "1.21.1");
        assert_eq!(manifest.resolve_id("latest_snapshot"), "24w33a");
        assert_eq!(manifest.resolve_id("1.20.1"), "1.20.1");
    }

    #[test]
    fn asset_object_paths() {
        let object = AssetObject {
            hash: "abcdef0123".into(),
            size: 1,
        };
        assert_eq!(object.relative_path(), "ab/abcdef0123");
        assert_eq!(
            object.url(),
            "https://resources.download.minecraft.net/ab/abcdef0123"
        );
    }
}
