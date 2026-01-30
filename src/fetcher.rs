use std::time::Duration;

use anyhow::{Context as _, anyhow};
use bytes::Bytes;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use tokio::sync::Semaphore;
use url::Url;

#[derive(Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    semaphore: std::sync::Arc<Semaphore>,
}

impl Fetcher {
    pub fn new(user_agent: &str, max_concurrency: usize) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            client,
            semaphore: std::sync::Arc::new(Semaphore::new(max_concurrency.max(1))),
        })
    }

    pub async fn get_bytes(&self, url: Url) -> anyhow::Result<(Bytes, HeaderMap)> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .context("acquire download permit")?;

        let mut backoff = Duration::from_millis(250);
        let max_attempts = 5usize;

        for attempt in 1..=max_attempts {
            let resp = self
                .client
                .get(url.clone())
                .send()
                .await
                .with_context(|| format!("GET {}", url))?;

            let status = resp.status();
            let headers = resp.headers().clone();

            if status.is_success() {
                let bytes = resp.bytes().await.context("read response body")?;
                return Ok((bytes, headers));
            }

            if status.as_u16() == 429 || status.as_u16() == 503 {
                let wait = retry_after_duration(&headers).unwrap_or(backoff);
                tracing::warn!(
                    %status,
                    attempt,
                    wait_ms = wait.as_millis(),
                    "throttled; backing off"
                );
                tokio::time::sleep(wait).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }

            return Err(anyhow!("GET {} failed with status {}", url, status));
        }

        Err(anyhow!("GET {} failed after retries", url))
    }
}

fn retry_after_duration(headers: &HeaderMap) -> Option<Duration> {
    let v = headers.get(RETRY_AFTER)?;
    let s = v.to_str().ok()?.trim();
    let seconds: u64 = s.parse().ok()?;
    Some(Duration::from_secs(seconds))
}
