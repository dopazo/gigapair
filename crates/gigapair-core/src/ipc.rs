//! Local IPC transport: a Unix socket on macOS and Linux, a named pipe on
//! Windows. Messages are newline-delimited JSON. The first exchange is a
//! version handshake, so a client and service from different versions fail
//! with a clear message.

use std::io;
use std::path::PathBuf;

use interprocess::local_socket::tokio::{Stream, prelude::*};
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::api::{Event, IPC_VERSION, Request, Response};
use crate::dirs::AppDirs;

pub use interprocess::local_socket::tokio::Listener;

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error(
        "the Gigapair service is not running. Start it with `gigapair service start` or run `gigapaird`"
    )]
    NotRunning,
    #[error("IPC error: {0}")]
    Io(#[from] io::Error),
    #[error("malformed message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the service closed the connection")]
    Closed,
    #[error(
        "the running Gigapair service (v{service}) is not compatible with this client; restart the service so both use the same version"
    )]
    Incompatible { service: String },
    #[error("{0}")]
    Service(String),
    #[error("unexpected reply from the service")]
    Unexpected,
}

/// Where the service listens.
#[derive(Debug, Clone)]
pub enum SocketAddr {
    Path(PathBuf),
    Pipe(String),
}

impl std::fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(p) => write!(f, "{}", p.display()),
            Self::Pipe(n) => write!(f, r"\\.\pipe\{n}"),
        }
    }
}

/// The socket address for this set of folders. Each `GIGAPAIR_HOME` gets
/// its own, so instances don't collide.
pub fn socket_addr(dirs: &AppDirs) -> SocketAddr {
    if cfg!(windows) {
        SocketAddr::Pipe(format!("gigapair-{}", dirs.instance_tag()))
    } else {
        let path = dirs.data.join("gigapaird.sock");
        // Unix socket paths are limited to ~104 bytes.
        if path.as_os_str().len() < 100 {
            SocketAddr::Path(path)
        } else {
            SocketAddr::Path(
                std::env::temp_dir().join(format!("gigapair-{}.sock", dirs.instance_tag())),
            )
        }
    }
}

fn to_name(addr: &SocketAddr) -> io::Result<Name<'static>> {
    match addr {
        SocketAddr::Path(p) => p.clone().to_fs_name::<GenericFilePath>(),
        SocketAddr::Pipe(n) => n.clone().to_ns_name::<GenericNamespaced>(),
    }
}

/// Binds the service socket. Fails with `AddrInUse` if another service is
/// already listening there; removes a stale socket file left by a crash.
pub async fn bind(dirs: &AppDirs) -> io::Result<Listener> {
    let addr = socket_addr(dirs);
    if let SocketAddr::Path(p) = &addr {
        if p.exists() {
            if Stream::connect(to_name(&addr)?).await.is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another Gigapair service is already running",
                ));
            }
            let _ = std::fs::remove_file(p);
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let listener = ListenerOptions::new()
        .name(to_name(&addr)?)
        .create_tokio()?;
    #[cfg(unix)]
    if let SocketAddr::Path(p) = &addr {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(listener)
}

/// Waits for the next client.
pub async fn accept(listener: &Listener) -> io::Result<Conn> {
    use interprocess::local_socket::traits::tokio::Listener as _;
    Ok(Conn::new(listener.accept().await?))
}

/// One side of a line-delimited JSON connection.
pub struct Conn {
    stream: BufReader<Stream>,
    line: String,
}

impl Conn {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream: BufReader::new(stream),
            line: String::new(),
        }
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        let mut bytes = serde_json::to_vec(msg)?;
        bytes.push(b'\n');
        let w = self.stream.get_mut();
        w.write_all(&bytes).await?;
        w.flush().await?;
        Ok(())
    }

    /// Next message, or `None` when the other side closed the connection.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, IpcError> {
        self.line.clear();
        let n = self.stream.read_line(&mut self.line).await?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(self.line.trim_end())?))
    }
}

/// A client connection to the service.
pub struct Client {
    conn: Conn,
    pub service_version: String,
}

