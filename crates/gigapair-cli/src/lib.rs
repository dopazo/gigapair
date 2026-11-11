//! The `gigapair` command line. A thin client: every command is a request
//! to the `gigapaird` service over local IPC (RF-10).

mod output;
mod strings;

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use gigapair_core::api::{Event, RELAY_NOTICE, Request, Response};
use gigapair_core::dirs::AppDirs;
use gigapair_core::ipc::{self, Client, IpcError};
use gigapair_core::service;
use gigapair_core::types::{PendingPair, TransferInfo, TransferStatus, now_secs, short_id};

use crate::output::{print_contacts, print_transfers, progress_line, render_qr};
use crate::strings as s;

#[derive(Parser)]
#[command(
    name = "gigapair",
    version,
    about = "Move huge files between the devices you trust. Pair once, send anytime."
)]
struct Cli {
    /// Keep all data under this folder (same as GIGAPAIR_HOME). Useful to
    /// run two instances on one computer.
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show a pairing code, or add a contact from the other device's code.
    Pair {
        /// The other device's pairing code. Without it, shows this
        /// device's code and waits for the other device.
        code: Option<String>,
        /// Name for the new contact (asked if not given).
        #[arg(long)]
        name: Option<String>,
        /// Don't ask to confirm the fingerprint (scripts).
        #[arg(long)]
        yes: bool,
    },
    /// List contacts, or rename, remove or trust one.
    Contacts {
        #[command(subcommand)]
        action: Option<ContactsCmd>,
    },
    /// Send files or folders to a contact and show the progress.
    Send {
        contact: String,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Return right away instead of showing the progress.
        #[arg(long)]
        no_wait: bool,
    },
    /// Incoming transfers waiting for an answer.
    Requests,
    /// Accept an incoming transfer.
    Accept {
        id: String,
        /// Destination folder (default: the download folder).
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
    },
    /// Reject an incoming transfer.
    Reject { id: String },
    /// Active transfers, speed and connection type.
    Status {
        /// Keep refreshing until Ctrl+C.
        #[arg(long)]
        watch: bool,
    },
    /// Finished transfers.
    History {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Delete the history.
        #[arg(long)]
        clear: bool,
    },
    /// Pause a transfer.
    Pause { id: String },
    /// Resume a paused transfer.
    Resume { id: String },
    /// Cancel a transfer.
    Cancel { id: String },
    /// Send a failed or cancelled transfer again.
    Retry { id: String },
    /// Read or change settings.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Manage the background service.
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

#[derive(Subcommand)]
enum ContactsCmd {
    /// List contacts (default).
    List,
    Rename {
        contact: String,
        name: String,
    },
    Remove {
        contact: String,
    },
    /// Accept transfers from this contact without asking (e.g. a server).
    Trust {
        contact: String,
        #[arg(long, value_enum)]
        auto_accept: OnOff,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show one setting, or all of them.
    Get { key: Option<String> },
    /// Change a setting. An empty value resets optional settings.
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Start gigapaird at login and start it now.
    Install,
    /// Stop starting gigapaird at login.
    Uninstall,
    Start,
    Stop,
    Status,
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let dirs = match &cli.home {
        Some(h) => AppDirs::under(h),
        None => AppDirs::from_env()?,
    };
    if let Command::Service { action } = &cli.command {
        return service_cmd(&dirs, action).await;
    }
    let mut client = connect(&dirs).await?;
    match cli.command {
        Command::Pair { code, name, yes } => pair(&dirs, &mut client, code, name, yes).await,
        Command::Contacts { action } => contacts(&mut client, action).await,
        Command::Send {
            contact,
            paths,
            no_wait,
        } => send(&dirs, &mut client, contact, paths, no_wait).await,
        Command::Requests => {
            let list = transfers(client.call(Request::Requests).await?)?;
            if list.is_empty() {
                println!("{}", s::NO_REQUESTS);
            } else {
                print_transfers(&list);
                println!("\n{}", s::ACCEPT_HINT);
            }
            Ok(())
        }
        Command::Accept { id, to } => {
            let dest_dir = to.map(absolute).transpose()?;
            let t = transfer(
                client
                    .call(Request::Accept {
                        id,
                        dest_dir: dest_dir.map(|p| p.display().to_string()),
                    })
                    .await?,
            )?;
            println!(
                "{} {} → {}",
                s::ACCEPTED,
                t.label,
                t.dest_dir.unwrap_or_default()
            );
            Ok(())
        }
        Command::Reject { id } => {
            let t = transfer(client.call(Request::Reject { id }).await?)?;
            println!("{} {}", s::REJECTED, t.label);
            Ok(())
        }
        Command::Status { watch } => status(&dirs, &mut client, watch).await,
        Command::History { limit, clear } => {
            if clear {
                client.call(Request::ClearHistory).await?;
                println!("{}", s::HISTORY_CLEARED);
                return Ok(());
            }
            let list = transfers(client.call(Request::History { limit }).await?)?;
            if list.is_empty() {
                println!("{}", s::NO_HISTORY);
            } else {
                print_transfers(&list);
            }
            Ok(())
        }
        Command::Pause { id } => simple(&mut client, Request::Pause { id }).await,
        Command::Resume { id } => simple(&mut client, Request::Resume { id }).await,
        Command::Cancel { id } => simple(&mut client, Request::Cancel { id }).await,
        Command::Retry { id } => match client.call(Request::Retry { id }).await? {
            Response::Sent { transfer, warnings } => {
                print_warnings(&warnings);
                println!("{} {}", s::SENDING_AGAIN, short_id(&transfer.id));
                Ok(())
            }
            Response::Transfer(t) => {
                println!("{}: {}", t.label, t.status.label());
                Ok(())
            }
            other => Err(unexpected(other)),
        },
        Command::Config { action } => config(&mut client, action).await,
        Command::Service { .. } => unreachable_service(),
    }
}

fn unreachable_service() -> Result<()> {
    Err(anyhow!("internal error"))
}

async fn connect(dirs: &AppDirs) -> Result<Client> {
    match ipc::connect_or_spawn(dirs, "cli").await {
        Ok((client, started)) => {
            if started {
                eprintln!("{}", s::SERVICE_STARTED);
            }
            Ok(client)
        }
        Err(e) => Err(anyhow!(e)),
    }
}

fn unexpected(r: Response) -> anyhow::Error {
    anyhow!("unexpected reply from the service: {r:?}")
}

fn transfer(r: Response) -> Result<TransferInfo> {
    match r {
        Response::Transfer(t) => Ok(t),
        other => Err(unexpected(other)),
    }
}

fn transfers(r: Response) -> Result<Vec<TransferInfo>> {
    match r {
        Response::Transfers(t) => Ok(t),
        other => Err(unexpected(other)),
    }
}

async fn simple(client: &mut Client, req: Request) -> Result<()> {
    let t = transfer(client.call(req).await?)?;
    println!("{}: {}", t.label, t.status.label());
    Ok(())
}

fn absolute(p: PathBuf) -> Result<PathBuf> {
    if p.is_absolute() {
        return Ok(p);
    }
    Ok(std::env::current_dir()?.join(p))
}

fn print_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("warning: {w}");
    }
}

