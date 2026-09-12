use serde::Deserialize;

use crate::foundation::options::LoaderKind;
use crate::network::{self, HttpClient};

#[derive(Debug, Clone, Copy)]
pub struct Endpoints {
    pub metadata: &'static str,
    pub profile: &'static str,
}

pub const FABRIC: Endpoints = Endpoints {
    metadata: "https://meta.fabricmc.net/v2/versions",
    profile: "https://meta.fabricmc.net/v2/versions/loader/${version}/${build}/profile/json",
};

pub const LEGACY_FABRIC: Endpoints = Endpoints {
    metadata: "https://meta.legacyfabric.net/v2/versions",
    profile: "https://meta.legacyfabric.net/v2/versions/loader/${version}/${build}/profile/json",
};

pub const QUILT: Endpoints = Endpoints {
    metadata: "https://meta.quiltmc.org/v3/versions",
    profile: "https://meta.quiltmc.org/v3/versions/loader/${version}/${build}/profile/json",
};

impl Endpoints {
    pub fn for_kind(kind: LoaderKind) -> Option<Self> {
        match kind {
            LoaderKind::Fabric => Some(FABRIC),
            LoaderKind::LegacyFabric => Some(LEGACY_FABRIC),
            LoaderKind::Quilt => Some(QUILT),
            LoaderKind::Forge | LoaderKind::NeoForge => None,
        }
    }

    pub fn profile_url(&self, version: &str, build: &str) -> String {
        self.profile
            .replacen("${build}", build, 1)
            .replacen("${version}", version, 1)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionsMeta {
    #[serde(default)]
    pub game: Vec<GameVersion>,
    #[serde(default)]
    pub loader: Vec<LoaderVersion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameVersion {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderVersion {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("unexpected response from {url} (status {status})")]
    Unexpected { url: String, status: u16 },
}

#[derive(Debug, Clone)]
pub struct FabricMeta {
    http: HttpClient,
    endpoints: Endpoints,
}

impl FabricMeta {
    pub fn new(http: HttpClient, endpoints: Endpoints) -> Self {
        Self { http, endpoints }
    }

    pub fn endpoints(&self) -> Endpoints {
        self.endpoints
    }

    pub async fn versions(&self) -> Result<VersionsMeta, Error> {
        self.get_json(self.endpoints.metadata).await
    }

    pub async fn profile(&self, version: &str, build: &str) -> Result<serde_json::Value, Error> {
        self.get_json(&self.endpoints.profile_url(version, build))
            .await
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, Error> {
        let response = self.http.get(url, None).await?;
        if !response.is_success() {
            return Err(Error::Unexpected {
                url: url.to_owned(),
                status: response.status,
            });
        }
        Ok(response.json()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_profile_url() {
        assert_eq!(
            FABRIC.profile_url("1.20.1", "0.16.9"),
            "https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.16.9/profile/json"
        );
        assert_eq!(
            QUILT.profile_url("1.20.1", "0.26.0"),
            "https://meta.quiltmc.org/v3/versions/loader/1.20.1/0.26.0/profile/json"
        );
    }
}
