use std::time::Duration;

use crate::network::HttpClient;

pub const MIRRORS: [&str; 5] = [
    "https://maven.minecraftforge.net",
    "https://maven.neoforged.net/releases",
    "https://maven.creeperhost.net",
    "https://libraries.minecraft.net",
    "https://repo1.maven.org/maven2",
];

pub async fn check_url(http: &HttpClient, url: &str, timeout: Duration) -> Option<u64> {
    http.head(url, timeout).await
}

pub async fn check_mirror(
    http: &HttpClient,
    base: &str,
    timeout: Duration,
) -> Option<(String, u64)> {
    for mirror in MIRRORS {
        let url = format!("{mirror}/{base}");
        if let Some(size) = check_url(http, &url, timeout).await {
            return Some((url, size));
        }
    }
    None
}
