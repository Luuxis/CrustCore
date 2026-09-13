use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crust_core::authenticator::{Account, Authenticator};
use crust_core::foundation::events::Event;
use crust_core::launcher::{Launch, LaunchOptions, LaunchPlan, LoaderKind};
use crust_core::network::HttpClient;
use crust_core::providers::launchermeta::LauncherMeta;

const CLIENT_ID: Option<&str> = None;
const DATA_DIR: &str = "./data";
const ACCOUNT_FILE: &str = "account.json";
const MINECRAFT_PATH: &str = "minecraft";
const INSTANCE_NAME: &str = "try_all";
const REPORT_FILE: &str = "try_all_report.md";
const DOWNLOAD_SIMULTANEOUS: usize = 30;
const MEMORY_CONFIG: (&str, &str) = ("2G", "4G");
const INTEL_ENABLED_MAC: bool = true;
const VERSION_TYPES: &str = "release";
const RUN_SECS: u64 = 20;
const START_TIMEOUT_SECS: u64 = 240;
const MAX_ERROR_LINES: usize = 60;

const START_MARKERS: &[&str] = &[
    "LWJGL Version:",
    "Backend library: LWJGL",
    "OpenAL initialized",
    "Sound engine started",
    "Starting up SoundSystem",
    "Reloading ResourceManager",
    "Created: ",
];
const ALIVE_MARKERS: &[&str] = &["Setting user:", "Setting up user"];
const KNOWN_NOISE: &[&str] = &[
    "s3.amazonaws.com/MinecraftResources",
    "Realms: Server not available",
    "Cocoa: Failed to find service port for display",
    "GLFW error during init: [0x10008]",
    "########## GL ERROR ##########",
    "/ERROR]: @ ",
];
const ERROR_MARKERS: &[&str] = &[
    "/ERROR]",
    "/FATAL]",
    "[SEVERE]",
    "Exception",
    "Caused by:",
    "Error:",
    "error occurred",
    "hs_err_pid",
    "Could not find or load main class",
    "UnsatisfiedLinkError",
];

struct Settings {
    types: Vec<String>,
    only: Vec<String>,
    from: Option<String>,
    loader: Option<LoaderKind>,
    loader_build: String,
    oldest_first: bool,
    run_secs: u64,
    start_timeout_secs: u64,
    intel_mac: bool,
    jvm_args: Vec<String>,
    report: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Started,
    Alive,
    Closed,
    Crashed(String),
    Exited(String),
    NoStart,
    Failed(String),
}

impl Status {
    fn label(&self) -> String {
        match self {
            Self::Started => "ok".to_owned(),
            Self::Alive => "alive (no render marker)".to_owned(),
            Self::Closed => "closed after start (exit 0)".to_owned(),
            Self::Crashed(status) => format!("crashed ({status})"),
            Self::Exited(status) => format!("exited before start ({status})"),
            Self::NoStart => "timeout without start marker".to_owned(),
            Self::Failed(message) => format!("launch failed: {message}"),
        }
    }

    fn is_ok(&self) -> bool {
        matches!(self, Self::Started | Self::Alive | Self::Closed)
    }
}

struct Outcome {
    id: String,
    kind: String,
    status: Status,
    duration: Duration,
    errors: Vec<String>,
    ignored: usize,
}

