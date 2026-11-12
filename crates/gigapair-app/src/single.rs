//! One app per user (and per `GIGAPAIR_HOME`). With the app living in the
//! tray, launching it again must bring the existing window back instead of
//! starting a second copy with a second tray icon.

use std::io::{self, Write};

use gigapair_core::dirs::AppDirs;
use gigapair_core::ipc::{self, SocketAddr};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ListenerOptions, Stream};

use crate::tray::{ActionSender, TrayAction};

pub enum Instance {
    /// This is the only app; call [`Primary::listen`] once the UI is up.
    Primary(Primary),
    /// Another app is running and was asked to show its window.
    Secondary,
}

pub struct Primary {
    listener: Option<interprocess::local_socket::Listener>,
}

/// Claims the app socket, or asks the running app to show itself.
pub fn claim(dirs: &AppDirs) -> Instance {
    let addr = ipc::app_socket_addr(dirs);
    let Ok(name) = addr.to_name() else {
        return Instance::Primary(Primary { listener: None });
    };
    if let Ok(mut stream) = Stream::connect(name.clone()) {
        let _ = stream.write_all(b"open\n");
        return Instance::Secondary;
    }
    // Nobody answered: a socket file left behind is stale.
    if let SocketAddr::Path(p) = &addr {
        let _ = std::fs::remove_file(p);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let listener = match ListenerOptions::new().name(name).create_sync() {
        Ok(l) => Some(l),
        Err(e) => {
            // Not fatal: the app works, it just can't be raised by a
            // second launch.
            tracing::warn!(
                "could not listen on {addr}: {e}; a second launch can't show this window"
            );
            None
        }
    };
    #[cfg(unix)]
    if let SocketAddr::Path(p) = &addr {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }
    Instance::Primary(Primary { listener })
}

impl Primary {
    /// Every later launch sends [`TrayAction::Open`].
    pub fn listen(mut self, actions: ActionSender) -> io::Result<()> {
        let Some(listener) = self.listener.take() else {
            return Ok(());
        };
        std::thread::Builder::new()
            .name("gigapair-single".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if conn.is_ok() && actions.send(TrayAction::Open).is_err() {
                        break;
                    }
                }
            })?;
        Ok(())
    }
}
