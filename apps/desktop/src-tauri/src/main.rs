#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;
use std::thread::JoinHandle;

use tauri::Manager;
use tokio::sync::watch;
use tracing::{error, info};

struct EmbeddedDaemon {
    shutdown_tx: watch::Sender<bool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl EmbeddedDaemon {
    fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Ok(mut guard) = self.thread.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vc_daemon=info,vc_media=info,vc_ptz=info,videocontroller_desktop=info".into()),
        )
        .try_init();

    let (shutdown_tx, _) = watch::channel(false);
    let daemon_shutdown_tx = shutdown_tx.clone();

    let app = tauri::Builder::default()
        .setup(move |app| {
            if std::env::var("VC_EXTERNAL_DAEMON").as_deref() == Ok("1") {
                info!("VC_EXTERNAL_DAEMON=1 set; skipping embedded vc-daemon startup");
                return Ok(());
            }

            let mut opts = vc_daemon::DaemonOptions::from_env();
            if opts.web_ui_dir.is_none() {
                if let Ok(res_dir) = app.path().resource_dir() {
                    let web_dir = res_dir.join("web");
                    if web_dir.join("index.html").exists() {
                        opts.web_ui_dir = Some(web_dir);
                    }
                }
            }

            let tx = daemon_shutdown_tx.clone();
            let thread = std::thread::Builder::new()
                .name("vc-daemon-embedded".into())
                .spawn(move || {
                    match tokio::runtime::Builder::new_multi_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => {
                            if let Err(err) = rt.block_on(vc_daemon::run_daemon(opts, tx)) {
                                error!("Embedded vc-daemon exited with error: {err:#}");
                            }
                        }
                        Err(err) => {
                            error!("Failed creating Tokio runtime for embedded vc-daemon: {err}");
                        }
                    }
                })?;

            app.manage(EmbeddedDaemon {
                shutdown_tx: daemon_shutdown_tx,
                thread: Mutex::new(Some(thread)),
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed building VideoController Tauri 2 application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(daemon) = app_handle.try_state::<EmbeddedDaemon>() {
                info!("Shutting down embedded vc-daemon...");
                daemon.shutdown();
            }
        }
    });
}