struct Observation {
    status: Status,
    errors: Vec<String>,
    ignored: usize,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn env_list(name: &str, default: &str) -> Vec<String> {
    env_or(name, default)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn data_dir(subdir: Option<&str>) -> PathBuf {
    let mut dir = PathBuf::from(env_or("CRUSTCORE_DATA_DIR", DATA_DIR));
    if let Some(subdir) = subdir {
        dir.push(subdir);
    }
    fs::create_dir_all(&dir).expect("failed to create the data directory");
    dir.canonicalize().unwrap_or(dir)
}

fn account_path() -> PathBuf {
    data_dir(None).join(ACCOUNT_FILE)
}

fn settings() -> Settings {
    Settings {
        types: env_list("CRUSTCORE_TRY_TYPES", VERSION_TYPES),
        only: env_list("CRUSTCORE_TRY_VERSIONS", ""),
        from: std::env::var("CRUSTCORE_TRY_FROM")
            .ok()
            .filter(|v| !v.is_empty()),
        loader: LoaderKind::parse(&env_or("CRUSTCORE_TRY_LOADER", "none")),
        loader_build: env_or("CRUSTCORE_TRY_LOADER_BUILD", "latest"),
        oldest_first: env_or("CRUSTCORE_TRY_ORDER", "newest") == "oldest",
        run_secs: env_or("CRUSTCORE_TRY_RUN_SECS", &RUN_SECS.to_string())
            .parse()
            .unwrap_or(RUN_SECS),
        start_timeout_secs: env_or(
            "CRUSTCORE_TRY_START_TIMEOUT_SECS",
            &START_TIMEOUT_SECS.to_string(),
        )
        .parse()
        .unwrap_or(START_TIMEOUT_SECS),
        intel_mac: env_or(
            "CRUSTCORE_INTEL_MAC",
            if INTEL_ENABLED_MAC { "true" } else { "false" },
        ) == "true",
        jvm_args: env_or("CRUSTCORE_JVM_ARGS", "")
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        report: data_dir(None).join(env_or("CRUSTCORE_TRY_REPORT", REPORT_FILE)),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Fatal error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<bool, Box<dyn Error>> {
    let settings = settings();

    println!("Signing in...");
    let auth = match CLIENT_ID {
        Some(client_id) => Authenticator::with_client_id(client_id)?,
        None => Authenticator::new()?,
    };
    let account = load_or_authenticate(&auth).await?;
    println!("Signed in as {} ({})", account.name, account.uuid);

    println!("Fetching the version manifest...");
    let meta = LauncherMeta::new(HttpClient::new()?);
    let manifest = meta.version_manifest().await?;
    let versions = select_versions(&manifest.versions, &settings);
    if versions.is_empty() {
        return Err("no version matches the selection".into());
    }
    println!(
        "{} version(s) to try with loader {}, {} s after start, {} s start timeout, report in {}",
        versions.len(),
        settings
            .loader
            .map(|kind| format!("{kind} {}", settings.loader_build))
            .unwrap_or_else(|| "none".to_owned()),
        settings.run_secs,
        settings.start_timeout_secs,
        settings.report.display()
    );

    let mut outcomes: Vec<Outcome> = Vec::new();
    for (index, (id, kind)) in versions.iter().enumerate() {
        println!();
        println!(
            "===== [{}/{}] {id} ({kind}) =====",
            index + 1,
            versions.len()
        );
        let started = Instant::now();
        let observation = try_version(&account, id, &settings).await;
        let outcome = Outcome {
            id: id.clone(),
            kind: kind.clone(),
            status: observation.status,
            duration: started.elapsed(),
            errors: observation.errors,
            ignored: observation.ignored,
        };
        println!(
            "----- {id}: {} in {:.1} s, {} error line(s), {} known",
            outcome.status.label(),
            outcome.duration.as_secs_f64(),
            outcome.errors.len(),
            outcome.ignored
        );
        outcomes.push(outcome);
        write_report(&outcomes, &settings)?;
    }

    println!();
    print_summary(&outcomes);
    Ok(outcomes
        .iter()
        .all(|o| o.status.is_ok() && o.errors.is_empty()))
}

fn select_versions(
    entries: &[crust_core::providers::launchermeta::VersionEntry],
    settings: &Settings,
) -> Vec<(String, String)> {
    let mut selected: Vec<(String, String)> = entries
        .iter()
        .filter(|entry| {
            if !settings.only.is_empty() {
                return settings.only.iter().any(|id| id == &entry.id);
            }
            settings
                .types
                .iter()
                .any(|t| t == "all" || t == &entry.kind)
        })
        .map(|entry| (entry.id.clone(), entry.kind.clone()))
        .collect();
    if settings.oldest_first {
        selected.reverse();
    }
    if let Some(from) = &settings.from
        && let Some(position) = selected.iter().position(|(id, _)| id == from)
    {
        selected.drain(..position);
    }
    selected
}

async fn try_version(account: &Account, id: &str, settings: &Settings) -> Observation {
    let mut options = LaunchOptions::new(data_dir(Some(MINECRAFT_PATH)), id);
    options.instance = Some(INSTANCE_NAME.to_owned());
    options.intel_enabled_mac = settings.intel_mac;
    options.ignore_log4j = true;
    options.download_concurrency = DOWNLOAD_SIMULTANEOUS;
    options.jvm_args = settings.jvm_args.clone();
    options.memory.min = MEMORY_CONFIG.0.to_owned();
    options.memory.max = MEMORY_CONFIG.1.to_owned();
    if let Some(kind) = settings.loader {
        options.loader.kind = Some(kind);
        options.loader.build = settings.loader_build.clone();
        options.loader.enable = true;
        options.loader.path = "./".to_owned();
    }

    let launch = match Launch::new(options, account.clone()) {
        Ok(launch) => launch.with_events(event_printer()),
        Err(error) => return failed(error),
    };
    let prepared = match launch.prepare().await {
        Ok(prepared) => prepared,
        Err(error) => return failed(error),
    };
    let plan = match launch.plan(&prepared) {
        Ok(plan) => plan,
        Err(error) => return failed(error),
    };
    let secrets = launch.secrets();
    let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
    println!(
        "[LAUNCH] {}",
        plan.redacted_command(&secrets)
            .replace(&format!("{}/", launch.options().root.display()), "")
    );

    let mut child = match spawn_piped(&plan) {
        Ok(child) => child,
        Err(error) => return failed(error),
    };
    watch(&mut child, &secrets, settings).await
}

fn failed(error: impl std::fmt::Display) -> Observation {
    let message = error.to_string();
    eprintln!("[ERROR] {message}");
    Observation {
        status: Status::Failed(message.clone()),
        errors: vec![message],
        ignored: 0,
    }
}

fn spawn_piped(plan: &LaunchPlan) -> Result<Child, std::io::Error> {
    fs::create_dir_all(&plan.working_dir)?;
    let inherit = env_or("CRUSTCORE_TRY_INHERIT", "false") == "true";
    let stdio = || {
        if inherit {
            Stdio::inherit()
        } else {
            Stdio::piped()
        }
    };
    Command::new(&plan.java)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::null())
        .stdout(stdio())
        .stderr(stdio())
        .kill_on_drop(true)
        .spawn()
}

fn forward<R>(reader: R, prefix: &'static str, tx: mpsc::UnboundedSender<(&'static str, String)>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buffer)
                .trim_end_matches(['\r', '\n'])
                .to_owned();
            if tx.send((prefix, line)).is_err() {
                break;
            }
        }
    });
}

