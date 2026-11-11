//! `gigapaird`: the background service. Owns the iroh node, the transfer
//! queue and the persisted state, and serves the app and the CLI over
//! local IPC. Closing a client never stops a transfer.

// No console window for the service in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod awake;
mod notify;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use gigapair_core::api::{Event, IPC_VERSION, Request, Response};
use gigapair_core::config::Config;
use gigapair_core::dirs::AppDirs;
use gigapair_core::engine::{Engine, EngineOptions, NetworkMode};
use gigapair_core::ipc::{self, Conn};
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

#[derive(Parser)]
#[command(version, about = "Gigapair background service")]
struct Args {
    /// Keep all data under this folder (same as GIGAPAIR_HOME).
    #[arg(long, value_name = "DIR")]
    home: Option<PathBuf>,
    /// Also log to the terminal.
    #[arg(long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let dirs = match &args.home {
        Some(h) => AppDirs::under(h),
        None => AppDirs::from_env()?,
    };
    dirs.create_all()
        .with_context(|| format!("could not create {}", dirs.data.display()))?;
    let _log_guard = init_logging(&dirs, args.verbose);

    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(run(dirs));
    if let Err(e) = &result {
        tracing::error!("{e:#}");
    }
    result
}

fn init_logging(dirs: &AppDirs, verbose: bool) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("gigapaird")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dirs.logs)
        .unwrap_or_else(|_| tracing_appender::rolling::never(&dirs.logs, "gigapaird.log"));
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_from_env("GIGAPAIR_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,iroh=warn,noq=warn,noq_proto=warn"));
    let file_layer = fmt::layer().with_writer(writer).with_ansi(false);
    let stderr_layer = verbose.then(|| fmt::layer().with_writer(std::io::stderr));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
    guard
}

/// Holds an exclusive lock on the data folder for the life of the
/// process, so two services never share one database. Waits a little for
/// a previous instance that is still shutting down.
async fn lock_instance(dirs: &AppDirs) -> Result<Option<std::fs::File>> {
    let path = dirs.data.join("gigapaird.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    for _ in 0..40 {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("could not lock {}", path.display()));
            }
        }
    }
    Ok(None)
}

async fn run(dirs: AppDirs) -> Result<()> {
    let Some(_lock) = lock_instance(&dirs).await? else {
        eprintln!("gigapaird is already running");
        return Ok(());
    };
    let listener = match ipc::bind(&dirs).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("gigapaird is already running");
            return Ok(());
        }
        Err(e) => return Err(e).context("could not open the local IPC socket"),
    };

    let config = Config::load(&dirs.config_file())?;
    let (key, location) = gigapair_core::keys::load_or_create(&dirs, config.key_storage)?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        key = ?location,
        socket = %ipc::socket_addr(&dirs),
        "starting"
    );

    let engine = Engine::start(EngineOptions {
        dirs: dirs.clone(),
        config,
        secret_key: key,
        network: NetworkMode::Internet,
    })
    .await?;
    eprintln!(
        "gigapaird {} running as {}",
        env!("CARGO_PKG_VERSION"),
        engine.node_id()
    );

    awake::spawn(engine.clone());
    notify::spawn(engine.clone());

    let stop = Arc::new(Notify::new());
    let accept = {
        let engine = engine.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            loop {
                match ipc::accept(&listener).await {
                    Ok(conn) => {
                        let engine = engine.clone();
                        let stop = stop.clone();
                        tokio::spawn(async move {
                            match serve_client(engine, conn, stop).await {
                                Ok(()) => {}
                                Err(e @ ipc::IpcError::Json(_)) => {
                                    tracing::warn!("client message error: {e}")
                                }
                                Err(e) => tracing::debug!("client: {e}"),
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("IPC accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        })
    };

    tokio::select! {
        _ = stop.notified() => tracing::info!("shutdown requested"),
        _ = tokio::signal::ctrl_c() => tracing::info!("interrupted"),
        _ = terminate() => tracing::info!("terminated"),
    }
    // Dropping the listener removes the socket file.
    accept.abort();
    engine.shutdown().await;
    Ok(())
}

#[cfg(unix)]
async fn terminate() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut s) => {
            s.recv().await;
        }
        Err(_) => std::future::pending().await,
    }
}

#[cfg(not(unix))]
async fn terminate() {
    std::future::pending::<()>().await
}

async fn serve_client(
    engine: Engine,
    mut conn: Conn,
    stop: Arc<Notify>,
) -> Result<(), ipc::IpcError> {
    match conn.recv::<Request>().await? {
        Some(Request::Hello { version, client }) => {
            conn.send(&Response::Hello {
                version: IPC_VERSION,
                service_version: env!("CARGO_PKG_VERSION").to_owned(),
            })
            .await?;
            if version != IPC_VERSION {
                return Ok(());
            }
            tracing::debug!("client connected: {client}");
        }
        _ => {
            conn.send(&Response::Error {
                message: "expected hello".into(),
            })
            .await?;
            return Ok(());
        }
    }

    while let Some(req) = conn.recv::<Request>().await? {
        match req {
            Request::Subscribe => {
                // Subscribe before acknowledging so no event is missed.
                let mut events = engine.subscribe();
                conn.send(&Response::Ok).await?;
                return forward_events(&mut conn, &mut events).await;
            }
            Request::Shutdown => {
                conn.send(&Response::Ok).await?;
                stop.notify_one();
                return Ok(());
            }
            Request::Hello { .. } => {
                conn.send(&Response::Error {
                    message: "already connected".into(),
                })
                .await?
            }
            other => {
                let resp = engine.handle(other).await;
                conn.send(&resp).await?;
            }
        }
    }
    Ok(())
}

async fn forward_events(
    conn: &mut Conn,
    events: &mut tokio::sync::broadcast::Receiver<Event>,
) -> Result<(), ipc::IpcError> {
    loop {
        match events.recv().await {
            Ok(ev) => conn.send(&ev).await?,
            // A slow client missed some progress updates; it gets the next.
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return Ok(()),
        }
    }
}
