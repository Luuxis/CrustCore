use serde::Deserialize;
use url::Url;

use crate::foundation::pkce::{CHALLENGE_METHOD, PkceChallenge};
use crate::network::{self, HttpClient, Response};

pub const DEFAULT_CLIENT_ID: &str = "00000000402b5328";
pub const DEFAULT_SCOPE: &str = "XboxLive.signin offline_access";
pub const LIVE_DESKTOP_REDIRECT_URI: &str = "https://login.live.com/oauth20_desktop.srf";
pub const NATIVE_CLIENT_REDIRECT_URI: &str =
    "https://login.microsoftonline.com/common/oauth2/nativeclient";

const LIVE_COBRAND_ID: &str = "8058f65d-ce06-4c30-9559-473c9275a65d";
const GRANT_DEVICE_CODE: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Authority {
    #[default]
    Live,
    Entra,
}

impl Authority {
    pub fn authorize_url(self) -> &'static str {
        match self {
            Self::Live => "https://login.live.com/oauth20_authorize.srf",
            Self::Entra => "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize",
        }
    }

    pub fn token_url(self) -> &'static str {
        match self {
            Self::Live => "https://login.live.com/oauth20_token.srf",
            Self::Entra => "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
        }
    }

    pub fn device_code_url(self) -> &'static str {
        match self {
            Self::Live => "https://login.live.com/oauth20_connect.srf",
            Self::Entra => "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode",
        }
    }

    pub fn default_redirect_uri(self) -> &'static str {
        match self {
            Self::Live => LIVE_DESKTOP_REDIRECT_URI,
            Self::Entra => NATIVE_CLIENT_REDIRECT_URI,
        }
    }

    pub fn supports_pkce(self) -> bool {
        matches!(self, Self::Entra)
    }
}

#[derive(Debug, Clone)]
pub struct MicrosoftOAuth {
    http: HttpClient,
    client_id: String,
    scope: String,
    authority: Authority,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
}

