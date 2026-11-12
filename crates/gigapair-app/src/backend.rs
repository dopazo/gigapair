//! Connection to the `gigapaird` service, on its own Tokio thread.
//!
//! The UI sends requests through a channel and gets answers on oneshot
//! channels; service events arrive on another channel. Both reconnect
//! (starting the service if needed) when the service restarts.

use std::time::Duration;

use gigapair_core::api::{Event, Request, Response};
use gigapair_core::dirs::AppDirs;
use gigapair_core::ipc::{self, Client, IpcError};
use tokio::sync::{mpsc, oneshot};

pub enum BackendMsg {
    Event(Box<Event>),
    Connected,
    Disconnected(String),
}

type Job = (Request, oneshot::Sender<Result<Response, String>>);

#[derive(Clone)]
pub struct Backend {
    tx: mpsc::UnboundedSender<Job>,
}

impl Backend {
    pub fn start(dirs: AppDirs) -> (Self, mpsc::UnboundedReceiver<BackendMsg>) {
        let (tx, rx) = mpsc::unbounded_channel::<Job>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("gigapair-ipc".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("could not start the IPC runtime: {e}");
                        let _ = ev_tx.send(BackendMsg::Disconnected(e.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    tokio::spawn(events_loop(dirs.clone(), ev_tx));
                    requests_loop(dirs, rx).await;
                });
            })
            .ok();
        (Self { tx }, ev_rx)
    }

    /// Sends a request; the answer arrives on the returned channel.
    /// Errors are messages ready to show.
    pub fn call(&self, req: Request) -> oneshot::Receiver<Result<Response, String>> {
        let (tx, rx) = oneshot::channel();
        if let Err(mpsc::error::SendError((_, tx))) = self.tx.send((req, tx)) {
            let _ = tx.send(Err("the connection to the service is closed".into()));
        }
        rx
    }
}

async fn requests_loop(dirs: AppDirs, mut rx: mpsc::UnboundedReceiver<Job>) {
    let mut client: Option<Client> = None;
    while let Some((req, reply)) = rx.recv().await {
        let mut result = Err(String::new());
        // One retry with a fresh connection if the service restarted.
        for _attempt in 0..2 {
            if client.is_none() {
                match ipc::connect_or_spawn(&dirs, "app").await {
                    Ok((c, _)) => client = Some(c),
                    Err(e) => {
                        result = Err(e.to_string());
                        break;
                    }
                }
            }
            let Some(c) = client.as_mut() else { break };
            match c.call(req.clone()).await {
                Ok(r) => {
                    result = Ok(r);
                    break;
                }
                Err(IpcError::Service(msg)) => {
                    result = Err(msg);
                    break;
                }
                Err(e) => {
                    client = None;
                    result = Err(e.to_string());
                }
            }
        }
        let _ = reply.send(result);
    }
}

async fn events_loop(dirs: AppDirs, tx: mpsc::UnboundedSender<BackendMsg>) {
    loop {
        match ipc::connect_or_spawn(&dirs, "app-events").await {
            Ok((client, _)) => match client.subscribe().await {
                Ok(mut events) => {
                    if tx.send(BackendMsg::Connected).is_err() {
                        return;
                    }
                    loop {
                        match events.next_event().await {
                            Ok(ev) => {
                                if tx.send(BackendMsg::Event(Box::new(ev))).is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(BackendMsg::Disconnected(e.to_string()));
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(BackendMsg::Disconnected(e.to_string()));
                }
            },
            Err(e) => {
                if tx.send(BackendMsg::Disconnected(e.to_string())).is_err() {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
