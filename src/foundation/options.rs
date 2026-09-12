use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoaderKind {
    Forge,
    NeoForge,
    Fabric,
    LegacyFabric,
    Quilt,
}

impl LoaderKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "forge" => Some(Self::Forge),
            "neoforge" => Some(Self::NeoForge),
            "fabric" => Some(Self::Fabric),
            "legacyfabric" => Some(Self::LegacyFabric),
            "quilt" => Some(Self::Quilt),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::Fabric => "fabric",
            Self::LegacyFabric => "legacyfabric",
            Self::Quilt => "quilt",
        }
    }
}

impl fmt::Display for LoaderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone)]
pub struct LoaderOptions {
    pub path: String,
    pub kind: Option<LoaderKind>,
    pub build: String,
    pub enable: bool,
}

impl Default for LoaderOptions {
    fn default() -> Self {
        Self {
            path: "./loader".to_owned(),
            kind: None,
            build: "latest".to_owned(),
            enable: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct JavaOptions {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScreenOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: bool,
}

#[derive(Debug, Clone)]
pub struct MemoryOptions {
    pub min: String,
    pub max: String,
}

impl Default for MemoryOptions {
    fn default() -> Self {
        Self {
            min: "1G".to_owned(),
            max: "2G".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub url: Option<String>,
    pub root: PathBuf,
    pub version: String,
    pub instance: Option<String>,
    pub detached: bool,
    pub intel_enabled_mac: bool,
    pub ignore_log4j: bool,
    pub download_concurrency: usize,
    pub bypass_offline: bool,
    pub loader: LoaderOptions,
    pub mcp: Option<String>,
    pub verify: bool,
    pub ignored: Vec<String>,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    pub java: JavaOptions,
    pub screen: ScreenOptions,
    pub memory: MemoryOptions,
    pub timeout: Duration,
}

impl LaunchOptions {
    pub fn new(root: impl Into<PathBuf>, version: impl Into<String>) -> Self {
        let root: PathBuf = root.into();
        let root = std::path::absolute(&root).unwrap_or(root);
        Self {
            url: None,
            root,
            version: version.into(),
            instance: None,
            detached: false,
            intel_enabled_mac: false,
            ignore_log4j: false,
            download_concurrency: 5,
            bypass_offline: false,
            loader: LoaderOptions::default(),
            mcp: None,
            verify: false,
            ignored: Vec::new(),
            jvm_args: Vec::new(),
            game_args: Vec::new(),
            java: JavaOptions::default(),
            screen: ScreenOptions::default(),
            memory: MemoryOptions::default(),
            timeout: Duration::from_secs(10),
        }
    }

    pub fn concurrency(&self) -> usize {
        self.download_concurrency.clamp(1, 30)
    }

    pub fn game_dir(&self) -> PathBuf {
        match &self.instance {
            Some(instance) => self.root.join("instances").join(instance),
            None => self.root.clone(),
        }
    }

    pub fn loader_dir(&self) -> PathBuf {
        join_normalized(&self.root, &self.loader.path)
    }

    pub fn loader_kind(&self) -> Option<LoaderKind> {
        self.loader.kind
    }

    pub fn loader_build(&self) -> String {
        self.loader.build.to_ascii_lowercase()
    }

    pub fn mcp_path(&self) -> Option<PathBuf> {
        let mcp = self.mcp.as_ref()?;
        Some(match &self.instance {
            Some(instance) => join_normalized(&self.root.join("instances").join(instance), mcp),
            None => join_normalized(&self.root, mcp),
        })
    }
}

pub fn join_normalized(base: &Path, relative: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loader_kinds() {
        assert_eq!(LoaderKind::parse("Forge"), Some(LoaderKind::Forge));
        assert_eq!(
            LoaderKind::parse("legacyfabric"),
            Some(LoaderKind::LegacyFabric)
        );
        assert_eq!(LoaderKind::parse("nope"), None);
    }

    #[test]
    fn joins_like_node_path_join() {
        let base = Path::new("/root");
        assert_eq!(
            join_normalized(base, "./loader"),
            PathBuf::from("/root/loader")
        );
        assert_eq!(join_normalized(base, "./"), PathBuf::from("/root"));
        assert_eq!(
            join_normalized(base, "loader/forge"),
            PathBuf::from("/root/loader/forge")
        );
        assert_eq!(join_normalized(base, "/abs"), PathBuf::from("/root/abs"));
    }

    #[test]
    fn directories_follow_instance_and_loader_options() {
        let mut options = LaunchOptions::new("/root", "1.20.1");
        assert_eq!(options.game_dir(), PathBuf::from("/root"));
        assert_eq!(options.loader_dir(), PathBuf::from("/root/loader"));
        options.instance = Some("hypixel".into());
        options.loader.path = "./".into();
        options.mcp = Some("mcp.jar".into());
        assert_eq!(options.game_dir(), PathBuf::from("/root/instances/hypixel"));
        assert_eq!(options.loader_dir(), PathBuf::from("/root"));
        assert_eq!(
            options.mcp_path(),
            Some(PathBuf::from("/root/instances/hypixel/mcp.jar"))
        );
        assert_eq!(options.concurrency(), 5);
    }
}
