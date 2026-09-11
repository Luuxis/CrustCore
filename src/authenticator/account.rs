use serde::{Deserialize, Serialize};

use crate::foundation::time;
use crate::providers::mojang::{Cape, Skin};

const EXPIRY_MARGIN_MILLIS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub access_token: String,
    pub client_token: String,
    pub uuid: String,
    pub name: String,
    pub refresh_token: String,
    pub user_properties: String,
    pub meta: AccountMeta,
    #[serde(rename = "xboxAccount")]
    pub xbox_account: XboxAccount,
    pub profile: AccountProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMeta {
    #[serde(rename = "type")]
    pub kind: AccountType,
    pub access_token_expires_in: u64,
    pub demo: bool,
    pub ownership: Ownership,
    pub entitlements: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountType {
    Xbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XboxAccount {
    pub xuid: Option<String>,
    pub gamertag: Option<String>,
    #[serde(rename = "ageGroup")]
    pub age_group: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProfile {
    #[serde(default)]
    pub skins: Vec<Skin>,
    #[serde(default)]
    pub capes: Vec<Cape>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Owned,
    GamePass,
    Unknown,
}

impl Account {
    pub fn is_expired(&self) -> bool {
        time::unix_now_millis().saturating_add(EXPIRY_MARGIN_MILLIS)
            >= self.meta.access_token_expires_in
    }

    pub fn is_game_pass(&self) -> bool {
        self.meta.ownership == Ownership::GamePass
    }
}
