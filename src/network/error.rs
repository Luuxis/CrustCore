#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to build http client: {0}")]
    Build(#[source] reqwest::Error),
    #[error("http request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("failed to decode response body: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("unexpected status {status} for {url}")]
    Status { status: u16, url: String },
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("download of {url} failed after {attempts} attempts: {source}")]
    Download {
        url: String,
        attempts: u32,
        #[source]
        source: Box<Error>,
    },
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}
