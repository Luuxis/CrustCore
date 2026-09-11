use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::Error;
use crate::foundation::time;
use crate::network::HttpClient;
use crate::providers::microsoft::{DeviceCode, DeviceCodePoll, MicrosoftOAuth, TokenResponse};
use crate::providers::xbox::XboxLive;

const SLOW_DOWN_STEP_SECS: u64 = 5;

#[derive(Debug, Clone)]
pub struct MicrosoftAuthenticator {
    oauth: MicrosoftOAuth,
    xbox: XboxLive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XboxSession {
    pub user_hash: String,
    pub xbl_token: String,
    pub xsts_token: String,
    pub not_after: String,
    pub xuid: Option<String>,
    pub gamertag: Option<String>,
    pub age_group: Option<String>,
}

impl MicrosoftAuthenticator {
    pub fn new(http: HttpClient, oauth: MicrosoftOAuth) -> Self {
        Self {
            oauth,
            xbox: XboxLive::new(http),
        }
    }

    pub fn oauth(&self) -> &MicrosoftOAuth {
        &self.oauth
    }

    pub fn xbox(&self) -> &XboxLive {
        &self.xbox
    }

    pub async fn request_device_code(&self) -> Result<DeviceCode, Error> {
        Ok(self.oauth.request_device_code().await?)
    }

    pub async fn wait_device_code(&self, code: &DeviceCode) -> Result<MicrosoftSession, Error> {
        let deadline = time::expires_at(code.expires_in);
        let mut interval = code.interval.max(1);
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            match self.oauth.poll_device_code(&code.device_code).await? {
                DeviceCodePoll::Pending => {}
                DeviceCodePoll::SlowDown => interval += SLOW_DOWN_STEP_SECS,
                DeviceCodePoll::Declined => return Err(Error::DeviceCodeDeclined),
                DeviceCodePoll::Expired => return Err(Error::DeviceCodeExpired),
                DeviceCodePoll::Authorized(token) => return Ok(Self::session(token)),
            }
            if time::is_expired(deadline, 0) {
                return Err(Error::DeviceCodeExpired);
            }
        }
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<MicrosoftSession, Error> {
        let token = self
            .oauth
            .exchange_code(code, redirect_uri, code_verifier)
            .await?;
        Ok(Self::session(token))
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<MicrosoftSession, Error> {
        let mut token = self.oauth.refresh_token(refresh_token).await?;
        if token.refresh_token.is_none() {
            token.refresh_token = Some(refresh_token.to_owned());
        }
        Ok(Self::session(token))
    }

    pub async fn xbox_session(&self, microsoft_access_token: &str) -> Result<XboxSession, Error> {
        let xbl = self.xbox.authenticate(microsoft_access_token).await?;
        let xsts = self.xbox.authorize_minecraft(&xbl.token).await?;
        let user_hash = xsts
            .user_hash()
            .or_else(|| xbl.user_hash())
            .ok_or(Error::MissingUserHash)?
            .to_owned();
        let account = self.xbox.authorize_xboxlive(&xbl.token).await?;
        let claim = account.display_claims.xui.first();
        Ok(XboxSession {
            user_hash,
            xbl_token: xbl.token,
            xsts_token: xsts.token,
            not_after: xsts.not_after,
            xuid: claim.and_then(|c| c.xid.clone()),
            gamertag: claim.and_then(|c| c.gtg.clone()),
            age_group: claim.and_then(|c| c.agg.clone()),
        })
    }

    fn session(token: TokenResponse) -> MicrosoftSession {
        MicrosoftSession {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: time::expires_at(token.expires_in),
        }
    }
}