impl Client {
    /// Connects and performs the version handshake.
    pub async fn connect(dirs: &AppDirs, client_name: &str) -> Result<Self, IpcError> {
        let addr = socket_addr(dirs);
        let stream = match Stream::connect(to_name(&addr)?).await {
            Ok(s) => s,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Err(IpcError::NotRunning);
            }
            Err(e) => return Err(e.into()),
        };
        let mut conn = Conn::new(stream);
        conn.send(&Request::Hello {
            version: IPC_VERSION,
            client: client_name.to_owned(),
        })
        .await?;
        // Parse the greeting loosely so a service from another version
        // (different message layout) still yields a clear error.
        let Some(raw) = conn.recv::<serde_json::Value>().await? else {
            return Err(IpcError::Closed);
        };
        let body = raw.get("data").unwrap_or(&raw);
        let field = |k: &str| body.get(k).or_else(|| raw.get(k));
        let service_version = field("service_version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();
        match (raw.get("type").and_then(|t| t.as_str()), field("version")) {
            (Some("hello"), Some(v)) if v.as_u64() == Some(IPC_VERSION as u64) => {
                if serde_json::from_value::<Response>(raw.clone()).is_err() {
                    return Err(IpcError::Incompatible {
                        service: service_version,
                    });
                }
                Ok(Self {
                    conn,
                    service_version,
                })
            }
            (Some("hello"), _) => Err(IpcError::Incompatible {
                service: service_version,
            }),
            (Some("error"), _) => Err(IpcError::Service(
                field("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("error")
                    .to_owned(),
            )),
            _ => Err(IpcError::Unexpected),
        }
    }

    /// Sends a request and waits for its response. Service-side errors
    /// become [`IpcError::Service`].
    pub async fn call(&mut self, req: Request) -> Result<Response, IpcError> {
        self.conn.send(&req).await?;
        match self.conn.recv::<Response>().await? {
            Some(Response::Error { message }) => Err(IpcError::Service(message)),
            Some(r) => Ok(r),
            None => Err(IpcError::Closed),
        }
    }

    /// Subscribes to events; use [`Self::next_event`] afterwards.
    pub async fn subscribe(mut self) -> Result<EventStream, IpcError> {
        self.call(Request::Subscribe).await?;
        Ok(EventStream { conn: self.conn })
    }
}

pub struct EventStream {
    conn: Conn,
}

impl EventStream {
    /// Next event; `Err(Closed)` when the service stops.
    pub async fn next_event(&mut self) -> Result<Event, IpcError> {
        self.conn.recv::<Event>().await?.ok_or(IpcError::Closed)
    }
}

/// Path of a sibling executable (e.g. `gigapaird` next to `gigapair`).
pub fn sibling_exe(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let candidate = dir.join(&file);
    if candidate.exists() {
        return Some(candidate);
    }
    // `cargo test` puts test binaries in target/<profile>/deps.
    let parent = dir.parent()?.join(&file);
    parent.exists().then_some(parent)
}

/// Starts `gigapaird` in the background, detached from this process.
/// With custom folders (`GIGAPAIR_HOME`), the service gets the same ones.
pub fn spawn_service(dirs: &AppDirs) -> io::Result<()> {
    let exe = sibling_exe("gigapaird").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "gigapaird was not found next to this program; build it with `cargo build` or install Gigapair again",
        )
    })?;
    let mut cmd = std::process::Command::new(exe);
    if dirs.custom_root {
        if let Some(root) = dirs.data.parent() {
            cmd.arg("--home").arg(root);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New session: the service survives the terminal closing.
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}

/// Connects to the service, starting it first if it isn't running.
/// Returns whether it had to be started.
pub async fn connect_or_spawn(
    dirs: &AppDirs,
    client_name: &str,
) -> Result<(Client, bool), IpcError> {
    match Client::connect(dirs, client_name).await {
        Ok(c) => return Ok((c, false)),
        Err(IpcError::NotRunning) => {}
        Err(e) => return Err(e),
    }
    spawn_service(dirs)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        match Client::connect(dirs, client_name).await {
            Ok(c) => return Ok((c, true)),
            Err(IpcError::NotRunning) if tokio::time::Instant::now() < deadline => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handshake_and_call() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(dir.path());
        std::fs::create_dir_all(&dirs.data).unwrap();
        let listener = bind(&dirs).await.unwrap();

        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let mut conn = Conn::new(stream);
            let hello: Request = conn.recv().await.unwrap().unwrap();
            assert!(matches!(
                hello,
                Request::Hello {
                    version: IPC_VERSION,
                    ..
                }
            ));
            conn.send(&Response::Hello {
                version: IPC_VERSION,
                service_version: "test".into(),
            })
            .await
            .unwrap();
            let req: Request = conn.recv().await.unwrap().unwrap();
            assert_eq!(req, Request::Contacts);
            conn.send(&Response::Error {
                message: "nope".into(),
            })
            .await
            .unwrap();
        });

        let mut client = Client::connect(&dirs, "test").await.unwrap();
        assert_eq!(client.service_version, "test");
        let err = client.call(Request::Contacts).await.unwrap_err();
        assert!(matches!(err, IpcError::Service(m) if m == "nope"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn not_running() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AppDirs::under(dir.path());
        assert!(matches!(
            Client::connect(&dirs, "test").await,
            Err(IpcError::NotRunning)
        ));
    }
}
