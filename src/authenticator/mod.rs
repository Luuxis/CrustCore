mod account;
pub mod azauth;
mod error;
pub mod microsoft;
pub mod mojang;
pub mod yggdrasil;

use url::Url;

use crate::foundation::pkce::{PkceChallenge, random_state};
use crate::foundation::time;
use crate::network::HttpClient;
use crate::providers::microsoft::{DeviceCode, MicrosoftOAuth};

pub use account::{
    Account, AccountMeta, AccountProfile, AccountType, AzAuthUserInfo, Ownership, XboxAccount,
};
pub use azauth::{AzAuth, AzAuthLogin};
pub use error::Error;
pub use microsoft::{MicrosoftAuthenticator, MicrosoftSession, XboxSession};
pub use mojang::{MinecraftSession, MojangAuthenticator};
pub use yggdrasil::Yggdrasil;

const REFRESH_MARGIN_MILLIS: u64 = 2 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct Authenticator {
    microsoft: MicrosoftAuthenticator,
    mojang: MojangAuthenticator,
}

#[derive(Debug, Clone)]
pub struct AuthorizeRequest {
    pub url: String,
    pub state: String,
    pub redirect_uri: String,
    pub code_verifier: Option<String>,
}

#[derive(Debug)]
pub struct DeviceCodeFlow<'a> {
    authenticator: &'a Authenticator,
    code: DeviceCode,
}

impl Authenticator {
    pub fn new() -> Result<Self, Error> {
        let http = HttpClient::new()?;
        Ok(Self::with_oauth(
            http.clone(),
            MicrosoftOAuth::official(http),
        ))
    }

    pub fn with_client_id(client_id: impl Into<String>) -> Result<Self, Error> {
        let http = HttpClient::new()?;
        Ok(Self::with_oauth(
            http.clone(),
            MicrosoftOAuth::azure(http, client_id),
        ))
    }

    pub fn with_oauth(http: HttpClient, oauth: MicrosoftOAuth) -> Self {
        Self {
            microsoft: MicrosoftAuthenticator::new(http.clone(), oauth),
            mojang: MojangAuthenticator::new(http),
        }
    }

    pub fn microsoft(&self) -> &MicrosoftAuthenticator {
        &self.microsoft
    }

    pub fn mojang(&self) -> &MojangAuthenticator {
        &self.mojang
    }

    pub async fn device_code(&self) -> Result<DeviceCodeFlow<'_>, Error> {
        let code = self.microsoft.request_device_code().await?;
        Ok(DeviceCodeFlow {
            authenticator: self,
            code,
        })
    }

    pub fn authorize_request(&self) -> AuthorizeRequest {
        let redirect_uri = self.microsoft.oauth().authority().default_redirect_uri();
        self.authorize_request_with_redirect(redirect_uri)
    }

    pub fn authorize_request_with_redirect(
        &self,
        redirect_uri: impl Into<String>,
    ) -> AuthorizeRequest {
        let oauth = self.microsoft.oauth();
        let redirect_uri = redirect_uri.into();
        let pkce = oauth
            .authority()
            .supports_pkce()
            .then(PkceChallenge::generate);
        let state = random_state();
        let url = oauth.authorize_url(&redirect_uri, pkce.as_ref(), &state);
        AuthorizeRequest {
            url,
            state,
            redirect_uri,
            code_verifier: pkce.map(|p| p.verifier().to_owned()),
        }
    }

    pub async fn login_with_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<Account, Error> {
        let session = self
            .microsoft
            .exchange_code(code, redirect_uri, code_verifier)
            .await?;
        self.complete(session).await
    }

    pub async fn refresh(&self, account: &Account) -> Result<Account, Error> {
        if let Some(expires_at) = account.meta.access_token_expires_in
            && time::unix_now_millis() < expires_at.saturating_sub(REFRESH_MARGIN_MILLIS)
        {
            return self.refresh_profile(account).await;
        }
        let refresh_token = account
            .refresh_token
            .as_deref()
            .filter(|token| !token.is_empty())
            .ok_or(Error::MissingRefreshToken)?;
        let session = self.microsoft.refresh(refresh_token).await?;
        self.complete(session).await
    }

    pub async fn refresh_profile(&self, account: &Account) -> Result<Account, Error> {
        let profile = self
            .mojang
            .services()
            .profile(&account.access_token)
            .await?;
        let mut updated = account.clone();
        updated.profile = AccountProfile {
            skins: profile.skins,
            capes: profile.capes,
        };
        Ok(updated)
    }

    pub async fn complete(&self, session: MicrosoftSession) -> Result<Account, Error> {
        let refresh_token = session
            .refresh_token
            .clone()
            .ok_or(Error::MissingRefreshToken)?;
        let xbox = self.microsoft.xbox_session(&session.access_token).await?;
        let minecraft = self.mojang.login(&xbox).await?;
        self.mojang
            .resolve_account(&minecraft, &xbox, refresh_token)
            .await
    }
}