/// Reads one line from the terminal.
async fn prompt(question: &str) -> Result<String> {
    print!("{question}");
    std::io::stdout().flush()?;
    let line = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map(|_| line)
    })
    .await??;
    Ok(line.trim().to_owned())
}

async fn confirm_pair(
    client: &mut Client,
    pending: PendingPair,
    name: Option<String>,
    yes: bool,
) -> Result<()> {
    println!("\n{}", s::FINGERPRINT_INTRO);
    println!("\n    {}\n", pending.fingerprint.to_uppercase());
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("{}", s::NEED_TTY_OR_YES);
        }
        let answer = prompt(s::FINGERPRINT_QUESTION).await?;
        if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
            client
                .call(Request::PairReject {
                    id: pending.id.clone(),
                })
                .await?;
            println!("{}", s::PAIR_CANCELLED);
            return Ok(());
        }
    }
    let name = match name {
        Some(n) => n,
        None if yes || !std::io::stdin().is_terminal() => pending.remote_name.clone(),
        None => {
            let n = prompt(&s::name_question(&pending.remote_name)).await?;
            if n.is_empty() {
                pending.remote_name.clone()
            } else {
                n
            }
        }
    };
    match client
        .call(Request::PairConfirm {
            id: pending.id,
            name,
        })
        .await?
    {
        Response::Contact(c) => {
            println!("{}", s::paired_with(&c.name));
            Ok(())
        }
        other => Err(unexpected(other)),
    }
}

