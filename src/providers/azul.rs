use serde::Deserialize;

use crate::foundation::os::{Arch, Platform};
use crate::network::{self, HttpClient};

pub const PACKAGES_URL: &str = "https://api.azul.com/metadata/v1/zulu/packages/";

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub download_url: String,
    #[serde(default)]
    pub java_version: Vec<u64>,
    #[serde(default)]
    pub latest: bool,
    #[serde(default)]
    pub package_uuid: String,
    #[serde(default)]
    pub product: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageQuery {
    pub java_version: String,
    pub os: String,
    pub arch: String,
    pub archive_type: String,
    pub java_package_type: String,
}

impl PackageQuery {
    pub fn url(&self) -> String {
        let mut url = url::Url::parse(PACKAGES_URL).expect("static url");
        url.query_pairs_mut()
            .append_pair("java_version", &self.java_version)
            .append_pair("os", &self.os)
            .append_pair("arch", &self.arch)
            .append_pair("archive_type", &self.archive_type)
            .append_pair("java_package_type", &self.java_package_type);
        url.into()
    }
}

pub fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "windows",
        Platform::MacOs => "macos",
        Platform::Linux => "linux",
    }
}

pub fn arch_name(arch: Arch) -> &'static str {
    match arch {
        Arch::X64 => "x64",
        Arch::X86 => "x32",
        Arch::Arm64 => "aarch64",
        Arch::Arm => "arm",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Network(#[from] network::Error),
    #[error("unexpected response from {url} (status {status})")]
    Unexpected { url: String, status: u16 },
}

#[derive(Debug, Clone)]
pub struct AzulMeta {
    http: HttpClient,
}

impl AzulMeta {
    pub fn new(http: HttpClient) -> Self {
        Self { http }
    }

    pub async fn packages(&self, query: &PackageQuery) -> Result<Vec<Package>, Error> {
        let url = query.url();
        let response = self.http.get(&url, None).await?;
        if !response.is_success() {
            return Err(Error::Unexpected {
                url,
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
    fn builds_query_url_like_node() {
        let query = PackageQuery {
            java_version: "17".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            archive_type: "zip".into(),
            java_package_type: "jre".into(),
        };
        assert_eq!(
            query.url(),
            "https://api.azul.com/metadata/v1/zulu/packages/?java_version=17&os=macos&arch=aarch64&archive_type=zip&java_package_type=jre"
        );
    }

    #[test]
    fn maps_platform_and_arch_names() {
        assert_eq!(platform_name(Platform::Windows), "windows");
        assert_eq!(platform_name(Platform::MacOs), "macos");
        assert_eq!(arch_name(Arch::X64), "x64");
        assert_eq!(arch_name(Arch::X86), "x32");
        assert_eq!(arch_name(Arch::Arm64), "aarch64");
        assert_eq!(arch_name(Arch::Arm), "arm");
    }

    #[test]
    fn parses_package_list() {
        let packages: Vec<Package> = serde_json::from_str(
            r#"[{"availability_type": "CA", "distro_version": [17, 68, 203, 0], "download_url": "https://cdn.azul.com/zulu/bin/zulu17.68.203-ca-jre17.0.20.1-macosx_aarch64.zip", "java_version": [17, 0, 20], "latest": true, "name": "zulu17.68.203-ca-jre17.0.20.1-macosx_aarch64.zip", "openjdk_build_number": 10, "package_uuid": "x", "product": "zulu"}]"#,
        )
        .unwrap();
        assert_eq!(
            packages[0].name,
            "zulu17.68.203-ca-jre17.0.20.1-macosx_aarch64.zip"
        );
        assert_eq!(packages[0].java_version, vec![17, 0, 20]);
    }
}
