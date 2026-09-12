use std::collections::HashMap;

use serde::Deserialize;

use crate::network::{self, HttpClient};

pub const METADATA_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json";
pub const PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
pub const META_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/${build}/meta.json";
pub const INSTALLER_URL: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/${version}/forge-${version}-installer";
pub const UNIVERSAL_URL: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/${version}/forge-${version}-universal";
pub const CLIENT_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/${version}/forge-${version}-client";

const FALLBACK_METADATA: &str = include_str!("../../assets/forge/maven-metadata.json");

pub type MavenMetadata = HashMap<String, Vec<String>>;

#[derive(Debug, Clone, Deserialize)]
pub struct Promotions {
    #[serde(default)]
    pub promos: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BuildMeta {
    #[serde(default)]
    pub classifiers: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("unexpected response from {url} (status {status})")]
    Unexpected { url: String, status: u16 },
}

#[derive(Debug, Clone)]
pub struct ForgeMeta {
    http: HttpClient,
}

impl ForgeMeta {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub async fn maven_metadata(&self) -> MavenMetadata {
        match self.get_json::<MavenMetadata>(METADATA_URL).await {
            Ok(metadata) => metadata,
            Err(_) => serde_json::from_str(FALLBACK_METADATA).unwrap_or_default(),
        }
    }

    pub async fn promotions(&self) -> Result<Promotions, Error> {
        self.get_json(PROMOTIONS_URL).await
    }

    pub async fn build_meta(&self, build: &str) -> Result<BuildMeta, Error> {
        self.get_json(&META_URL.replace("${build}", build)).await
    }

    pub fn installer_url(build: &str) -> String {
        INSTALLER_URL.replace("${version}", build)
    }

    pub fn universal_url(build: &str) -> String {
        UNIVERSAL_URL.replace("${version}", build)
    }

    pub fn client_url(build: &str) -> String {
        CLIENT_URL.replace("${version}", build)
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
    fn embedded_metadata_is_valid() {
        let metadata: MavenMetadata = serde_json::from_str(FALLBACK_METADATA).unwrap();
        assert!(metadata.contains_key("1.12.2"));
        assert!(metadata["1.12.2"].contains(&"1.12.2-14.23.5.2860".to_owned()));
    }

    #[test]
    fn builds_urls() {
        assert_eq!(
            ForgeMeta::installer_url("1.20.1-47.2.0"),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-installer"
        );
    }
}
