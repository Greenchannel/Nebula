//! Minimal pure-Rust HTTP client (hyper 1 + hyper-util).
//!
//! Nebula only ever talks plain HTTP on the LAN, so no TLS stack is needed.
//! Avoids pulling OpenSSL / ring / native-tls into the build, which keeps
//! Windows-GNU and Android cross-compilation painless.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct HttpClient {
    inner: Client<HttpConnector, Full<Bytes>>,
    timeout: Duration,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").field("timeout", &self.timeout).finish()
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

impl HttpClient {
    pub fn new(timeout: Duration) -> Self {
        Self {
            inner: Client::builder(TokioExecutor::new()).build(HttpConnector::new()),
            timeout,
        }
    }

    pub async fn post_json(&self, url: &str, body: &Value) -> Result<Value> {
        let payload = Bytes::from(serde_json::to_vec(body)?);
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(url)
            .header(CONTENT_TYPE, "application/json");
        req = req.header("x-nebula-proto", "NEBULA_V1");
        let req = req.body(Full::new(payload)).context("building request")?;
        let (status, bytes, _mime) = self.send(req).await?;
        if !status.is_success() {
            return Err(anyhow!(
                "POST {url} failed with HTTP {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&bytes[..bytes.len().min(300)])
            ));
        }
        let value = serde_json::from_slice(&bytes)
            .with_context(|| format!("decoding JSON response from {url}"))?;
        Ok(value)
    }

    pub async fn get_bytes(&self, url: &str) -> Result<(String, Vec<u8>)> {
        let req = Request::builder()
            .method(Method::GET)
            .uri(url)
            .header("x-nebula-proto", "NEBULA_V1")
            .body(Full::new(Bytes::new()))
            .context("building request")?;
        let (status, bytes, mime) = self.send(req).await?;
        if !status.is_success() {
            return Err(anyhow!("GET {url} failed with HTTP {}", status.as_u16()));
        }
        Ok((mime, bytes.to_vec()))
    }

    async fn send(
        &self,
        req: Request<Full<Bytes>>,
    ) -> Result<(StatusCode, Bytes, String)> {
        let fut = async {
            let resp = self.inner.request(req).await.context("http request")?;
            let status = resp.status();
            let mime = resp
                .headers()
                .get(CONTENT_TYPE)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"))
                .to_str()
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = resp
                .into_body()
                .collect()
                .await
                .context("reading response body")?
                .to_bytes();
            Ok::<_, anyhow::Error>((status, bytes, mime))
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| anyhow!("http request timed out after {:?}", self.timeout))?
    }
}