async fn watch(child: &mut Child, secrets: &[&str], settings: &Settings) -> Observation {
    let (tx, mut rx) = mpsc::unbounded_channel();
    if let Some(stdout) = child.stdout.take() {
        forward(stdout, "out", tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        forward(stderr, "err", tx.clone());
    }
    drop(tx);

    let spawned = Instant::now();
    let mut started_at: Option<Instant> = None;
    let mut alive_marker = false;
    let mut errors: Vec<String> = Vec::new();
    let mut ignored = 0;
    let mut in_trace = false;
    let mut output_open = true;

    let exit = loop {
        let deadline = match started_at {
            Some(at) => at + Duration::from_secs(settings.run_secs),
            None => spawned + Duration::from_secs(settings.start_timeout_secs),
        };
        tokio::select! {
            line = rx.recv(), if output_open => {
                let Some((prefix, line)) = line else {
                    output_open = false;
                    continue;
                };
                let mut redacted = line.clone();
                for secret in secrets.iter().filter(|s| !s.is_empty()) {
                    redacted = redacted.replace(secret, "????????");
                }
                println!("  [{prefix}] {redacted}");
                if started_at.is_none() && START_MARKERS.iter().any(|m| line.contains(m)) {
                    started_at = Some(Instant::now());
                    println!("  [try_all] start marker seen, keeping the game up for {} s", settings.run_secs);
                }
                if ALIVE_MARKERS.iter().any(|m| line.contains(m)) {
                    alive_marker = true;
                }
                let is_error = ERROR_MARKERS.iter().any(|m| line.contains(m));
                let is_noise = KNOWN_NOISE.iter().any(|m| line.contains(m));
                let is_trace = line.starts_with("\tat ") || line.starts_with("    at ") || line.starts_with("\t... ");
                if is_error && is_noise {
                    ignored += 1;
                    in_trace = false;
                } else if is_error || (in_trace && is_trace) {
                    if errors.len() < MAX_ERROR_LINES {
                        errors.push(redacted);
                    }
                    in_trace = is_error || is_trace;
                } else {
                    in_trace = false;
                }
            }
            status = child.wait() => {
                while let Ok((prefix, line)) = rx.try_recv() {
                    println!("  [{prefix}] {line}");
                    if ERROR_MARKERS.iter().any(|m| line.contains(m)) && errors.len() < MAX_ERROR_LINES {
                        errors.push(line);
                    }
                }
                break status.ok();
            }
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                println!("  [try_all] stopping the game");
                let _ = child.kill().await;
                let status = if started_at.is_some() {
                    Status::Started
                } else if alive_marker {
                    Status::Alive
                } else {
                    Status::NoStart
                };
                return Observation { status, errors, ignored };
            }
        }
    };

    let code = exit
        .map(|status| status.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let status = match exit {
        Some(status) if status.success() && started_at.is_some() => Status::Closed,
        Some(status) if status.success() => Status::Exited(code),
        _ => Status::Crashed(code),
    };
    Observation {
        status,
        errors,
        ignored,
    }
}

fn print_summary(outcomes: &[Outcome]) {
    let ok = outcomes.iter().filter(|o| o.status.is_ok()).count();
    println!("Summary: {ok}/{} started", outcomes.len());
    for outcome in outcomes {
        let flag = if outcome.status.is_ok() && outcome.errors.is_empty() {
            "OK  "
        } else if outcome.status.is_ok() {
            "WARN"
        } else {
            "FAIL"
        };
        println!(
            "  {flag} {:<16} {} ({} error line(s), {} known)",
            outcome.id,
            outcome.status.label(),
            outcome.errors.len(),
            outcome.ignored
        );
    }
}

fn write_report(outcomes: &[Outcome], settings: &Settings) -> Result<(), Box<dyn Error>> {
    let mut report = String::new();
    writeln!(report, "# try_all report")?;
    writeln!(report)?;
    writeln!(
        report,
        "{} version(s), {} started, {} with error lines",
        outcomes.len(),
        outcomes.iter().filter(|o| o.status.is_ok()).count(),
        outcomes.iter().filter(|o| !o.errors.is_empty()).count()
    )?;
    writeln!(report)?;
    writeln!(
        report,
        "| Version | Type | Result | Time | Error lines | Known noise |"
    )?;
    writeln!(report, "|---|---|---|---|---|---|")?;
    for outcome in outcomes {
        writeln!(
            report,
            "| {} | {} | {} | {:.1} s | {} | {} |",
            outcome.id,
            outcome.kind,
            outcome.status.label(),
            outcome.duration.as_secs_f64(),
            outcome.errors.len(),
            outcome.ignored
        )?;
    }
    let detailed: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| !o.status.is_ok() || !o.errors.is_empty())
        .collect();
    if !detailed.is_empty() {
        writeln!(report)?;
        writeln!(report, "## Details")?;
        for outcome in detailed {
            writeln!(report)?;
            writeln!(report, "### {} ({})", outcome.id, outcome.status.label())?;
            writeln!(report)?;
            writeln!(report, "```")?;
            for line in &outcome.errors {
                writeln!(report, "{line}")?;
            }
            writeln!(report, "```")?;
        }
    }
    fs::write(&settings.report, report)?;
    Ok(())
}