impl AuthorizeRequest {
    pub fn extract_code(&self, redirect_url: &str) -> Result<String, Error> {
        let url = Url::parse(redirect_url.trim()).map_err(|_| Error::InvalidRedirectUrl)?;
        let mut code = None;
        let mut state = None;
        let mut error = None;
        let mut description = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                "error_description" => description = Some(value.into_owned()),
                _ => {}
            }
        }
        if let Some(error) = error {
            return Err(Error::AuthorizationDenied { error, description });
        }
        if state.as_deref() != Some(self.state.as_str()) {
            return Err(Error::StateMismatch);
        }
        code.ok_or(Error::InvalidRedirectUrl)
    }

    pub async fn login(&self, auth: &Authenticator, redirect_url: &str) -> Result<Account, Error> {
        let code = self.extract_code(redirect_url)?;
        auth.login_with_code(&code, &self.redirect_uri, self.code_verifier.as_deref())
            .await
    }
}

impl DeviceCodeFlow<'_> {
    pub fn user_code(&self) -> &str {
        &self.code.user_code
    }

    pub fn verification_uri(&self) -> &str {
        &self.code.verification_uri
    }

    pub fn verification_uri_complete(&self) -> String {
        self.code.verification_uri_complete()
    }

    pub fn message(&self) -> &str {
        &self.code.message
    }

    pub fn expires_in(&self) -> u64 {
        self.code.expires_in
    }

    pub fn device_code(&self) -> &DeviceCode {
        &self.code
    }

    pub async fn wait(self) -> Result<Account, Error> {
        let session = self
            .authenticator
            .microsoft
            .wait_device_code(&self.code)
            .await?;
        self.authenticator.complete(session).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(state: &str) -> AuthorizeRequest {
        AuthorizeRequest {
            url: String::new(),
            state: state.to_owned(),
            redirect_uri: "https://login.live.com/oauth20_desktop.srf".to_owned(),
            code_verifier: None,
        }
    }

    #[test]
    fn extracts_code_when_state_matches() {
        let code = request("abc")
            .extract_code(
                "https://login.live.com/oauth20_desktop.srf?code=M.C1_BAY&state=abc&lc=1036",
            )
            .unwrap();
        assert_eq!(code, "M.C1_BAY");
    }

    #[test]
    fn rejects_state_mismatch() {
        let result = request("abc").extract_code("https://x/?code=1&state=other");
        assert!(matches!(result, Err(Error::StateMismatch)));
    }

    #[test]
    fn surfaces_denied_authorization() {
        let result = request("abc")
            .extract_code("https://x/?error=access_denied&error_description=cancelled&state=abc");
        match result {
            Err(Error::AuthorizationDenied { error, description }) => {
                assert_eq!(error, "access_denied");
                assert_eq!(description.as_deref(), Some("cancelled"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            request("abc").extract_code("not a url"),
            Err(Error::InvalidRedirectUrl)
        ));
    }
}
