//! Bi-directional Bitfocus Buttons Control API client (`/control/...` with Bearer API key).
//! Allows VideoController to trigger Bitfocus Buttons cuelists or query surface states
//! alongside the first-party Companion/Buttons connection module.

use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use vc_core::BitfocusButtonsConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ButtonsTriggerRequest {
    /// Control API relative endpoint, e.g. `/control/surface/v1/press` or `/control/cuelist/v1/go`
    pub endpoint: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct BitfocusButtonsClient {
    http: Client,
}

impl BitfocusButtonsClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { http }
    }

    pub async fn post_control(
        &self,
        cfg: &BitfocusButtonsConfig,
        req: &ButtonsTriggerRequest,
    ) -> Result<serde_json::Value> {
        if !cfg.enabled {
            return Err(anyhow!("Bitfocus Buttons Control API client is disabled"));
        }
        let base = cfg.base_url.trim_end_matches('/');
        let path = if req.endpoint.starts_with('/') {
            req.endpoint.clone()
        } else {
            format!("/{}", req.endpoint)
        };
        let url = format!("{base}{path}");

        let mut builder = self.http.post(&url).json(&req.payload);
        if !cfg.api_key.trim().is_empty() {
            builder = builder.bearer_auth(cfg.api_key.trim());
        }
        let resp = builder.send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("Bitfocus Buttons API HTTP {status}: {text}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "ok": true, "raw": text })))
    }
}

impl Default for BitfocusButtonsClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn post_control_returns_error_when_disabled() {
        let client = BitfocusButtonsClient::new();
        let cfg = BitfocusButtonsConfig {
            enabled: false,
            ..Default::default()
        };
        let req = ButtonsTriggerRequest {
            endpoint: "/control/cuelist/v1/go".into(),
            payload: serde_json::json!({}),
        };
        let err = client.post_control(&cfg, &req).await.unwrap_err();
        assert!(err.to_string().contains("disabled"));
    }
}