fn event_printer() -> Arc<dyn Fn(Event) + Send + Sync> {
    let last_print = Mutex::new(Instant::now() - Duration::from_secs(1));
    Arc::new(move |event| match event {
        Event::Progress {
            downloaded,
            total,
            element,
        } => {
            let mut last = last_print.lock().expect("poisoned");
            let done = total > 0 && downloaded >= total;
            if done || last.elapsed() >= Duration::from_millis(200) {
                *last = Instant::now();
                let percent = if total > 0 {
                    downloaded as f64 * 100.0 / total as f64
                } else {
                    0.0
                };
                print!("\r[DL] {percent:6.2} % ({element})          ");
                let _ = std::io::stdout().flush();
                if done {
                    println!();
                }
            }
        }
        Event::Check {
            checked,
            total,
            element,
        } => {
            if checked == total || checked % 200 == 0 {
                print!("\r[CHECK] {checked}/{total} {element}          ");
                let _ = std::io::stdout().flush();
                if checked == total {
                    println!();
                }
            }
        }
        Event::Extract(message) => println!("[EXTRACT] {message}"),
        Event::Patch(line) => println!("[PATCH] {line}"),
        Event::Error(message) => eprintln!("[ERROR] {message}"),
        Event::Speed(_) | Event::Estimated(_) => {}
    })
}

async fn load_or_authenticate(auth: &Authenticator) -> Result<Account, Box<dyn Error>> {
    if let Some(stored) = load_account() {
        match auth.refresh(&stored).await {
            Ok(account) => {
                save_account(&account);
                return Ok(account);
            }
            Err(error) => println!("Stored session rejected ({error}), signing in again"),
        }
    }

    let flow = auth.device_code().await?;
    println!();
    println!("==> Open {}", flow.verification_uri_complete());
    println!();
    let account = flow.wait().await?;
    save_account(&account);
    Ok(account)
}

fn load_account() -> Option<Account> {
    let content = fs::read_to_string(account_path()).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_account(account: &Account) {
    let content = serde_json::to_string_pretty(account).expect("account serialization failed");
    fs::write(account_path(), content).expect("failed to write the account file");
}
