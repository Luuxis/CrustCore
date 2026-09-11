use serde::{Deserialize, Serialize};

use super::{
    Account, AccountMeta, AccountProfile, AccountType, Error, Ownership, XboxAccount, XboxSession,
};
use crate::foundation::{jwt, time};
use crate::network::HttpClient;
use crate::providers::mojang::{
    self, ENTITLEMENT_GAME_MINECRAFT, ENTITLEMENT_GAME_PASS_PC, ENTITLEMENT_GAME_PASS_ULTIMATE,
    ENTITLEMENT_PRODUCT_MINECRAFT, MinecraftServices, Profile,
};

#[derive(Debug, Clone)]
pub struct MojangAuthenticator {
    services: MinecraftServices,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinecraftSession {
    pub access_token: String,
    pub expires_in: u64,
    pub xuid: Option<String>,
}

impl MojangAuthenticator {
    pub fn new(http: HttpClient) -> Self {
        Self {
            services: MinecraftServices::new(http),
        }
    }

    pub fn services(&self) -> &MinecraftServices {
        &self.services
    }

    pub async fn login(&self, xbox: &XboxSession) -> Result<MinecraftSession, Error> {
        let token = self
            .services
            .login_with_xbox(&xbox.user_hash, &xbox.xsts_token)
            .await?;
        let xuid = jwt::string_claim(&token.access_token, "xuid");
        Ok(MinecraftSession {
            access_token: token.access_token,
            expires_in: token.expires_in,
            xuid,
        })
    }

    pub async fn entitlements(&self, access_token: &str) -> Vec<String> {
        match self.services.entitlements(access_token).await {
            Ok(entitlements) => entitlements.items.into_iter().map(|e| e.name).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub async fn profile(&self, access_token: &str) -> Result<Option<Profile>, Error> {
        match self.services.profile(access_token).await {
            Ok(profile) => Ok(Some(profile)),
            Err(mojang::Error::Api(error)) if error.is_not_found() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn resolve_account(
        &self,
        session: &MinecraftSession,
        xbox: &XboxSession,
        refresh_token: String,
    ) -> Result<Account, Error> {
        let entitlements = self.entitlements(&session.access_token).await;
        let ownership = ownership_from_names(&entitlements);
        let profile = self
            .profile(&session.access_token)
            .await?
            .ok_or(Error::NoProfile { ownership })?;
        Ok(Account {
            access_token: session.access_token.clone(),
            client_token: profile.id.clone(),
            uuid: profile.id,
            name: profile.name,
            refresh_token,
            user_properties: "{}".to_owned(),
            meta: AccountMeta {
                kind: AccountType::Xbox,
                access_token_expires_in: time::expires_at_millis(session.expires_in),
                demo: false,
                ownership,
                entitlements,
            },
            xbox_account: XboxAccount {
                xuid: xbox.xuid.clone().or_else(|| session.xuid.clone()),
                gamertag: xbox.gamertag.clone(),
                age_group: xbox.age_group.clone(),
            },
            profile: AccountProfile {
                skins: profile.skins,
                capes: profile.capes,
            },
        })
    }
}

pub fn ownership_from_names<S: AsRef<str>>(entitlements: &[S]) -> Ownership {
    let has = |name: &str| entitlements.iter().any(|e| e.as_ref() == name);
    if has(ENTITLEMENT_GAME_MINECRAFT) || has(ENTITLEMENT_PRODUCT_MINECRAFT) {
        Ownership::Owned
    } else if has(ENTITLEMENT_GAME_PASS_PC) || has(ENTITLEMENT_GAME_PASS_ULTIMATE) {
        Ownership::GamePass
    } else {
        Ownership::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_takes_precedence_over_game_pass() {
        let names = [
            "product_minecraft",
            "game_minecraft",
            "product_game_pass_pc",
        ];
        assert_eq!(ownership_from_names(&names), Ownership::Owned);
    }

    #[test]
    fn game_pass_only() {
        assert_eq!(
            ownership_from_names(&["product_game_pass_ultimate"]),
            Ownership::GamePass
        );
    }

    #[test]
    fn empty_is_unknown() {
        let none: [&str; 0] = [];
        assert_eq!(ownership_from_names(&none), Ownership::Unknown);
    }

    #[test]
    fn account_serializes_like_the_javascript_shape() {
        let account = Account {
            access_token: "mc".into(),
            client_token: "ct".into(),
            uuid: "id".into(),
            name: "Luuxis".into(),
            refresh_token: "rt".into(),
            user_properties: "{}".into(),
            meta: AccountMeta {
                kind: AccountType::Xbox,
                access_token_expires_in: 1,
                demo: false,
                ownership: Ownership::GamePass,
                entitlements: vec!["product_game_pass_pc".into()],
            },
            xbox_account: XboxAccount {
                xuid: Some("x".into()),
                gamertag: Some("g".into()),
                age_group: Some("Adult".into()),
            },
            profile: AccountProfile::default(),
        };
        let json = serde_json::to_value(&account).unwrap();
        assert_eq!(json["meta"]["type"], "Xbox");
        assert_eq!(json["meta"]["ownership"], "game_pass");
        assert_eq!(json["xboxAccount"]["ageGroup"], "Adult");
        assert_eq!(json["user_properties"], "{}");
        assert!(json["profile"]["skins"].is_array());
        let back: Account = serde_json::from_value(json).unwrap();
        assert_eq!(back, account);
        assert!(back.is_game_pass());
    }
}
