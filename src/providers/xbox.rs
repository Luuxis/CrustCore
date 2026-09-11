use serde::Deserialize;
use serde_json::json;

use crate::network::{self, HttpClient, Response};

pub const USER_AUTHENTICATE_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
pub const XSTS_AUTHORIZE_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
pub const MINECRAFT_RELYING_PARTY: &str = "rp://api.minecraftservices.com/";
pub const XBOXLIVE_RELYING_PARTY: &str = "http://xboxlive.com";

#[derive(Debug, Clone)]
pub struct XboxLive {
    http: HttpClient,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct XboxToken {
    pub issue_instant: String,
    pub not_after: String,
    pub token: String,
    pub display_claims: DisplayClaims,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayClaims {
    #[serde(default)]
    pub xui: Vec<XuiClaim>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct XuiClaim {
    pub uhs: String,
    #[serde(default)]
    pub xid: Option<String>,
    #[serde(default)]
    pub gtg: Option<String>,
    #[serde(default)]
    pub agg: Option<String>,
}

impl XboxToken {
    pub fn user_hash(&self) -> Option<&str> {
        self.display_claims
            .xui
            .first()
            .map(|claim| claim.uhs.as_str())
    }
}

#[derive(Debug, Clone, Deserialize, thiserror::Error)]
#[error("xsts error {xerr} ({}): {message}", self.kind().describe())]
pub struct XstsError {
    #[serde(rename = "XErr")]
    pub xerr: u64,
    #[serde(rename = "Message", default)]
    pub message: String,
    #[serde(rename = "Redirect", default)]
    pub redirect: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XstsErrorKind {
    Banned,
    NoXboxAccount,
    RegionUnavailable,
    AdultVerificationRequired,
    ChildAccount,
    Unknown,
}

impl XstsError {
    pub fn kind(&self) -> XstsErrorKind {
        match self.xerr {
            2148916227 => XstsErrorKind::Banned,
            2148916233 => XstsErrorKind::NoXboxAccount,
            2148916235 => XstsErrorKind::RegionUnavailable,
            2148916236 | 2148916237 => XstsErrorKind::AdultVerificationRequired,
            2148916238 => XstsErrorKind::ChildAccount,
            _ => XstsErrorKind::Unknown,
        }
    }
}

impl XstsErrorKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Banned => "account banned from Xbox",
            Self::NoXboxAccount => "no Xbox account, sign up on minecraft.net first",
            Self::RegionUnavailable => "Xbox Live is unavailable in this region",
            Self::AdultVerificationRequired => "adult verification required",
            Self::ChildAccount => "child account must be added to a family",
            Self::Unknown => "unknown error",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error(transparent)]
    Xsts(#[from] XstsError),
    #[error("unexpected response from xbox live (status {status}): {body}")]
    Unexpected { status: u16, body: String },
}

impl XboxLive {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub async fn authenticate(&self, microsoft_access_token: &str) -> Result<XboxToken, Error> {
        let body = json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={microsoft_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        });
        let response = self
            .http
            .post_json(USER_AUTHENTICATE_URL, &body, None)
            .await?;
        Self::parse(response)
    }

    pub async fn authorize(
        &self,
        xbl_token: &str,
        relying_party: &str,
    ) -> Result<XboxToken, Error> {
        let body = json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token],
            },
            "RelyingParty": relying_party,
            "TokenType": "JWT",
        });
        let response = self.http.post_json(XSTS_AUTHORIZE_URL, &body, None).await?;
        Self::parse(response)
    }

    pub async fn authorize_minecraft(&self, xbl_token: &str) -> Result<XboxToken, Error> {
        self.authorize(xbl_token, MINECRAFT_RELYING_PARTY).await
    }

    pub async fn authorize_xboxlive(&self, xbl_token: &str) -> Result<XboxToken, Error> {
        self.authorize(xbl_token, XBOXLIVE_RELYING_PARTY).await
    }

    fn parse(response: Response) -> Result<XboxToken, Error> {
        if response.is_success() {
            return Ok(response.json()?);
        }
        match response.json::<XstsError>() {
            Ok(error) => Err(Error::Xsts(error)),
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
    fn parses_token_response() {
        let response = Response {
            status: 200,
            body: r#"{
                "IssueInstant": "2020-12-07T19:52:08.4463796Z",
                "NotAfter": "2020-12-21T19:52:08.4463796Z",
                "Token": "token",
                "DisplayClaims": { "xui": [{ "uhs": "userhash" }] }
            }"#
            .into(),
        };
        let token = XboxLive::parse(response).unwrap();
        assert_eq!(token.token, "token");
        assert_eq!(token.user_hash(), Some("userhash"));
    }

    #[test]
    fn parses_xsts_error() {
        let response = Response {
            status: 401,
            body: r#"{"Identity":"0","XErr":2148916238,"Message":"","Redirect":"https://start.ui.xboxlive.com/AddChildToFamily"}"#.into(),
        };
        match XboxLive::parse(response) {
            Err(Error::Xsts(error)) => {
                assert_eq!(error.kind(), XstsErrorKind::ChildAccount);
                assert!(error.redirect.is_some());
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn maps_known_xerr_codes() {
        let kind = |xerr| {
            XstsError {
                xerr,
                message: String::new(),
                redirect: None,
            }
            .kind()
        };
        assert_eq!(kind(2148916227), XstsErrorKind::Banned);
        assert_eq!(kind(2148916233), XstsErrorKind::NoXboxAccount);
        assert_eq!(kind(2148916235), XstsErrorKind::RegionUnavailable);
        assert_eq!(kind(2148916236), XstsErrorKind::AdultVerificationRequired);
        assert_eq!(kind(2148916237), XstsErrorKind::AdultVerificationRequired);
        assert_eq!(kind(2148916238), XstsErrorKind::ChildAccount);
        assert_eq!(kind(1), XstsErrorKind::Unknown);
    }
}
