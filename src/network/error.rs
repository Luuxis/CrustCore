#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to build http client: {0}")]
    Build(#[source] reqwest::Error),
    #[error("http request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("failed to decode response body: {0}")]
    Decode(#[from] serde_json::Error),
}
