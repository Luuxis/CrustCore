use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::network::{self, HttpClient, Response};

pub const BASE_URL: &str = "https://api.minecraftservices.com";

pub const ENTITLEMENT_PRODUCT_MINECRAFT: &str = "product_minecraft";
pub const ENTITLEMENT_GAME_MINECRAFT: &str = "game_minecraft";
pub const ENTITLEMENT_GAME_PASS_PC: &str = "product_game_pass_pc";
pub const ENTITLEMENT_GAME_PASS_ULTIMATE: &str = "product_game_pass_ultimate";

#[derive(Debug, Clone)]
pub struct MinecraftServices {
    http: HttpClient,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinecraftToken {
    pub username: String,
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entitlements {
    #[serde(default)]
    pub items: Vec<Entitlement>,
    #[serde(default)]
    pub signature: String,
    #[serde(rename = "keyId", default)]
    pub key_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entitlement {
    pub name: String,
    #[serde(default)]
    pub signature: String,
}

impl Entitlements {
    pub fn contains(&self, name: &str) -> bool {
        self.items.iter().any(|item| item.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub skins: Vec<Skin>,
    #[serde(default)]
    pub capes: Vec<Cape>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cape {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}

#[derive(Debug, Clone, Deserialize, thiserror::Error)]
#[error("minecraft services error {status}: {} ({})",
    error.as_deref().unwrap_or("unknown"),
    error_message.as_deref().unwrap_or("no message"))]
pub struct ApiError {
    #[serde(skip)]
    pub status: u16,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(rename = "errorMessage", default)]
    pub error_message: Option<String>,
    #[serde(rename = "developerMessage", default)]
    pub developer_message: Option<String>,
}

impl ApiError {
    pub fn is_not_found(&self) -> bool {
        self.status == 404 || self.error.as_deref() == Some("NOT_FOUND")
    }

    pub fn is_forbidden(&self) -> bool {
        self.status == 403
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("unexpected response from minecraft services (status {status}): {body}")]
    Unexpected { status: u16, body: String },
}

impl MinecraftServices {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub async fn login_with_xbox(
        &self,
        user_hash: &str,
        xsts_token: &str,
    ) -> Result<MinecraftToken, Error> {
        let body = json!({ "identityToken": format!("XBL3.0 x={user_hash};{xsts_token}") });
        let response = self
            .http
            .post_json(
                &format!("{BASE_URL}/authentication/login_with_xbox"),
                &body,
                None,
            )
            .await?;
        Self::parse(response)
    }

    pub async fn entitlements(&self, access_token: &str) -> Result<Entitlements, Error> {
        let response = self
            .http
            .get(
                &format!("{BASE_URL}/entitlements/mcstore"),
                Some(access_token),
            )
            .await?;
        Self::parse(response)
    }

    pub async fn profile(&self, access_token: &str) -> Result<Profile, Error> {
        let response = self
            .http
            .get(&format!("{BASE_URL}/minecraft/profile"), Some(access_token))
            .await?;
        Self::parse(response)
    }

    fn parse<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, Error> {
        if response.is_success() {
            return Ok(response.json()?);
        }
        match response.json::<ApiError>() {
            Ok(mut error) => {
                error.status = response.status;
                Err(Error::Api(error))
            }
            Err(_) => Err(Error::Unexpected {
                status: response.status,
                body: response.body,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entitlements() {
        let response = Response {
            status: 200,
            body: r#"{
                "items": [
                    {"name": "product_minecraft", "signature": "jwt"},
                    {"name": "game_minecraft", "signature": "jwt"},
                    {"name": "product_game_pass_pc", "signature": "jwt"}
                ],
                "signature": "jwt",
                "keyId": "1"
            }"#
            .into(),
        };
        let entitlements: Entitlements = MinecraftServices::parse(response).unwrap();
        assert_eq!(entitlements.items.len(), 3);
        assert!(entitlements.contains(ENTITLEMENT_GAME_MINECRAFT));
        assert!(entitlements.contains(ENTITLEMENT_GAME_PASS_PC));
        assert!(!entitlements.contains(ENTITLEMENT_GAME_PASS_ULTIMATE));
        assert_eq!(entitlements.key_id, "1");
    }

    #[test]
    fn parses_profile() {
        let response = Response {
            status: 200,
            body: r#"{
                "id": "986dec87b7ec47ff89ff033fdb95c4b5",
                "name": "HowDoesAuthWork",
                "skins": [{"id": "6a6e65e5", "state": "ACTIVE", "url": "http://x", "variant": "CLASSIC", "alias": "STEVE"}],
                "capes": []
            }"#
            .into(),
        };
        let profile: Profile = MinecraftServices::parse(response).unwrap();
        assert_eq!(profile.name, "HowDoesAuthWork");
        assert_eq!(profile.skins[0].alias.as_deref(), Some("STEVE"));
        assert!(profile.capes.is_empty());
    }

    #[test]
    fn parses_not_found_error() {
        let response = Response {
            status: 404,
            body: r#"{"path":"/minecraft/profile","error":"NOT_FOUND","errorMessage":"The server has not found anything matching the request URI"}"#.into(),
        };
        match MinecraftServices::parse::<Profile>(response) {
            Err(Error::Api(error)) => {
                assert!(error.is_not_found());
                assert_eq!(error.status, 404);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}
