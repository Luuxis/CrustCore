use serde::Deserialize;

use crate::network::{self, HttpClient};

pub const LEGACY_METADATA_URL: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge";
pub const METADATA_URL: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
pub const LEGACY_INSTALLER_URL: &str = "https://maven.neoforged.net/releases/net/neoforged/forge/${version}/forge-${version}-installer.jar";
pub const INSTALLER_URL: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge/${version}/neoforge-${version}-installer.jar";

#[derive(Debug, Clone, Deserialize)]
pub struct VersionList {
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("unexpected response from {url} (status {status})")]
    Unexpected { url: String, status: u16 },
}

#[derive(Debug, Clone)]
pub struct NeoForgeMeta {
    http: HttpClient,
}

impl NeoForgeMeta {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub async fn legacy_versions(&self) -> Result<VersionList, Error> {
        self.get_json(LEGACY_METADATA_URL).await
    }

    pub async fn versions(&self) -> Result<VersionList, Error> {
        self.get_json(METADATA_URL).await
    }

    pub fn legacy_installer_url(build: &str) -> String {
        LEGACY_INSTALLER_URL.replace("${version}", build)
    }

    pub fn installer_url(build: &str) -> String {
        INSTALLER_URL.replace("${version}", build)
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

pub fn minecraft_version_of(version: &str) -> Option<String> {
    if let Some(rest) = version.strip_prefix("0.")
        && let Some(end) = rest.find('.')
        && end > 0
    {
        return Some(rest[..end].to_owned());
    }
    let numeric = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = numeric.split('.').collect();
    if parts.len() >= 4 {
        let mut mc = format!("{}.{}", parts[0], parts[1]);
        if parts[2] != "0" {
            mc.push('.');
            mc.push_str(parts[2]);
        }
        if let Some(index) = version.find('+') {
            let suffix = &version[index + 1..];
            for prefix in ["snapshot-", "pre-"] {
                if let Some(number) = suffix.strip_prefix(prefix) {
                    let digits: String = number.chars().take_while(char::is_ascii_digit).collect();
                    if !digits.is_empty() {
                        mc.push('-');
                        mc.push_str(prefix);
                        mc.push_str(&digits);
                    }
                    break;
                }
            }
        }
        return Some(mc);
    }
    if parts.len() == 3 {
        if parts[1] == "0" {
            return Some(format!("1.{}", parts[0]));
        }
        return Some(format!("1.{}.{}", parts[0], parts[1]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_neoforge_versions_to_minecraft() {
        assert_eq!(
            minecraft_version_of("0.25w14craftmine.3-beta").as_deref(),
            Some("25w14craftmine")
        );
        assert_eq!(
            minecraft_version_of("26.1.0.0-alpha.1+snapshot-1").as_deref(),
            Some("26.1-snapshot-1")
        );
        assert_eq!(
            minecraft_version_of("26.1.0.0-alpha.15+pre-3").as_deref(),
            Some("26.1-pre-3")
        );
        assert_eq!(
            minecraft_version_of("26.1.0.1-beta").as_deref(),
            Some("26.1")
        );
        assert_eq!(
            minecraft_version_of("26.1.1.0-beta").as_deref(),
            Some("26.1.1")
        );
        assert_eq!(minecraft_version_of("21.4.121").as_deref(), Some("1.21.4"));
        assert_eq!(minecraft_version_of("21.0.143").as_deref(), Some("1.21"));
        assert_eq!(minecraft_version_of("21.1.72").as_deref(), Some("1.21.1"));
        assert_eq!(minecraft_version_of("nope"), None);
    }
}
