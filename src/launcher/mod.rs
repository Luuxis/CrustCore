mod arguments;
pub mod java;
mod natives;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::process::{Child, Command};

use crate::authenticator::Account;
use crate::checker::{self, CheckOptions};
use crate::foundation::events::{Event, EventHandler, noop};
use crate::loader::{self, InstallConfig, LoaderJson};
use crate::network::{Downloader, HttpClient};
use crate::resolver::{self, GameFiles, LABEL_LOG_CONFIGS, ResolvedVersion, Resolver};

pub use crate::foundation::options::{
    JavaOptions, LaunchOptions, LoaderKind, LoaderOptions, MemoryOptions, ScreenOptions,
    simplify_path,
};
pub use arguments::{ArgumentsInput, LaunchPlan};
pub use natives::extract_natives;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] crate::network::Error),
    #[error(transparent)]
    Resolver(#[from] resolver::Error),
    #[error(transparent)]
    Loader(#[from] loader::Error),
    #[error("Loader {0} not found")]
    LoaderNotFound(String),
    #[error(
        "no java executable available: download the mojang runtime or set `java.path` in the launch options"
    )]
    NoJava,
    #[error(transparent)]
    Azul(#[from] crate::providers::azul::Error),
    #[error("{0}")]
    JavaUnavailable(String),
    #[error("Minecraft main class not found")]
    NoMainClass,
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read native archive {path}: {source}")]
    Zip {
        path: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub version: ResolvedVersion,
    pub loader: Option<LoaderJson>,
    pub java: PathBuf,
    pub natives_list: bool,
    pub files: GameFiles,
}

pub struct Launch {
    http: HttpClient,
    options: LaunchOptions,
    account: Account,
    events: EventHandler,
}

impl Launch {
    pub fn new(mut options: LaunchOptions, account: Account) -> Result<Self, Error> {
        // Paths may have been set directly on the struct (or canonicalized by
        // the caller): make sure nothing verbatim (`\\?\`) reaches the JVM.
        options.root = simplify_path(&options.root);
        if let Some(java) = &options.java.path {
            options.java.path = Some(simplify_path(java));
        }
        Ok(Self {
            http: HttpClient::new()?,
            options,
            account,
            events: noop(),
        })
    }

    pub fn with_events(mut self, events: EventHandler) -> Self {
        self.events = events;
        self
    }

    pub fn with_http(mut self, http: HttpClient) -> Self {
        self.http = http;
        self
    }

    pub fn options(&self) -> &LaunchOptions {
        &self.options
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    pub async fn run(&self) -> Result<Child, Error> {
        let prepared = self.prepare().await?;
        let plan = self.plan(&prepared)?;
        self.spawn(&plan)
    }

    pub async fn prepare(&self) -> Result<Prepared, Error> {
        let options = &self.options;
        if options.loader.enable && options.loader.kind.is_none() {
            return Err(Error::LoaderNotFound("null".to_owned()));
        }

        let resolver = Resolver::new(self.http.clone()).with_intel_mac(options.intel_enabled_mac);
        let version = resolver.resolve_version(&options.version).await?;
        let files = resolver.resolve_files(&version, options).await?;

        for content in &files.content {
            write_content(&content.path, &content.content).await?;
        }

        let check_options = CheckOptions {
            root: options.root.clone(),
            instance: options.instance.clone(),
            ignored: options.ignored.clone(),
        };
        let check_events = self.events.clone();
        let missing = checker::find_missing(
            files.downloads.clone(),
            &check_options,
            Arc::new(move |progress| {
                check_events(Event::Check {
                    checked: progress.checked,
                    total: progress.total,
                    element: "Checking files".to_owned(),
                });
            }),
        )
        .await?;

        if !missing.is_empty() {
            let events = self.events.clone();
            let tracker = Mutex::new(SpeedTracker::new());
            Downloader::new(self.http.clone(), options.concurrency())
                .download_all(
                    missing,
                    Arc::new(move |progress| {
                        events(Event::Progress {
                            downloaded: progress.downloaded,
                            total: progress.total,
                            element: progress.label.clone(),
                        });
                        if let Ok(mut tracker) = tracker.lock()
                            && let Some(speed) = tracker.sample(progress.downloaded)
                        {
                            events(Event::Speed(speed));
                            if speed > 0.0 {
                                let remaining = progress.total.saturating_sub(progress.downloaded);
                                events(Event::Estimated(remaining as f64 / speed));
                            }
                        }
                    }),
                )
                .await?;
        }

        let mut java = options
            .java
            .path
            .clone()
            .or_else(|| files.java_executable.clone());
        if java.is_none()
            && let Some(request) = &files.azul
        {
            let platform = crate::foundation::os::Platform::current();
            let arch = crate::foundation::os::effective_arch(
                platform,
                crate::foundation::os::Arch::current(),
                options.intel_enabled_mac,
            );
            let install = java::AzulInstall {
                http: &self.http,
                root: &options.root,
                platform,
                arch,
                concurrency: options.concurrency(),
            };
            java = Some(java::install_azul(&install, request, &self.events).await?);
        }
        let java = java.ok_or(Error::NoJava)?;

        let mut loader_json = None;
        if options.loader.enable {
            let kind = options
                .loader
                .kind
                .ok_or_else(|| Error::LoaderNotFound("null".to_owned()))?;
            let version_dir = options.root.join("versions").join(&version.id);
            let config = InstallConfig {
                kind,
                minecraft_version: version.id.clone(),
                build: options.loader_build(),
                loader_dir: options.loader_dir(),
                java_path: java.clone(),
                minecraft_jar: version_dir.join(format!("{}.jar", version.id)),
                minecraft_json: version_dir.join(format!("{}.json", version.id)),
                concurrency: options.concurrency(),
                timeout: options.timeout,
            };
            loader_json = Some(loader::install(&self.http, &config, &self.events).await?);
        }

        if options.verify {
            let mut keep: Vec<PathBuf> = files
                .downloads
                .iter()
                .filter(|item| item.label != LABEL_LOG_CONFIGS)
                .map(|item| item.path.clone())
                .collect();
            keep.extend(files.content.iter().map(|c| c.path.clone()));
            checker::clean_unexpected(&check_options, &keep)
                .map_err(|source| io_error(&options.root, source))?;
        }

        let natives = natives_dir(&options.root, &version.id);
        extract_natives(&files.natives, &natives).await?;
        let natives_list = !files.natives.is_empty();

        if resolver::is_old(&version.json) {
            copy_legacy_assets(&options.root, &version.json, options.game_dir()).await?;
        }

        Ok(Prepared {
            version,
            loader: loader_json,
            java,
            natives_list,
            files,
        })
    }

    pub fn plan(&self, prepared: &Prepared) -> Result<LaunchPlan, Error> {
        let input = ArgumentsInput {
            version: &prepared.version.json,
            loader: prepared.loader.as_ref(),
            account: &self.account,
            options: &self.options,
            natives_list: prepared.natives_list,
        };
        arguments::build(&input, prepared.java.clone())
    }

    pub fn spawn(&self, plan: &LaunchPlan) -> Result<Child, Error> {
        spawn_with(plan, self.options.detached)
    }

    pub fn secrets(&self) -> Vec<String> {
        let mut secrets = vec![
            self.account.access_token.clone(),
            self.account.client_token.clone(),
            self.account.uuid.clone(),
        ];
        if let Some(xuid) = self.account.xuid() {
            secrets.push(xuid.to_owned());
        }
        secrets.retain(|s| !s.is_empty());
        secrets
    }
}

struct SpeedTracker {
    started: Instant,
    last_at: Instant,
    last_bytes: u64,
    samples: Vec<f64>,
}

impl SpeedTracker {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_at: now,
            last_bytes: 0,
            samples: Vec::new(),
        }
    }

    fn sample(&mut self, downloaded: u64) -> Option<f64> {
        let elapsed = self.last_at.elapsed().as_secs_f64();
        if elapsed < 0.5 {
            return None;
        }
        let chunk = downloaded.saturating_sub(self.last_bytes) as f64;
        if self.samples.len() >= 5 {
            self.samples.remove(0);
        }
        self.samples.push(chunk / elapsed);
        self.last_at = Instant::now();
        self.last_bytes = downloaded;
        let _ = self.started;
        Some(self.samples.iter().sum::<f64>() / self.samples.len() as f64)
    }
}