impl DeviceCode {
    pub fn verification_uri_complete(&self) -> String {
        match &self.verification_uri_complete {
            Some(uri) => uri.clone(),
            None => {
                let mut url = Url::parse(&self.verification_uri).expect("server-provided uri");
                url.query_pairs_mut().append_pair("otc", &self.user_code);
                url.into()
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DeviceCodePoll {
    Pending,
    SlowDown,
    Declined,
    Expired,
    Authorized(TokenResponse),
}

#[derive(Debug, Clone, Deserialize, thiserror::Error)]
#[error("{error}: {}", error_description.as_deref().unwrap_or("no description"))]
pub struct OAuthError {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
    #[serde(default)]
    pub error_codes: Vec<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("microsoft oauth error: {0}")]
    OAuth(#[from] OAuthError),
    #[error("unexpected response from microsoft (status {status}): {body}")]
    Unexpected { status: u16, body: String },
}

impl MicrosoftOAuth {
    pub fn new(http: HttpClient, client_id: impl Into<String>, authority: Authority) -> Self {
        Self {
            http,
            client_id: client_id.into(),
            scope: DEFAULT_SCOPE.to_owned(),
            authority,
        }
    }

    pub fn official(http: HttpClient) -> Self {
        Self::new(http, DEFAULT_CLIENT_ID, Authority::Live)
    }

    pub fn azure(http: HttpClient, client_id: impl Into<String>) -> Self {
        Self::new(http, client_id, Authority::Entra)
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn authority(&self) -> Authority {
        self.authority
    }

    pub fn authorize_url(
        &self,
        redirect_uri: &str,
        pkce: Option<&PkceChallenge>,
        state: &str,
    ) -> String {
        let mut url = Url::parse(self.authority.authorize_url()).expect("static url");
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", &self.client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("scope", &self.scope)
                .append_pair("state", state)
                .append_pair("prompt", "select_account");
            match self.authority {
                Authority::Live => {
                    query.append_pair("cobrandid", LIVE_COBRAND_ID);
                }
                Authority::Entra => {
                    query.append_pair("response_mode", "query");
                    if let Some(pkce) = pkce {
                        query
                            .append_pair("code_challenge", pkce.challenge())
                            .append_pair("code_challenge_method", CHALLENGE_METHOD);
                    }
                }
            }
        }
        url.into()
    }

    pub async fn request_device_code(&self) -> Result<DeviceCode, Error> {
        let mut form = vec![
            ("client_id", self.client_id.as_str()),
            ("scope", self.scope.as_str()),
        ];
        if self.authority == Authority::Live {
            form.push(("response_type", "device_code"));
        }
        let response = self
            .http
            .post_form(self.authority.device_code_url(), &form)
            .await?;
        Self::parse::<DeviceCode>(response)?.map_err(Error::OAuth)
    }

    pub async fn poll_device_code(&self, device_code: &str) -> Result<DeviceCodePoll, Error> {
        let response = self
            .token_request(&[
                ("grant_type", GRANT_DEVICE_CODE),
                ("client_id", self.client_id.as_str()),
                ("device_code", device_code),
            ])
            .await?;
        match response {
            Ok(token) => Ok(DeviceCodePoll::Authorized(token)),
            Err(error) => match error.error.as_str() {
                "authorization_pending" => Ok(DeviceCodePoll::Pending),
                "slow_down" => Ok(DeviceCodePoll::SlowDown),
                "authorization_declined" | "access_denied" => Ok(DeviceCodePoll::Declined),
                "expired_token" => Ok(DeviceCodePoll::Expired),
                _ => Err(Error::OAuth(error)),
            },
        }
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<TokenResponse, Error> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("client_id", self.client_id.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ];
        if self.authority == Authority::Entra {
            form.push(("scope", self.scope.as_str()));
        }
        if let Some(verifier) = code_verifier.filter(|_| self.authority.supports_pkce()) {
            form.push(("code_verifier", verifier));
        }
        self.token_request(&form).await?.map_err(Error::OAuth)
    }

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse, Error> {
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("client_id", self.client_id.as_str()),
            ("refresh_token", refresh_token),
        ];
        if self.authority == Authority::Entra {
            form.push(("scope", self.scope.as_str()));
        }
        self.token_request(&form).await?.map_err(Error::OAuth)
    }

    async fn token_request(
        &self,
        form: &[(&str, &str)],
    ) -> Result<Result<TokenResponse, OAuthError>, Error> {
        let response = self
            .http
            .post_form(self.authority.token_url(), form)
            .await?;
        Self::parse(response)
    }

    fn parse<T: serde::de::DeserializeOwned>(
        response: Response,
    ) -> Result<Result<T, OAuthError>, Error> {
        if response.is_success() {
            return Ok(Ok(response.json()?));
        }
        match response.json::<OAuthError>() {
            Ok(error) => Ok(Err(error)),
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

    const LIVE_BASE: &str = "https://login.live.com";
    const ENTRA_BASE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";

    fn query(url: &str) -> Vec<(String, String)> {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn entra_authorize_url_contains_pkce() {
        let oauth = MicrosoftOAuth::azure(HttpClient::new().unwrap(), "client-123");
        let pkce = PkceChallenge::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        let url = oauth.authorize_url("http://localhost:1234", Some(&pkce), "abc");
        let pairs = query(&url);

        assert!(url.starts_with(&format!("{ENTRA_BASE}/authorize?")));
        assert_eq!(get(&pairs, "client_id"), Some("client-123"));
        assert_eq!(get(&pairs, "response_type"), Some("code"));
        assert_eq!(get(&pairs, "redirect_uri"), Some("http://localhost:1234"));
        assert_eq!(get(&pairs, "scope"), Some(DEFAULT_SCOPE));
        assert_eq!(get(&pairs, "state"), Some("abc"));
        assert_eq!(get(&pairs, "code_challenge"), Some(pkce.challenge()));
        assert_eq!(get(&pairs, "code_challenge_method"), Some("S256"));
        assert_eq!(get(&pairs, "cobrandid"), None);
    }

    #[test]
    fn live_authorize_url_matches_official_launcher() {
        let oauth = MicrosoftOAuth::official(HttpClient::new().unwrap());
        let pkce = PkceChallenge::generate();
        let url = oauth.authorize_url(LIVE_DESKTOP_REDIRECT_URI, Some(&pkce), "abc");
        let pairs = query(&url);

        assert!(url.starts_with(&format!("{LIVE_BASE}/oauth20_authorize.srf?")));
        assert_eq!(get(&pairs, "client_id"), Some(DEFAULT_CLIENT_ID));
        assert_eq!(get(&pairs, "redirect_uri"), Some(LIVE_DESKTOP_REDIRECT_URI));
        assert_eq!(get(&pairs, "scope"), Some(DEFAULT_SCOPE));
        assert_eq!(get(&pairs, "cobrandid"), Some(LIVE_COBRAND_ID));
        assert_eq!(get(&pairs, "prompt"), Some("select_account"));
        assert_eq!(get(&pairs, "code_challenge"), None);
        assert_eq!(get(&pairs, "response_mode"), None);
    }

    #[test]
    fn parses_live_device_code_without_message() {
        let response = Response {
            status: 200,
            body: r#"{"user_code":"PR486LHB","device_code":"abc","verification_uri":"https://www.microsoft.com/link","interval":5,"expires_in":900}"#.into(),
        };
        let code = MicrosoftOAuth::parse::<DeviceCode>(response)
            .unwrap()
            .unwrap();
        assert_eq!(code.user_code, "PR486LHB");
        assert_eq!(code.interval, 5);
        assert!(code.message.is_empty());
        assert_eq!(
            code.verification_uri_complete(),
            "https://www.microsoft.com/link?otc=PR486LHB"
        );
    }

    #[test]
    fn prefers_server_provided_complete_uri() {
        let code = DeviceCode {
            device_code: String::new(),
            user_code: "ABC".into(),
            verification_uri: "https://microsoft.com/devicelogin".into(),
            expires_in: 900,
            interval: 5,
            message: String::new(),
            verification_uri_complete: Some("https://example.com/x?otc=ABC".into()),
        };
        assert_eq!(
            code.verification_uri_complete(),
            "https://example.com/x?otc=ABC"
        );
    }

    #[test]
    fn parses_oauth_error_body() {
        let response = Response {
            status: 400,
            body: r#"{"error":"authorization_pending","error_description":"waiting","error_codes":[70016]}"#.into(),
        };
        let parsed = MicrosoftOAuth::parse::<TokenResponse>(response).unwrap();
        let error = parsed.unwrap_err();
        assert_eq!(error.error, "authorization_pending");
        assert_eq!(error.error_codes, vec![70016]);
    }

    #[test]
    fn non_json_error_is_unexpected() {
        let response = Response {
            status: 502,
            body: "<html>bad gateway</html>".into(),
        };
        assert!(matches!(
            MicrosoftOAuth::parse::<TokenResponse>(response),
            Err(Error::Unexpected { status: 502, .. })
        ));
    }
}
