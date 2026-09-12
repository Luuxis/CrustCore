use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::network::{self, HttpClient};

#[derive(Debug, Clone)]
pub struct AzAuthClient {
    http: HttpClient,
    auth_url: String,
    skin_url: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UserData {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub money: Option<f64>,
    #[serde(default)]
    pub role: Option<serde_json::Value>,
    #[serde(default)]
    pub banned: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthOutcome {
    TwoFactorRequired,
    User(UserData),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinData {
    pub url: String,
    pub base64: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("azauth error: {reason} ({})", message.as_deref().unwrap_or("no message"))]
    Rejected {
        reason: String,
        message: Option<String>,
    },
    #[error("unexpected response from azauth (status {status}): {body}")]
    Unexpected { status: u16, body: String },
}

impl AzAuthClient {
    pub fn new(http: HttpClient, base_url: &str) -> Result<Self, url::ParseError> {
        let base = Url::parse(base_url)?;
        Ok(Self {
            http,
            auth_url: base.join("/api/auth")?.to_string(),
            skin_url: base.join("/api/skin-api/skins")?.to_string(),
        })
    }

    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }

    pub fn skin_url(&self) -> &str {
        &self.skin_url
    }

    pub async fn authenticate(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<AuthOutcome, Error> {
        let body = json!({ "email": email, "password": password, "code": code });
        let response = self
            .http
            .post_json(&format!("{}/authenticate", self.auth_url), &body, None)
            .await?;
        parse_outcome(response.status, &response.body)
    }

    pub async fn verify(&self, access_token: &str) -> Result<UserData, Error> {
        let body = json!({ "access_token": access_token });
        let response = self
            .http
            .post_json(&format!("{}/verify", self.auth_url), &body, None)
            .await?;
        match parse_outcome(response.status, &response.body)? {
            AuthOutcome::User(user) => Ok(user),
            AuthOutcome::TwoFactorRequired => Err(Error::Unexpected {
                status: response.status,
                body: response.body,
            }),
        }
    }

    pub async fn logout(&self, access_token: &str) -> Result<bool, Error> {
        let body = json!({ "access_token": access_token });
        let response = self
            .http
            .post_json(&format!("{}/logout", self.auth_url), &body, None)
            .await?;
        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
        Ok(!value
            .get("error")
            .is_some_and(|e| !e.is_null() && e != false))
    }

    pub async fn skin(&self, id: &str) -> Result<SkinData, Error> {
        let url = format!("{}/{id}", self.skin_url);
        let response = self
            .http
            .inner()
            .get(&url)
            .send()
            .await
            .map_err(network::Error::from)?;
        if response.status().as_u16() == 404 {
            return Ok(SkinData { url, base64: None });
        }
        let bytes = response.bytes().await.map_err(network::Error::from)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(SkinData {
            url,
            base64: Some(format!("data:image/png;base64,{encoded}")),
        })
    }
}

pub fn parse_outcome(status: u16, body: &str) -> Result<AuthOutcome, Error> {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) => {
            return Err(Error::Unexpected {
                status,
                body: body.to_owned(),
            });
        }
    };
    let state = value.get("status").and_then(|s| s.as_str());
    let reason = value.get("reason").and_then(|r| r.as_str());
    if state == Some("pending") && reason == Some("2fa") {
        return Ok(AuthOutcome::TwoFactorRequired);
    }
    if state == Some("error") {
        return Err(Error::Rejected {
            reason: reason.unwrap_or("unknown").to_owned(),
            message: value
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_owned),
        });
    }
    match serde_json::from_value::<UserData>(value) {
        Ok(user) => Ok(AuthOutcome::User(user)),
        Err(_) => Err(Error::Unexpected {
            status,
            body: body.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_urls_from_the_site_root() {
        let client =
            AzAuthClient::new(HttpClient::new().unwrap(), "https://example.com/site/").unwrap();
        assert_eq!(client.auth_url(), "https://example.com/api/auth");
        assert_eq!(client.skin_url(), "https://example.com/api/skin-api/skins");
    }

    #[test]
    fn detects_two_factor_and_errors() {
        assert_eq!(
            parse_outcome(200, r#"{"status":"pending","reason":"2fa"}"#).unwrap(),
            AuthOutcome::TwoFactorRequired
        );
        match parse_outcome(
            422,
            r#"{"status":"error","reason":"invalid_credentials","message":"Bad password"}"#,
        ) {
            Err(Error::Rejected { reason, message }) => {
                assert_eq!(reason, "invalid_credentials");
                assert_eq!(message.as_deref(), Some("Bad password"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_user_data() {
        let outcome = parse_outcome(
            200,
            r##"{"id":12,"username":"Luuxis","uuid":"9a2f","access_token":"tok","email_verified":true,"money":42.5,"role":{"name":"Admin","color":"#ff0000"},"banned":false}"##,
        )
        .unwrap();
        match outcome {
            AuthOutcome::User(user) => {
                assert_eq!(user.username, "Luuxis");
                assert_eq!(user.uuid, "9a2f");
                assert_eq!(user.money, Some(42.5));
                assert_eq!(user.email_verified, Some(true));
                assert_eq!(user.role.unwrap()["name"], "Admin");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
