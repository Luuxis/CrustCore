mod archive;
mod fabric;
mod forgelike;
mod mirrors;
mod patcher;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::foundation::events::{Event, EventHandler};
use crate::foundation::options::LoaderKind;
use crate::network::{self, HttpClient};
use crate::providers::launchermeta::{Argument, Library};
use crate::providers::{fabric as fabric_meta, forge as forge_meta, neoforge as neoforge_meta};

pub use forgelike::Flavor;

#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub kind: LoaderKind,
    pub minecraft_version: String,
    pub build: String,
    pub loader_dir: PathBuf,
    pub java_path: PathBuf,
    pub minecraft_jar: PathBuf,
    pub minecraft_json: PathBuf,
    pub concurrency: usize,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct LoaderJson {
    pub raw: serde_json::Value,
    pub id: Option<String>,
    pub main_class: Option<String>,
    pub inherits_from: Option<String>,
    pub minecraft_arguments: Option<String>,
    pub game_arguments: Vec<String>,
    pub jvm_arguments: Vec<String>,
    pub libraries: Vec<Library>,
    pub loader_dir: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LoaderJsonView {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "mainClass", default)]
    main_class: Option<String>,
    #[serde(rename = "inheritsFrom", default)]
    inherits_from: Option<String>,
    #[serde(rename = "minecraftArguments", default)]
    minecraft_arguments: Option<String>,
    #[serde(default)]
    arguments: Option<LoaderArguments>,
    #[serde(default)]
    libraries: Vec<Library>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LoaderArguments {
    #[serde(default)]
    game: Vec<Argument>,
    #[serde(default)]
    jvm: Vec<Argument>,
}

impl LoaderJson {
    pub fn from_value(raw: serde_json::Value, loader_dir: PathBuf) -> Result<Self, Error> {
        let view: LoaderJsonView = serde_json::from_value(raw.clone())?;
        let arguments = view.arguments.unwrap_or_default();
        Ok(Self {
            raw,
            id: view.id,
            main_class: view.main_class,
            inherits_from: view.inherits_from,
            minecraft_arguments: view.minecraft_arguments,
            game_arguments: plain_strings(&arguments.game),
            jvm_arguments: plain_strings(&arguments.jvm),
            libraries: view.libraries,
            loader_dir,
        })
    }
}

