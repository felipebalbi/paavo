//! Thin HTTP client around the paavod surface.

use anyhow::{Context, Result};
use paavo_proto::{BoardSpec, JobSpec};
use serde::de::DeserializeOwned;

/// HTTP client.
pub struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    /// Construct.
    pub fn new(base: String) -> Self {
        Self {
            base,
            http: reqwest::Client::new(),
        }
    }

    /// Submit a job. Returns the new `job_id` string.
    pub async fn submit_job(&self, spec: &JobSpec, tar_bytes: Vec<u8>) -> Result<String> {
        let meta = serde_json::to_string(spec)?;
        let form = reqwest::multipart::Form::new()
            .part(
                "metadata",
                reqwest::multipart::Part::bytes(meta.into_bytes()).mime_str("application/json")?,
            )
            .part(
                "crate",
                reqwest::multipart::Part::bytes(tar_bytes)
                    .file_name("crate.tar")
                    .mime_str("application/octet-stream")?,
            );
        let resp = self
            .http
            .post(format!("{}/jobs", self.base))
            .multipart(form)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.text().await.unwrap_or_default());
        }
        #[derive(serde::Deserialize)]
        struct Body {
            job_id: String,
        }
        let body: Body = resp.json().await?;
        Ok(body.job_id)
    }

    /// GET helper.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.text().await.unwrap_or_default());
        }
        let val = resp.json().await?;
        Ok(val)
    }

    /// POST with optional JSON body.
    pub async fn post_json<B: serde::Serialize>(&self, path: &str, body: Option<&B>) -> Result<()> {
        let mut req = self.http.post(format!("{}{}", self.base, path));
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.text().await.unwrap_or_default());
        }
        Ok(())
    }

    /// Stream `GET /jobs/:id/stream` as an NDJSON response. Callers
    /// read `resp.chunk()` and split on `\n`.
    pub async fn stream(&self, job_id: &str) -> Result<reqwest::Response> {
        let resp = self
            .http
            .get(format!("{}/jobs/{}/stream", self.base, job_id))
            .send()
            .await
            .with_context(|| "GET /jobs/:id/stream")?;
        if !resp.status().is_success() {
            anyhow::bail!("paavod: {}", resp.status());
        }
        Ok(resp)
    }

    /// Add a board.
    pub async fn add_board(&self, spec: &BoardSpec) -> Result<()> {
        self.post_json("/boards", Some(spec)).await
    }
}
