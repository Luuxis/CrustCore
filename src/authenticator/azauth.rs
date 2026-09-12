use super::{Account, AccountMeta, AccountProfile, AccountType, AzAuthUserInfo, Error};
use crate::network::HttpClient;
use crate::providers::azauth::{AuthOutcome, AzAuthClient, SkinData, UserData};
use crate::providers::mojang::Skin;

#[derive(Debug, Clone)]
pub struct AzAuth {
    client: AzAuthClient,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AzAuthLogin {
    Account(Box<Account>),
    TwoFactorRequired,
}

impl AzAuth {
    pub fn new(http: HttpClient, base_url: &str) -> Result<Self, Error> {
        Ok(Self {
            client: AzAuthClient::new(http, base_url)?,
        })
    }

    pub fn client(&self) -> &AzAuthClient {
        &self.client
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<AzAuthLogin, Error> {
        match self.client.authenticate(username, password, code).await? {
            AuthOutcome::TwoFactorRequired => Ok(AzAuthLogin::TwoFactorRequired),
            AuthOutcome::User(user) => Ok(AzAuthLogin::Account(Box::new(
                self.account_from(user).await?,
            ))),
        }
    }

    pub async fn verify(&self, account: &Account) -> Result<Account, Error> {
        let user = self.client.verify(&account.access_token).await?;
        self.account_from(user).await
    }

    pub async fn signout(&self, account: &Account) -> Result<bool, Error> {
        Ok(self.client.logout(&account.access_token).await?)
    }

    async fn account_from(&self, user: UserData) -> Result<Account, Error> {
        let id = user
            .id
            .as_ref()
            .map(|id| match id {
                serde_json::Value::String(value) => value.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let skin = self.client.skin(&id).await?;
        Ok(account_from(user, skin))
    }
}

pub fn account_from(user: UserData, skin: SkinData) -> Account {
    Account {
        access_token: user.access_token,
        client_token: user.uuid.clone(),
        uuid: user.uuid,
        name: user.username,
        refresh_token: None,
        user_properties: "{}".to_owned(),
        meta: AccountMeta::offline(AccountType::AzAuth, false),
        xbox_account: None,
        profile: AccountProfile {
            skins: vec![Skin {
                id: None,
                state: None,
                url: skin.url,
                variant: None,
                alias: None,
                base64: skin.base64,
            }],
            capes: Vec::new(),
        },
        client_id: None,
        user_info: Some(AzAuthUserInfo {
            id: user.id,
            banned: user.banned,
            money: user.money,
            role: user.role,
            verified: user.email_verified,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_user_data_to_account_like_node() {
        let user = UserData {
            id: Some(serde_json::json!(12)),
            username: "Luuxis".into(),
            uuid: "9a2f".into(),
            access_token: "tok".into(),
            email_verified: Some(true),
            money: Some(42.5),
            role: Some(serde_json::json!({"name": "Admin"})),
            banned: Some(false),
        };
        let skin = SkinData {
            url: "https://example.com/api/skin-api/skins/12".into(),
            base64: Some("data:image/png;base64,AAAA".into()),
        };
        let account = account_from(user, skin);
        assert_eq!(account.client_token, "9a2f");
        assert_eq!(account.uuid, "9a2f");
        assert_eq!(account.meta.kind, AccountType::AzAuth);
        assert_eq!(account.meta.online, Some(false));
        let info = account.user_info.clone().unwrap();
        assert_eq!(info.verified, Some(true));
        assert_eq!(info.money, Some(42.5));
        assert_eq!(
            account.profile.skins[0].base64.as_deref(),
            Some("data:image/png;base64,AAAA")
        );

        let json = serde_json::to_value(&account).unwrap();
        assert_eq!(json["meta"]["type"], "AZauth");
        assert_eq!(json["user_info"]["banned"], false);
        assert!(json.get("refresh_token").is_none());
        assert!(json["profile"]["skins"][0].get("id").is_none());
    }
}