fn plain_strings(arguments: &[Argument]) -> Vec<String> {
    arguments
        .iter()
        .filter_map(|argument| match argument {
            Argument::Plain(value) => Some(value.clone()),
            Argument::Conditional { .. } => None,
        })
        .collect()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InstallSection {
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub processors: Vec<Processor>,
    #[serde(default)]
    pub data: HashMap<String, DataEntry>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(rename = "filePath", default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub json: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Processor {
    pub jar: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub sides: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataEntry {
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub server: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error(transparent)]
    Forge(#[from] forge_meta::Error),
    #[error(transparent)]
    NeoForge(#[from] neoforge_meta::Error),
    #[error(transparent)]
    Fabric(#[from] fabric_meta::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Invalid(String),
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read archive {path}: {source}")]
    Zip {
        path: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub async fn install(
    http: &HttpClient,
    config: &InstallConfig,
    events: &EventHandler,
) -> Result<LoaderJson, Error> {
    let json = match config.kind {
        LoaderKind::Forge => forgelike::install(http, config, Flavor::Forge, events).await?,
        LoaderKind::NeoForge => forgelike::install(http, config, Flavor::NeoForge, events).await?,
        LoaderKind::Fabric | LoaderKind::LegacyFabric | LoaderKind::Quilt => {
            fabric::install(http, config, events).await?
        }
    };
    Ok(json)
}

pub(crate) fn patch_libraries(json: &mut LoaderJson, platform: crate::foundation::os::Platform) {
    json.libraries = crate::resolver::patches::apply(&json.libraries, platform);
}

pub(crate) fn write_version_files(
    loader_dir: &Path,
    id: &str,
    json: &serde_json::Value,
    minecraft_jar: &Path,
) -> Result<(), Error> {
    let destination = loader_dir.join("versions").join(id);
    std::fs::create_dir_all(&destination).map_err(|source| io_error(&destination, source))?;
    let json_path = destination.join(format!("{id}.json"));
    let content = serde_json::to_string_pretty(json)?;
    std::fs::write(&json_path, content).map_err(|source| io_error(&json_path, source))?;
    let jar_path = destination.join(format!("{id}.jar"));
    std::fs::copy(minecraft_jar, &jar_path).map_err(|source| io_error(&jar_path, source))?;
    Ok(())
}

pub(crate) fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    std::fs::write(path, bytes).map_err(|source| io_error(path, source))
}

pub(crate) fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

pub(crate) fn emit(events: &EventHandler, event: Event) {
    events(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fabric_profile_json() {
        let raw: serde_json::Value = serde_json::from_str(
            r#"{
            "id": "fabric-loader-0.16.9-1.20.1",
            "inheritsFrom": "1.20.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "]},
            "libraries": [{"name": "org.ow2.asm:asm:9.7.1", "url": "https://maven.fabricmc.net/", "sha1": "x", "size": 1}]
        }"#,
        )
        .unwrap();
        let json = LoaderJson::from_value(raw, PathBuf::from("/root/loader")).unwrap();
        assert_eq!(json.id.as_deref(), Some("fabric-loader-0.16.9-1.20.1"));
        assert_eq!(
            json.jvm_arguments,
            vec!["-DFabricMcEmu= net.minecraft.client.main.Main "]
        );
        assert!(json.game_arguments.is_empty());
        assert_eq!(
            json.libraries[0].url.as_deref(),
            Some("https://maven.fabricmc.net/")
        );
    }

    #[test]
    fn parses_old_forge_version_info() {
        let raw: serde_json::Value = serde_json::from_str(
            r#"{
            "id": "1.7.10-Forge10.13.4.1614-1.7.10",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "minecraftArguments": "--username ${auth_player_name} --tweakClass cpw.mods.fml.common.launcher.FMLTweaker",
            "jar": "1.7.10",
            "libraries": [{"name": "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10", "url": "https://maven.minecraftforge.net/"}, {"name": "net.minecraft:launchwrapper:1.12"}]
        }"#,
        )
        .unwrap();
        let json = LoaderJson::from_value(raw, PathBuf::from("/root")).unwrap();
        assert!(json.minecraft_arguments.is_some());
        assert_eq!(json.libraries.len(), 2);
        assert!(json.libraries[1].downloads.is_none());
    }

    #[test]
    fn loader_jna_is_upgraded_on_macos() {
        let raw: serde_json::Value = serde_json::from_str(
            r#"{
            "id": "1.18.2-forge-40.3.12",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "arguments": {"game": [], "jvm": ["-DmergeModules=jna-5.12.1.jar,jna-platform-5.12.1.jar,java-objc-bridge-1.0.0.jar"]},
            "libraries": [
                {"name": "net.java.dev.jna:jna:5.12.1", "downloads": {"artifact": {"path": "net/java/dev/jna/jna/5.12.1/jna-5.12.1.jar", "sha1": "a", "size": 1, "url": "https://maven.minecraftforge.net/net/java/dev/jna/jna/5.12.1/jna-5.12.1.jar"}}},
                {"name": "net.java.dev.jna:jna-platform:5.12.1", "downloads": {"artifact": {"path": "net/java/dev/jna/jna-platform/5.12.1/jna-platform-5.12.1.jar", "sha1": "b", "size": 1, "url": "https://maven.minecraftforge.net/net/java/dev/jna/jna-platform/5.12.1/jna-platform-5.12.1.jar"}}},
                {"name": "ca.weblite:java-objc-bridge:1.0.0"}
            ]
        }"#,
        )
        .unwrap();
        let mut json = LoaderJson::from_value(raw.clone(), PathBuf::from("/root")).unwrap();
        patch_libraries(&mut json, crate::foundation::os::Platform::MacOs);
        let names: Vec<&str> = json.libraries.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "net.java.dev.jna:jna:5.13.0",
                "net.java.dev.jna:jna-platform:5.13.0",
                "ca.weblite:java-objc-bridge:1.0.0"
            ]
        );
        assert_eq!(json.raw, raw);

        let mut windows = LoaderJson::from_value(raw, PathBuf::from("/root")).unwrap();
        patch_libraries(&mut windows, crate::foundation::os::Platform::Windows);
        assert_eq!(windows.libraries[0].name, "net.java.dev.jna:jna:5.12.1");
    }
}
