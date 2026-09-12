use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crust_core::authenticator::{Account, Authenticator};
use crust_core::foundation::events::Event;
use crust_core::launcher::{Launch, LaunchOptions, LoaderKind};
use crust_core::network::HttpClient;

const CLIENT_ID: Option<&str> = None;
const DATA_DIR: &str = "./data";
const ACCOUNT_FILE: &str = "account.json";
const MINECRAFT_PATH: &str = "minecraft";
const API_URL: &str = "https://luuxcraft.fr/api/user/bb8f5247-1d38-41bb-ab6d-3200471a06b2/instances";
const INSTANCE_NAME: &str = "dev";
const DOWNLOAD_SIMULTANEOUS: usize = 30;
const JAVA_VERSION: &str = "default"; // "default" for default system Java, or specify a version like "8", "11", etc.
const JAVA_TYPE: &str = "default"; // "default" for default system Java, or specify a type like "hotspot", "openj9", etc.
const MEMORY_CONFIG: (&str, &str) = ("14G", "16G");
const INTEL_ENABLED_MAC: bool = true;
const JVM_ARGS: &[&str] = &[];

#[derive(Debug, Clone, Deserialize)]
struct Instance {
    name: String,
    url: String,
    #[serde(default)]
    loader: Option<InstanceLoader>,
    #[serde(default)]
    loadder: Option<LegacyInstanceLoader>,
    #[serde(default)]
    verify: bool,
    #[serde(default)]
    ignored: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct InstanceLoader {
    minecraft_version: String,
    loader_type: String,
    #[serde(default)]
    loader_version: Option<String>,
    #[serde(default)]
    mcp_file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyInstanceLoader {
    minecraft_version: String,
    loadder_type: String,
    #[serde(default)]
    loadder_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiError {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn data_dir(subdir: Option<&str>) -> PathBuf {
    let mut dir = PathBuf::from(env_or("CRUSTCORE_DATA_DIR", DATA_DIR));
    if let Some(subdir) = subdir {
        dir.push(subdir);
    }
    fs::create_dir_all(&dir).expect("failed to create the data directory");
    // `canonicalize` would return a `\\?\C:\...` path on Windows, which Java
    // cannot use for `java.library.path`; `absolute` keeps a plain path.
    std::path::absolute(&dir).unwrap_or(dir)
}

fn account_path() -> PathBuf {
    data_dir(None).join(ACCOUNT_FILE)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Fatal error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    println!("Signing in...");
    let auth = match CLIENT_ID {
        Some(client_id) => Authenticator::with_client_id(client_id)?,
        None => Authenticator::new()?,
    };
    let account = load_or_authenticate(&auth).await?;
    println!(
        "Signed in as {} ({}), gamertag: {}, ownership: {:?}",
        account.name,
        account.uuid,
        account
            .xbox_account
            .as_ref()
            .and_then(|xbox| xbox.gamertag.as_deref())
            .unwrap_or("?"),
        account.meta.ownership
    );

    println!("Fetching instance data...");
    let http = HttpClient::new()?;
    let api_url = env_or("CRUSTCORE_API_URL", API_URL);
    let instance_name = env_or("CRUSTCORE_INSTANCE", INSTANCE_NAME);
    let instance = fetch_instance_data(&http, &api_url, &instance_name).await?;

    println!("Launching {}...", instance.name);
    let options = build_launch_options(&instance)?;
    let launch = Launch::new(options, account)?.with_events(event_printer());
    let prepared = launch.prepare().await?;
    let plan = launch.plan(&prepared)?;
    let secrets = launch.secrets();
    let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
    println!(
        "Launching with arguments {}",
        plan.redacted_command(&secrets)
            .replace(&format!("{}/", launch.options().root.display()), "")
    );

    let mut child = launch.spawn(&plan)?;
    let status = child.wait().await?;
    println!("Minecraft exited with {status}");
    Ok(())
}

async fn fetch_instance_data(
    http: &HttpClient,
    api_url: &str,
    instance_name: &str,
) -> Result<Instance, Box<dyn Error>> {
    let response = http.get(api_url, None).await?;
    if let Ok(error) = serde_json::from_str::<ApiError>(&response.body)
        && (error.success == Some(false) || error.status.as_deref() == Some("error"))
    {
        return Err(format!(
            "API error: {}",
            error.message.unwrap_or_else(|| "unknown error".to_owned())
        )
        .into());
    }
    if !response.is_success() {
        return Err(format!("API error: {} {}", response.status, response.body.trim()).into());
    }
    let instances: HashMap<String, Instance> = serde_json::from_str(&response.body)?;
    instances
        .into_values()
        .find(|instance| instance.name.eq_ignore_ascii_case(instance_name))
        .ok_or_else(|| format!("Instance \"{instance_name}\" not found").into())
}

fn build_launch_options(instance: &Instance) -> Result<LaunchOptions, Box<dyn Error>> {
    let (minecraft_version, loader_type, loader_version, mcp_file) =
        match (&instance.loader, &instance.loadder) {
            (Some(loader), _) => (
                loader.minecraft_version.clone(),
                loader.loader_type.to_ascii_lowercase(),
                loader.loader_version.clone(),
                loader.mcp_file.clone(),
            ),
            (None, Some(legacy)) => (
                legacy.minecraft_version.clone(),
                legacy.loadder_type.to_ascii_lowercase(),
                legacy.loadder_version.clone(),
                None,
            ),
            (None, None) => return Err("instance has no loader information".into()),
        };
    let (loader_type, mcp) = if loader_type == "mcp" {
        ("none".to_owned(), mcp_file)
    } else {
        (loader_type, None)
    };

    let mut options = LaunchOptions::new(data_dir(Some(MINECRAFT_PATH)), minecraft_version);
    options.url = Some(instance.url.clone());
    options.instance = Some(instance.name.clone());
    options.intel_enabled_mac = env_or(
        "CRUSTCORE_INTEL_MAC",
        if INTEL_ENABLED_MAC { "true" } else { "false" },
    ) == "true";
    options.ignore_log4j = true;
    options.ignored = instance.ignored.clone();
    options.download_concurrency = DOWNLOAD_SIMULTANEOUS;
    options.verify = instance.verify;
    options.mcp = mcp;
    options.loader.kind = LoaderKind::parse(&loader_type);
    options.loader.build = loader_version.unwrap_or_else(|| "latest".to_owned());
    options.loader.enable = loader_type != "none";
    options.loader.path = "./".to_owned();
    if options.loader.enable && options.loader.kind.is_none() {
        return Err(format!("unknown loader type {loader_type}").into());
    }
    options.jvm_args = JVM_ARGS.iter().map(|s| (*s).to_owned()).collect();
    options.jvm_args.extend(
        env_or("CRUSTCORE_JVM_ARGS", "")
            .split_whitespace()
            .map(str::to_owned),
    );
    options.memory.min = MEMORY_CONFIG.0.to_owned();
    options.memory.max = MEMORY_CONFIG.1.to_owned();
    let java_version = env_or("CRUSTCORE_JAVA_VERSION", JAVA_VERSION);
    let java_type = env_or("CRUSTCORE_JAVA_TYPE", JAVA_TYPE);
    options.java.version = (java_version != "default").then_some(java_version);
    options.java.kind = (java_type != "default").then_some(java_type);
    Ok(options)
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
    } else if let Some(refresh_token) = load_refresh_token() {
        match auth.microsoft().refresh(&refresh_token).await {
            Ok(session) => match auth.complete(session).await {
                Ok(account) => {
                    save_account(&account);
                    return Ok(account);
                }
                Err(error) => println!("Stored session rejected ({error}), signing in again"),
            },
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

fn load_refresh_token() -> Option<String> {
    let content = fs::read_to_string(account_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value["refresh_token"].as_str().map(str::to_owned)
}

fn save_account(account: &Account) {
    let content = serde_json::to_string_pretty(account).expect("account serialization failed");
    fs::write(account_path(), content).expect("failed to write the account file");
}