pub fn natives_dir(root: &Path, version_id: &str) -> PathBuf {
    root.join("versions").join(version_id).join("natives")
}

pub fn spawn_with(plan: &LaunchPlan, detached: bool) -> Result<Child, Error> {
    std::fs::create_dir_all(&plan.working_dir)
        .map_err(|source| io_error(&plan.working_dir, source))?;
    let mut command = Command::new(&plan.java);
    command
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if detached {
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x0000_0008);
    }
    command
        .spawn()
        .map_err(|source| io_error(&plan.java, source))
}

async fn write_content(path: &Path, content: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| io_error(parent, source))?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|source| io_error(path, source))
}

async fn copy_legacy_assets(
    root: &Path,
    version: &crate::providers::launchermeta::VersionJson,
    game_dir: PathBuf,
) -> Result<(), Error> {
    let Some(assets) = &version.assets else {
        return Ok(());
    };
    let index_path = root
        .join("assets")
        .join("indexes")
        .join(format!("{assets}.json"));
    let Ok(raw) = tokio::fs::read_to_string(&index_path).await else {
        return Ok(());
    };
    let index: crate::providers::launchermeta::AssetIndex = match serde_json::from_str(&raw) {
        Ok(index) => index,
        Err(_) => return Ok(()),
    };
    let objects = root.join("assets").join("objects");
    let target_root = game_dir.join("resources");
    for (name, object) in index.objects {
        let target = target_root.join(&name);
        if tokio::fs::metadata(&target).await.is_ok() {
            continue;
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| io_error(parent, source))?;
        }
        let source = objects.join(&object.hash[..2]).join(&object.hash);
        tokio::fs::copy(&source, &target)
            .await
            .map_err(|source| io_error(&target, source))?;
    }
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
