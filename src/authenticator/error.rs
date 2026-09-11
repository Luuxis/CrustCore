use super::Ownership;
use crate::network;
use crate::providers::{microsoft, mojang, xbox};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error(transparent)]
    Microsoft(#[from] microsoft::Error),
    #[error(transparent)]
    Xbox(#[from] xbox::Error),
    #[error(transparent)]
    Minecraft(#[from] mojang::Error),
    #[error("the user declined the sign-in request")]
    DeviceCodeDeclined,
    #[error("the device code expired before the user signed in")]
    DeviceCodeExpired,
    #[error("the redirect url is not a valid url or has no `code` parameter")]
    InvalidRedirectUrl,
    #[error("the `state` of the redirect does not match the authorization request")]
    StateMismatch,
    #[error("authorization denied: {error} ({})", description.as_deref().unwrap_or("no description"))]
    AuthorizationDenied {
        error: String,
        description: Option<String>,
    },
    #[error(
        "microsoft did not return a refresh token; make sure the `offline_access` scope is requested"
    )]
    MissingRefreshToken,
    #[error("xbox live response did not contain a user hash")]
    MissingUserHash,
    #[error("{}", no_profile_message(*ownership))]
    NoProfile { ownership: Ownership },
}

fn no_profile_message(ownership: Ownership) -> &'static str {
    match ownership {
        Ownership::GamePass => {
            "the account has Game Pass but no Minecraft profile yet: sign in once to the official Minecraft Launcher to choose a username"
        }
        Ownership::Owned => {
            "the account owns Minecraft but has no profile yet: sign in once to the official Minecraft Launcher to choose a username"
        }
        Ownership::Unknown => {
            "no Minecraft profile found: the account does not own Minecraft, or has Game Pass and must sign in once to the official Minecraft Launcher"
        }
    }
}
