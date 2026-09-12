use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Error;

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl HttpClient {
    pub fn new() -> Result<Self, Error> {
        let inner = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(Error::Build)?;
        Ok(Self { inner })
    }

    pub fn from_reqwest(inner: reqwest::Client) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &reqwest::Client {
        &self.inner
    }

    pub async fn head(&self, url: &str, timeout: std::time::Duration) -> Option<u64> {
        let response = self.inner.head(url).timeout(timeout).send().await.ok()?;
        if response.status().as_u16() != 200 {
            return None;
        }
        Some(response.content_length().unwrap_or(0))
    }

    pub async fn get(&self, url: &str, bearer: Option<&str>) -> Result<Response, Error> {
        let mut request = self.inner.get(url).header("Accept", "application/json");
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        Self::collect(request).await
    }

    pub async fn post_json<B: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &B,
        bearer: Option<&str>,
    ) -> Result<Response, Error> {
        let mut request = self
            .inner
            .post(url)
            .header("Accept", "application/json")
            .json(body);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        Self::collect(request).await
    }

    pub async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Response, Error> {
        let request = self
            .inner
            .post(url)
            .header("Accept", "application/json")
            .form(form);
        Self::collect(request).await
    }

    async fn collect(request: reqwest::RequestBuilder) -> Result<Response, Error> {
        let response = request.send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        Ok(Response { status, body })
    }
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        Ok(serde_json::from_str(&self.body)?)
    }
}