async fn pair(
    dirs: &AppDirs,
    client: &mut Client,
    code: Option<String>,
    name: Option<String>,
    yes: bool,
) -> Result<()> {
    if let Some(code) = code {
        println!("{}", s::CONNECTING);
        let pending = match client.call(Request::PairJoin { code }).await? {
            Response::PendingPair(p) => p,
            other => return Err(unexpected(other)),
        };
        println!("{}", s::connected_to(&pending.remote_name));
        return confirm_pair(client, pending, name, yes).await;
    }

    // Subscribe first so the request isn't missed.
    let mut events = Client::connect(dirs, "cli-events")
        .await?
        .subscribe()
        .await?;
    let (code, expires_at) = match client.call(Request::PairCreate).await? {
        Response::PairCode { code, expires_at } => (code, expires_at),
        other => return Err(unexpected(other)),
    };
    println!("{}\n", s::PAIR_CODE_INTRO);
    if let Some(qr) = render_qr(&code) {
        println!("{qr}");
    }
    println!("    {code}\n");
    println!("{}", s::PAIR_WAITING);

    let wait = Duration::from_secs((expires_at - now_secs()).max(1) as u64);
    let pending = tokio::time::timeout(wait, async {
        loop {
            match events.next_event().await {
                Ok(Event::PairRequest(p)) if p.incoming => return Ok(p),
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .map_err(|_| anyhow!("{}", s::CODE_EXPIRED))??;
    println!("{}", s::pair_request_from(&pending.remote_name));
    confirm_pair(client, pending, name, yes).await
}

async fn contacts(client: &mut Client, action: Option<ContactsCmd>) -> Result<()> {
    match action.unwrap_or(ContactsCmd::List) {
        ContactsCmd::List => {
            let Response::Contacts(list) = client.call(Request::Contacts).await? else {
                bail!("unexpected reply");
            };
            if list.is_empty() {
                println!("{}", s::NO_CONTACTS);
            } else {
                print_contacts(&list);
            }
        }
        ContactsCmd::Rename { contact, name } => {
            client
                .call(Request::ContactRename {
                    contact,
                    name: name.clone(),
                })
                .await?;
            println!("{} {name}", s::RENAMED);
        }
        ContactsCmd::Remove { contact } => {
            client
                .call(Request::ContactRemove {
                    contact: contact.clone(),
                })
                .await?;
            println!("{} {contact}", s::REMOVED);
        }
        ContactsCmd::Trust {
            contact,
            auto_accept,
        } => {
            let on = matches!(auto_accept, OnOff::On);
            client
                .call(Request::ContactTrust {
                    contact: contact.clone(),
                    auto_accept: on,
                })
                .await?;
            println!("{}", s::auto_accept_set(&contact, on));
        }
    }
    Ok(())
}

async fn send(
    dirs: &AppDirs,
    client: &mut Client,
    contact: String,
    paths: Vec<PathBuf>,
    no_wait: bool,
) -> Result<()> {
    let paths = paths
        .into_iter()
        .map(|p| absolute(p).map(|p| p.display().to_string()))
        .collect::<Result<Vec<_>>>()?;
    // Subscribe first so no update is missed.
    let events = if no_wait {
        None
    } else {
        Some(
            Client::connect(dirs, "cli-events")
                .await?
                .subscribe()
                .await?,
        )
    };
    let (t, warnings) = match client.call(Request::Send { contact, paths }).await? {
        Response::Sent { transfer, warnings } => (transfer, warnings),
        other => return Err(unexpected(other)),
    };
    print_warnings(&warnings);
    println!(
        "{}",
        s::sending(&t.label, &t.peer_name, t.total_bytes, short_id(&t.id))
    );
    let Some(mut events) = events else {
        return Ok(());
    };
    println!("{}", s::DETACH_HINT);
    let mut last_len: usize = 0;
    let mut current = t;
    loop {
        let line = progress_line(&current);
        print!(
            "\r{line}{}",
            " ".repeat(last_len.saturating_sub(line.chars().count()))
        );
        last_len = line.chars().count();
        std::io::stdout().flush()?;
        if current.status.is_terminal() {
            println!();
            return match current.status {
                TransferStatus::Completed => Ok(()),
                _ => Err(anyhow!(
                    "{}",
                    current
                        .error
                        .unwrap_or_else(|| current.status.label().to_owned())
                )),
            };
        }
        match events.next_event().await {
            Ok(Event::Transfer(t)) if t.id == current.id => current = t,
            Ok(Event::RelayFirstUse { .. }) => {
                println!("\n\n{RELAY_NOTICE}\n");
            }
            Ok(_) => {}
            Err(IpcError::Closed) => bail!("{}", s::SERVICE_STOPPED),
            Err(e) => return Err(e.into()),
        }
    }
}

async fn status(dirs: &AppDirs, client: &mut Client, watch: bool) -> Result<()> {
    let Response::Status(info) = client.call(Request::Status).await? else {
        bail!("unexpected reply");
    };
    let header = s::status_header(&info);
    if !watch {
        println!("{header}\n");
        let list = transfers(client.call(Request::Transfers).await?)?;
        if list.is_empty() {
            println!("{}", s::NO_ACTIVE);
        } else {
            print_transfers(&list);
        }
        return Ok(());
    }
    let mut events = Client::connect(dirs, "cli-events")
        .await?
        .subscribe()
        .await?;
    loop {
        let list = transfers(client.call(Request::Transfers).await?)?;
        // Clear the screen and redraw.
        print!("\x1b[2J\x1b[H{header}\n\n");
        if list.is_empty() {
            println!("{}", s::NO_ACTIVE);
        } else {
            print_transfers(&list);
        }
        println!("\n{}", s::WATCH_HINT);
        std::io::stdout().flush()?;
        // Redraw on the next event, at most every half second.
        tokio::select! {
            r = events.next_event() => { r?; }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn config(client: &mut Client, action: ConfigCmd) -> Result<()> {
    match action {
        ConfigCmd::Get { key } => {
            let Response::Config(entries) = client.call(Request::ConfigGet { key }).await? else {
                bail!("unexpected reply");
            };
            let width = entries.iter().map(|e| e.key.len()).max().unwrap_or(0);
            for e in entries {
                println!(
                    "{:width$}  {}",
                    e.key,
                    e.value.unwrap_or_else(|| s::UNSET.to_owned())
                );
            }
        }
        ConfigCmd::Set { key, value } => {
            client
                .call(Request::ConfigSet {
                    key: key.clone(),
                    value,
                })
                .await?;
            println!("{} {key}", s::SAVED);
        }
    }
    Ok(())
}

async fn service_cmd(dirs: &AppDirs, action: &ServiceCmd) -> Result<()> {
    let running = || async { Client::connect(dirs, "cli").await.is_ok() };
    match action {
        ServiceCmd::Install => {
            let exe =
                ipc::sibling_exe("gigapaird").ok_or_else(|| anyhow!("{}", s::NO_DAEMON_EXE))?;
            // A copy started by hand would hold the socket.
            if running().await {
                if let Ok(mut c) = Client::connect(dirs, "cli").await {
                    let _ = c.call(Request::Shutdown).await;
                    tokio::time::sleep(Duration::from_millis(800)).await;
                }
            }
            let msg = service::install(&exe, &dirs.logs).context(s::INSTALL_FAILED)?;
            println!("{msg}");
            if !running().await {
                ipc::spawn_service(dirs)?;
            }
        }
        ServiceCmd::Uninstall => {
            println!("{}", service::uninstall()?);
        }
        ServiceCmd::Start => {
            if running().await {
                println!("{}", s::ALREADY_RUNNING);
                return Ok(());
            }
            if !service::start_managed()? {
                ipc::spawn_service(dirs)?;
            }
            for _ in 0..60 {
                if running().await {
                    println!("{}", s::STARTED);
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            bail!("{}", s::START_TIMEOUT);
        }
        ServiceCmd::Stop => {
            if !service::stop_managed()? {
                match Client::connect(dirs, "cli").await {
                    Ok(mut c) => {
                        c.call(Request::Shutdown).await?;
                    }
                    Err(IpcError::NotRunning) => {
                        println!("{}", s::NOT_RUNNING);
                        return Ok(());
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            println!("{}", s::STOPPED);
        }
        ServiceCmd::Status => {
            let st = service::status()?;
            println!(
                "{}",
                s::service_status(st.installed, st.location.as_deref(), running().await)
            );
        }
    }
    Ok(())
}
