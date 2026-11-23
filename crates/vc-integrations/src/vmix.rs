//! vMix Live Broadcast Switcher Integration:
//! - Persistent TCP API (default port 8099) with `SUBSCRIBE TALLY` & `SUBSCRIBE ACTS`
//! - HTTP XML API (default port 8088) for input/scene enumeration & instant switching
//!   (`Cut`, `Fade`, `Stinger1..4`, `PreviewInput`, `ActiveInput`, `OverlayInput1..4`)

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{watch, RwLock};
use tracing::{debug, info, warn};
use vc_core::{TallyState, VmixConfig};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VmixInputInfo {
    pub key: String,
    pub number: u32,
    pub kind: String,
    pub title: String,
    pub short_title: String,
    pub state: String,
    pub tally: TallyState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VmixRuntimeState {
    pub enabled: bool,
    pub connected: bool,
    pub host: String,
    pub http_port: u16,
    pub tcp_port: u16,
    pub version: Option<String>,
    pub active_input: Option<u32>,
    pub preview_input: Option<u32>,
    pub recording: bool,
    pub streaming: bool,
    pub tally_string: String,
    pub inputs: Vec<VmixInputInfo>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VmixSwitchCommand {
    /// vMix shortcut function: Cut, Fade, Stinger1..4, PreviewInput, ActiveInput, OverlayInput1..4, etc.
    pub function: String,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u32>,
    #[serde(default)]
    pub mix: Option<u8>,
}

/// Parse a vMix TCP `TALLY OK 001020...` line into per-input `TallyState` values (1-indexed input = vec idx + 1).
pub fn parse_vmix_tally_line(line: &str) -> Option<Vec<TallyState>> {
    let trimmed = line.trim();
    let payload = trimmed.strip_prefix("TALLY OK ")?;
    let mut states = Vec::with_capacity(payload.len());
    for ch in payload.chars() {
        let s = match ch {
            '1' => TallyState::Program,
            '2' => TallyState::Preview,
            _ => TallyState::Off,
        };
        states.push(s);
    }
    Some(states)
}

/// Parse the vMix HTTP XML API document (`GET http://<host>:8088/api/`).
pub fn parse_vmix_xml_state(xml: &str, cfg: &VmixConfig) -> Result<VmixRuntimeState> {
    let doc = roxmltree::Document::parse(xml).context("invalid vMix XML payload")?;
    let root = doc.root_element();

    let text_of = |tag: &str| -> Option<String> {
        root.children()
            .find(|n| n.tag_name().name() == tag)
            .and_then(|n| n.text())
            .map(|s| s.trim().to_string())
    };

    let version = text_of("version");
    let active_input = text_of("active").and_then(|s| s.parse::<u32>().ok());
    let preview_input = text_of("preview").and_then(|s| s.parse::<u32>().ok());
    let recording = text_of("recording").map(|s| s.eq_ignore_ascii_case("true")).unwrap_or(false);
    let streaming = text_of("streaming").map(|s| s.eq_ignore_ascii_case("true")).unwrap_or(false);

    let mut inputs = Vec::new();
    if let Some(inputs_node) = root.children().find(|n| n.tag_name().name() == "inputs") {
        for node in inputs_node.children().filter(|n| n.tag_name().name() == "input") {
            let number = node
                .attribute("number")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let key = node.attribute("key").unwrap_or("").to_string();
            let kind = node.attribute("type").unwrap_or("").to_string();
            let title = node.attribute("title").unwrap_or("").to_string();
            let short_title = node
                .attribute("shortTitle")
                .unwrap_or(&title)
                .to_string();
            let state = node.attribute("state").unwrap_or("").to_string();
            let tally = if active_input == Some(number) {
                TallyState::Program
            } else if preview_input == Some(number) {
                TallyState::Preview
            } else {
                TallyState::Off
            };
            inputs.push(VmixInputInfo {
                key,
                number,
                kind,
                title,
                short_title,
                state,
                tally,
            });
        }
    }

    let tally_string: String = inputs
        .iter()
        .map(|i| match i.tally {
            TallyState::Program => '1',
            TallyState::Preview => '2',
            TallyState::Off => '0',
        })
        .collect();

    Ok(VmixRuntimeState {
        enabled: cfg.enabled,
        connected: true,
        host: cfg.host.clone(),
        http_port: cfg.http_port,
        tcp_port: cfg.tcp_port,
        version,
        active_input,
        preview_input,
        recording,
        streaming,
        tally_string,
        inputs,
        last_error: None,
    })
}

/// Resolve a camera's `vmix.input_ref` (input number, title, short title, or GUID) against live vMix state.
pub fn resolve_camera_tally(input_ref: &str, state: &VmixRuntimeState) -> TallyState {
    if !state.connected || input_ref.trim().is_empty() {
        return TallyState::Off;
    }
    let q = input_ref.trim();

    if let Ok(num) = q.parse::<u32>() {
        if let Some(inp) = state.inputs.iter().find(|i| i.number == num) {
            return inp.tally;
        }
        if let Some(ch) = state.tally_string.chars().nth(num.saturating_sub(1) as usize) {
            return match ch {
                '1' => TallyState::Program,
                '2' => TallyState::Preview,
                _ => TallyState::Off,
            };
        }
    }

    state
        .inputs
        .iter()
        .find(|i| {
            i.key.eq_ignore_ascii_case(q)
                || i.title.eq_ignore_ascii_case(q)
                || i.short_title.eq_ignore_ascii_case(q)
        })
        .map(|i| i.tally)
        .unwrap_or(TallyState::Off)
}

#[derive(Clone)]
pub struct VmixClient {
    config: Arc<RwLock<VmixConfig>>,
    http: Client,
    state_tx: watch::Sender<VmixRuntimeState>,
    state_rx: watch::Receiver<VmixRuntimeState>,
}

impl VmixClient {
    pub fn new(initial_config: VmixConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| Client::new());
        let init_state = VmixRuntimeState {
            enabled: initial_config.enabled,
            host: initial_config.host.clone(),
            http_port: initial_config.http_port,
            tcp_port: initial_config.tcp_port,
            ..Default::default()
        };
        let (state_tx, state_rx) = watch::channel(init_state);
        Self {
            config: Arc::new(RwLock::new(initial_config)),
            http,
            state_tx,
            state_rx,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<VmixRuntimeState> {
        self.state_rx.clone()
    }

    pub fn current_state(&self) -> VmixRuntimeState {
        self.state_rx.borrow().clone()
    }

    pub async fn update_config(&self, cfg: VmixConfig) {
        *self.config.write().await = cfg.clone();
        let mut st = self.current_state();
        st.enabled = cfg.enabled;
        st.host = cfg.host;
        st.http_port = cfg.http_port;
        st.tcp_port = cfg.tcp_port;
        if !cfg.enabled {
            st.connected = false;
            st.last_error = None;
        }
        let _ = self.state_tx.send(st);
    }

    /// Fetch full XML state from vMix HTTP API and update watch state.
    pub async fn refresh_xml_state(&self) -> Result<VmixRuntimeState> {
        let cfg = self.config.read().await.clone();
        let url = format!("http://{}:{}/api/", cfg.host, cfg.http_port);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed connecting to vMix HTTP API at {url}"))?;
        let xml = resp.text().await?;
        let parsed = parse_vmix_xml_state(&xml, &cfg)?;
        let _ = self.state_tx.send(parsed.clone());
        Ok(parsed)
    }

    /// Execute any vMix shortcut function (Cut, Fade, Stinger1, PreviewInput, ActiveInput, etc.).
    pub async fn execute(&self, cmd: &VmixSwitchCommand) -> Result<()> {
        let cfg = self.config.read().await.clone();
        if !cfg.enabled {
            return Err(anyhow!("vMix integration is currently disabled"));
        }
        let mut params: Vec<(&str, String)> = vec![("Function", cmd.function.clone())];
        if let Some(ref input) = cmd.input {
            params.push(("Input", input.clone()));
        }
        if let Some(ref value) = cmd.value {
            params.push(("Value", value.clone()));
        }
        if let Some(dur) = cmd.duration_ms {
            params.push(("Duration", dur.to_string()));
        }
        if let Some(mix) = cmd.mix {
            params.push(("Mix", mix.to_string()));
        }

        let url = format!("http://{}:{}/api/", cfg.host, cfg.http_port);
        let resp = self.http.get(&url).query(&params).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("vMix returned HTTP {}", resp.status()));
        }
        // Best-effort quick refresh after scene switch
        let _ = self.refresh_xml_state().await;
        Ok(())
    }

    /// Spawn the resilient background TCP Tally (`:8099`) & XML metadata synchronizer task.
    pub fn spawn_background_sync(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let cfg = self.config.read().await.clone();
                if !cfg.enabled {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    continue;
                }

                let tcp_addr = format!("{}:{}", cfg.host, cfg.tcp_port);
                match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&tcp_addr)).await {
                    Ok(Ok(stream)) => {
                        info!(addr = %tcp_addr, "Connected to vMix TCP Tally API");
                        let xml_refresher = self.clone();
                        tokio::spawn(async move {
                            let _ = xml_refresher.refresh_xml_state().await;
                        });
                        if let Err(err) = self.run_tcp_session(stream, &cfg).await {
                            warn!(addr = %tcp_addr, "vMix TCP session ended: {err}");
                        }
                    }
                    Ok(Err(err)) => {
                        self.mark_disconnected(&cfg, format!("TCP connect failed: {err}"));
                    }
                    Err(_) => {
                        self.mark_disconnected(&cfg, "TCP connect timed out".to_string());
                    }
                }

                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
    }

    fn mark_disconnected(&self, cfg: &VmixConfig, err: String) {
        let mut st = self.current_state();
        st.enabled = cfg.enabled;
        st.connected = false;
        st.last_error = Some(err);
        let _ = self.state_tx.send(st);
    }

    async fn run_tcp_session(&self, stream: TcpStream, initial_cfg: &VmixConfig) -> Result<()> {
        let (read_half, mut write_half) = stream.into_split();
        write_half.write_all(b"SUBSCRIBE TALLY\r\nSUBSCRIBE ACTS\r\n").await?;

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        let mut xml_poll = tokio::time::interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                _ = xml_poll.tick() => {
                    let cur = self.config.read().await.clone();
                    if !cur.enabled || cur.host != initial_cfg.host || cur.tcp_port != initial_cfg.tcp_port {
                        return Ok(());
                    }
                    let _ = self.refresh_xml_state().await;
                }
                res = reader.read_line(&mut line) => {
                    let bytes = res?;
                    if bytes == 0 {
                        return Err(anyhow!("vMix TCP socket closed"));
                    }
                    let trimmed = line.trim().to_string();
                    line.clear();

                    if let Some(tallies) = parse_vmix_tally_line(&trimmed) {
                        debug!(count = tallies.len(), "Received live vMix TALLY update");
                        let mut st = self.current_state();
                        st.connected = true;
                        st.last_error = None;
                        st.tally_string = trimmed.strip_prefix("TALLY OK ").unwrap_or("").to_string();
                        for (idx, t) in tallies.iter().enumerate() {
                            let num = (idx + 1) as u32;
                            if *t == TallyState::Program {
                                st.active_input = Some(num);
                            } else if *t == TallyState::Preview {
                                st.preview_input = Some(num);
                            }
                            if let Some(inp) = st.inputs.iter_mut().find(|i| i.number == num) {
                                inp.tally = *t;
                            }
                        }
                        let _ = self.state_tx.send(st);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn parses_vmix_tally_line_and_xml_state() {
        let tallies = parse_vmix_tally_line("TALLY OK 0120\r\n").unwrap();
        assert_eq!(
            tallies,
            vec![TallyState::Off, TallyState::Program, TallyState::Preview, TallyState::Off]
        );

        let xml = r#"<vmix>
          <version>28.0.0.42</version>
          <inputs>
            <input key="aaa-111" number="1" type="NDI" title="Stage Wide" shortTitle="Wide" state="Running" />
            <input key="bbb-222" number="2" type="NDI" title="Pulpit Follow" shortTitle="Pulpit" state="Running" />
          </inputs>
          <preview>1</preview>
          <active>2</active>
          <recording>True</recording>
          <streaming>False</streaming>
        </vmix>"#;

        let cfg = VmixConfig {
            enabled: true,
            ..Default::default()
        };
        let st = parse_vmix_xml_state(xml, &cfg).unwrap();
        assert_eq!(st.version.as_deref(), Some("28.0.0.42"));
        assert_eq!(st.active_input, Some(2));
        assert_eq!(st.preview_input, Some(1));
        assert!(st.recording);
        assert_eq!(resolve_camera_tally("2", &st), TallyState::Program);
        assert_eq!(resolve_camera_tally("Stage Wide", &st), TallyState::Preview);
        assert_eq!(resolve_camera_tally("bbb-222", &st), TallyState::Program);
    }

    #[tokio::test]
    async fn tcp_tally_subscriber_receives_live_updates_from_mock_vmix() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 256];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let _ = sock.write_all(b"TALLY OK 210\r\n").await;
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let client = VmixClient::new(VmixConfig {
            enabled: true,
            host: "127.0.0.1".into(),
            http_port: 1, // unused in this TCP test
            tcp_port: port,
        });
        let mut rx = client.subscribe();
        let handle = client.clone().spawn_background_sync();

        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                rx.changed().await.ok();
                let st = rx.borrow().clone();
                if st.tally_string == "210" {
                    return st;
                }
            }
        })
        .await
        .expect("should receive live TALLY OK 210 from mock vMix server");

        assert_eq!(received.active_input, Some(2));
        assert_eq!(received.preview_input, Some(1));
        handle.abort();
    }
}
