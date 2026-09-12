use serde::{Deserialize, Serialize};

use crate::foundation::time;
use crate::providers::mojang::{Cape, Skin};

const EXPIRY_MARGIN_MILLIS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub access_token: String,
    pub client_token: String,
    pub uuid: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub user_properties: String,
    pub meta: AccountMeta,
    #[serde(
        rename = "xboxAccount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub xbox_account: Option<XboxAccount>,
    #[serde(default)]
    pub profile: AccountProfile,
    #[serde(rename = "clientId", default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_info: Option<AzAuthUserInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMeta {
    #[serde(rename = "type")]
    pub kind: AccountType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token_expires_in: Option<u64>,
    #[serde(default)]
    pub demo: bool,
    #[serde(default)]
    pub ownership: Ownership,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entitlements: Vec<String>,
}

impl AccountMeta {
    pub fn xbox(
        access_token_expires_in: u64,
        ownership: Ownership,
        entitlements: Vec<String>,
    ) -> Self {
        Self {
            kind: AccountType::Xbox,
            online: None,
            access_token_expires_in: Some(access_token_expires_in),
            demo: false,
            ownership,
            entitlements,
        }
    }

    pub fn offline(kind: AccountType, online: bool) -> Self {
        Self {
            kind,
            online: Some(online),
            access_token_expires_in: None,
            demo: false,
            ownership: Ownership::Unknown,
            entitlements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountType {
    Xbox,
    Mojang,
    #[serde(rename = "AZauth")]
    AzAuth,
}

impl AccountType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Xbox => "Xbox",
            Self::Mojang => "Mojang",
            Self::AzAuth => "AZauth",
        }
    }

    pub fn user_type(self) -> &'static str {
        match self {
            Self::Xbox => "msa",
            Self::Mojang => "Mojang",
            Self::AzAuth => "AZauth",
        }
    }
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AzAuthUserInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub money: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Owned,
    GamePass,
    #[default]
    Unknown,
}

impl Account {
    pub fn kind(&self) -> AccountType {
        self.meta.kind
    }

    pub fn is_expired(&self) -> bool {
        match self.meta.access_token_expires_in {
            Some(expires_at) => {
                time::unix_now_millis().saturating_add(EXPIRY_MARGIN_MILLIS) >= expires_at
            }
            None => false,
        }
    }

    pub fn is_game_pass(&self) -> bool {
        self.meta.ownership == Ownership::GamePass
    }

    pub fn xuid(&self) -> Option<&str> {
        self.xbox_account
            .as_ref()
            .and_then(|xbox| xbox.xuid.as_deref())
            .filter(|xuid| !xuid.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_types_map_to_node_user_types() {
        assert_eq!(AccountType::Xbox.user_type(), "msa");
        assert_eq!(AccountType::Mojang.user_type(), "Mojang");
        assert_eq!(AccountType::AzAuth.user_type(), "AZauth");
        assert_eq!(
            serde_json::to_string(&AccountType::AzAuth).unwrap(),
            "\"AZauth\""
        );
        assert_eq!(
            serde_json::to_string(&AccountType::Xbox).unwrap(),
            "\"Xbox\""
        );
    }

    #[test]
    fn expiry_only_applies_to_accounts_with_a_deadline() {
        let mut meta = AccountMeta::offline(AccountType::Mojang, false);
        let account = Account {
            access_token: "a".into(),
            client_token: "c".into(),
            uuid: "u".into(),
            name: "n".into(),
            refresh_token: None,
            user_properties: "{}".into(),
            meta: meta.clone(),
            xbox_account: None,
            profile: AccountProfile::default(),
            client_id: None,
            user_info: None,
        };
        assert!(!account.is_expired());
        assert_eq!(account.xuid(), None);
        meta.access_token_expires_in = Some(1);
        let expired = Account { meta, ..account };
        assert!(expired.is_expired());
    }
}
