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

    #[tokio::test]
    async fn post_control_handles_json_raw_text_and_http_500_from_mock_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            for response in [
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 14\r\n\r\n{\"fired\":true}",
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK",
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 11\r\n\r\nbad api key",
            ] {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
            }
        });

        let client = BitfocusButtonsClient::default();
        let cfg = BitfocusButtonsConfig {
            enabled: true,
            base_url: format!("http://127.0.0.1:{port}/"),
            api_key: "tok-xyz".into(),
        };

        // 1. JSON 200 response with leading slash
        let r1 = client
            .post_control(
                &cfg,
                &ButtonsTriggerRequest {
                    endpoint: "/control/cuelist/v1/go".into(),
                    payload: serde_json::json!({ "id": 1 }),
                },
            )
            .await
            .unwrap();
        assert_eq!(r1["fired"], true);

        // 2. Raw non-JSON 200 response without leading slash
        let r2 = client
            .post_control(
                &cfg,
                &ButtonsTriggerRequest {
                    endpoint: "control/surface/v1/press".into(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(r2["ok"], true);
        assert_eq!(r2["raw"], "OK");

        // 3. HTTP 401 failure path
        let err = client
            .post_control(
                &cfg,
                &ButtonsTriggerRequest {
                    endpoint: "/control/fail".into(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"));

        let _ = server.await;
    }
}
