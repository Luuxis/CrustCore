use super::{Account, AccountMeta, AccountProfile, AccountType, Error};
use crate::network::HttpClient;
use crate::providers::yggdrasil::{Session, YggdrasilClient};

#[derive(Debug, Clone)]
pub struct Yggdrasil {
    client: YggdrasilClient,
}

impl Yggdrasil {
    pub fn new(http: HttpClient) -> Self {
        Self {
            client: YggdrasilClient::new(http),
        }
    }

    pub fn with_api(http: HttpClient, api_url: impl Into<String>) -> Self {
        Self {
            client: YggdrasilClient::with_api(http, api_url),
        }
    }

    pub fn client(&self) -> &YggdrasilClient {
        &self.client
    }

    pub fn offline(username: &str) -> Account {
        let id = random_hex();
        Account {
            access_token: id.clone(),
            client_token: id.clone(),
            uuid: id,
            name: username.to_owned(),
            refresh_token: None,
            user_properties: "{}".to_owned(),
            meta: AccountMeta::offline(AccountType::Mojang, false),
            xbox_account: None,
            profile: AccountProfile::default(),
            client_id: None,
            user_info: None,
        }
    }

    pub async fn login(&self, username: &str, password: Option<&str>) -> Result<Account, Error> {
        let Some(password) = password.filter(|p| !p.is_empty()) else {
            return Ok(Self::offline(username));
        };
        let client_token = random_hex();
        let session = self
            .client
            .authenticate(username, password, &client_token)
            .await?;
        Ok(account_from(session))
    }

    pub async fn refresh(&self, account: &Account) -> Result<Account, Error> {
        let session = self
            .client
            .refresh(&account.access_token, &account.client_token)
            .await?;
        Ok(account_from(session))
    }

    pub async fn validate(&self, account: &Account) -> Result<bool, Error> {
        Ok(self
            .client
            .validate(&account.access_token, &account.client_token)
            .await?)
    }

    pub async fn signout(&self, account: &Account) -> Result<bool, Error> {
        Ok(self
            .client
            .invalidate(&account.access_token, &account.client_token)
            .await?)
    }
}

fn account_from(session: Session) -> Account {
    Account {
        access_token: session.access_token,
        client_token: session.client_token,
        uuid: session.uuid,
        name: session.name,
        refresh_token: None,
        user_properties: "{}".to_owned(),
        meta: AccountMeta::offline(AccountType::Mojang, true),
        xbox_account: None,
        profile: AccountProfile::default(),
        client_id: None,
        user_info: None,
    }
}

fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_account_matches_node_shape() {
        let account = Yggdrasil::offline("Steve");
        assert_eq!(account.name, "Steve");
        assert_eq!(account.uuid.len(), 32);
        assert!(account.uuid.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(account.access_token, account.uuid);
        assert_eq!(account.client_token, account.uuid);
        assert_eq!(account.meta.kind, AccountType::Mojang);
        assert_eq!(account.meta.online, Some(false));
        assert!(!account.is_expired());

        let json = serde_json::to_value(&account).unwrap();
        assert_eq!(json["meta"]["type"], "Mojang");
        assert_eq!(json["meta"]["online"], false);
        assert!(json.get("refresh_token").is_none());
        assert!(json.get("xboxAccount").is_none());
        assert_eq!(json["user_properties"], "{}");
        assert_ne!(Yggdrasil::offline("Steve").uuid, account.uuid);
    }

    #[test]
    fn online_session_maps_to_account() {
        let account = account_from(Session {
            access_token: "a".into(),
            client_token: "c".into(),
            uuid: "id".into(),
            name: "Alex".into(),
        });
        assert_eq!(account.meta.online, Some(true));
        assert_eq!(account.meta.kind, AccountType::Mojang);
        assert_eq!(account.uuid, "id");
    }
}
