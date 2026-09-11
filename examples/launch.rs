use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use crust_core::authenticator::{Account, Authenticator, Error};

const CLIENT_ID: Option<&str> = None;
const DATA_DIR: &str = "./data";
const ACCOUNT_FILE: &str = "account.json";

fn data_dir() -> PathBuf {
    let dir = PathBuf::from(DATA_DIR);
    fs::create_dir_all(&dir).expect("failed to create the data directory");
    dir
}

fn account_path() -> PathBuf {
    data_dir().join(ACCOUNT_FILE)
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

async fn run() -> Result<(), Error> {
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
        account.xbox_account.gamertag.as_deref().unwrap_or("?"),
        account.meta.ownership
    );

    Ok(())
}

async fn load_or_authenticate(auth: &Authenticator) -> Result<Account, Error> {
    if let Some(refresh_token) = load_refresh_token() {
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

fn load_refresh_token() -> Option<String> {
    let content = fs::read_to_string(account_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value["refresh_token"].as_str().map(str::to_owned)
}

fn save_account(account: &Account) {
    let content = serde_json::to_string_pretty(account).expect("account serialization failed");
    fs::write(account_path(), content).expect("failed to write the account file");
}
