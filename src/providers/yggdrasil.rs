use serde::Deserialize;
use serde_json::json;

use crate::network::{self, HttpClient, Response};

pub const DEFAULT_API_URL: &str = "https://authserver.mojang.com";

#[derive(Debug, Clone)]
pub struct YggdrasilClient {
    http: HttpClient,
    api_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub access_token: String,
    pub client_token: String,
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AuthResponse {
    #[serde(rename = "accessToken", default)]
    access_token: Option<String>,
    #[serde(rename = "clientToken", default)]
    client_token: Option<String>,
    #[serde(rename = "selectedProfile", default)]
    selected_profile: Option<SelectedProfile>,
    #[serde(default)]
    error: Option<String>,
    #[serde(rename = "errorMessage", default)]
    error_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SelectedProfile {
    id: String,
    name: String,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{error}: {}", message.as_deref().unwrap_or("no message"))]
pub struct ApiError {
    pub error: String,
    pub message: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("yggdrasil error: {0}")]
    Api(#[from] ApiError),
    #[error("unexpected response from yggdrasil (status {status}): {body}")]
    Unexpected { status: u16, body: String },
}

impl YggdrasilClient {
    pub fn new(http: HttpClient) -> Self {
        Self::with_api(http, DEFAULT_API_URL)
    }

    pub fn with_api(http: HttpClient, api_url: impl Into<String>) -> Self {
        Self {
            http,
            api_url: api_url.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        client_token: &str,
    ) -> Result<Session, Error> {
        let body = json!({
            "agent": { "name": "Minecraft", "version": 1 },
            "username": username,
            "password": password,
            "clientToken": client_token,
            "requestUser": true,
        });
        let response = self
            .http
            .post_json(&format!("{}/authenticate", self.api_url), &body, None)
            .await?;
        Self::session(response)
    }

    pub async fn refresh(&self, access_token: &str, client_token: &str) -> Result<Session, Error> {
        let body = json!({
            "accessToken": access_token,
            "clientToken": client_token,
            "requestUser": true,
        });
        let response = self
            .http
            .post_json(&format!("{}/refresh", self.api_url), &body, None)
            .await?;
        Self::session(response)
    }

    pub async fn validate(&self, access_token: &str, client_token: &str) -> Result<bool, Error> {
        let body = json!({ "accessToken": access_token, "clientToken": client_token });
        let response = self
            .http
            .post_json(&format!("{}/validate", self.api_url), &body, None)
            .await?;
        Ok(response.status == 204)
    }

    pub async fn invalidate(&self, access_token: &str, client_token: &str) -> Result<bool, Error> {
        let body = json!({ "accessToken": access_token, "clientToken": client_token });
        let response = self
            .http
            .post_json(&format!("{}/invalidate", self.api_url), &body, None)
            .await?;
        Ok(response.body.is_empty())
    }

    fn session(response: Response) -> Result<Session, Error> {
        let parsed: AuthResponse = match response.json() {
            Ok(parsed) => parsed,
            Err(_) => {
                return Err(Error::Unexpected {
                    status: response.status,
                    body: response.body,
                });
            }
        };
        if let Some(error) = parsed.error {
            return Err(Error::Api(ApiError {
                error,
                message: parsed.error_message,
            }));
        }
        match (
            parsed.access_token,
            parsed.client_token,
            parsed.selected_profile,
        ) {
            (Some(access_token), Some(client_token), Some(profile)) => Ok(Session {
                access_token,
                client_token,
                uuid: profile.id,
                name: profile.name,
            }),
            _ => Err(Error::Unexpected {
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
    fn parses_successful_session() {
        let response = Response {
            status: 200,
            body: r#"{"accessToken":"a","clientToken":"c","selectedProfile":{"id":"id","name":"Steve"},"user":{"id":"x"}}"#.into(),
        };
        let session = YggdrasilClient::session(response).unwrap();
        assert_eq!(session.access_token, "a");
        assert_eq!(session.uuid, "id");
        assert_eq!(session.name, "Steve");
    }

    #[test]
    fn parses_api_error() {
        let response = Response {
            status: 403,
            body: r#"{"error":"ForbiddenOperationException","errorMessage":"Invalid credentials. Invalid username or password."}"#.into(),
        };
        match YggdrasilClient::session(response) {
            Err(Error::Api(error)) => {
                assert_eq!(error.error, "ForbiddenOperationException");
                assert!(error.message.unwrap().contains("Invalid credentials"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn custom_api_url_is_normalised() {
        let client =
            YggdrasilClient::with_api(HttpClient::new().unwrap(), "https://auth.example.com/");
        assert_eq!(client.api_url(), "https://auth.example.com");
    }
}
