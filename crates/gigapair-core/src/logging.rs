//! Local log files, shared by the service and the desktop app.
//!
//! One file per day in [`AppDirs::logs`], named `<prefix>.YYYY-MM-DD.log`;
//! the last 7 days are kept. `GIGAPAIR_LOG` overrides the level (for
//! example `debug`, or `info,gigapair_core=debug`). Logs never leave the
//! computer and never contain keys or pairing codes.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::dirs::AppDirs;

/// Days of logs kept per program.
pub const KEEP_DAYS: usize = 7;

const DEFAULT_FILTER: &str = "info,iroh=warn,noq=warn,noq_proto=warn";

/// Starts logging to a daily file (and to stderr when `stderr`). Keep the
/// returned guard alive until the program ends: dropping it flushes the
/// file. Also records panics in the log (see [`log_panics`]).
pub fn init(dirs: &AppDirs, prefix: &str, stderr: bool) -> WorkerGuard {
    let file = rolling_file(&dirs.logs, prefix);
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter =
        EnvFilter::try_from_env("GIGAPAIR_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let file_layer = fmt::layer().with_writer(writer).with_ansi(false);
    let stderr_layer = stderr.then(|| fmt::layer().with_writer(std::io::stderr));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
    log_panics();
    guard
}

fn rolling_file(dir: &Path, prefix: &str) -> tracing_appender::rolling::RollingFileAppender {
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .max_log_files(KEEP_DAYS)
        .build(dir)
        .unwrap_or_else(|_| tracing_appender::rolling::never(dir, format!("{prefix}.log")))
}

/// Writes panics (with a backtrace) to the log before the default handler
/// runs. Without a console (release builds on Windows) a crash would
/// otherwise leave no trace.
pub fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            "panic: {info}\n{backtrace}"
        );
        default(info);
    }));
}
