//! Running `gigapaird` at login (RF-11), per user:
//!
//! - Linux: a systemd user unit (`~/.config/systemd/user/gigapaird.service`).
//!   On a server without a desktop session, `loginctl enable-linger <user>`
//!   keeps it running without a login.
//! - macOS: a LaunchAgent (`~/Library/LaunchAgents/dev.gigapair.gigapaird.plist`).
//! - Windows: an entry under `HKCU\...\CurrentVersion\Run`, which needs no
//!   administrator rights.
//!
//! System-wide variants (Windows Service, dedicated system user) are left
//! for the distribution stage.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dirs::HOME_ENV;

pub const SERVICE_NAME: &str = "gigapaird";
#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "dev.gigapair.gigapaird";
#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "Gigapair";

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("could not find the home folder")]
    NoHome,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("`{cmd}` failed: {output}")]
    Command { cmd: String, output: String },
    #[error("installing as a service is not supported on this system; run gigapaird directly")]
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub installed: bool,
    /// Where the service definition lives.
    pub location: Option<String>,
}

fn run(cmd: &str, args: &[&str]) -> Result<String, ServiceError> {
    let out = Command::new(cmd).args(args).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(ServiceError::Command {
            cmd: format!("{cmd} {}", args.join(" ")),
            output: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

fn home() -> Result<PathBuf, ServiceError> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or(ServiceError::NoHome)
}

/// `GIGAPAIR_HOME` of this process, passed on to the service.
fn custom_home() -> Option<String> {
    std::env::var(HOME_ENV).ok().filter(|v| !v.is_empty())
}

#[cfg(target_os = "linux")]
fn unit_path() -> Result<PathBuf, ServiceError> {
    Ok(directories::BaseDirs::new()
        .ok_or(ServiceError::NoHome)?
        .config_dir()
        .join("systemd/user/gigapaird.service"))
}

/// Text of the systemd user unit.
pub fn systemd_unit(exe: &Path, home: Option<&str>) -> String {
    let env = home
        .map(|h| format!("Environment=\"{HOME_ENV}={h}\"\n"))
        .unwrap_or_default();
    format!(
        "[Unit]\n\
         Description=Gigapair file transfer service\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart=\"{}\"\n\
         {env}Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe.display()
    )
}

/// Text of the macOS LaunchAgent.
pub fn launchd_plist(exe: &Path, home: Option<&str>, log_dir: &Path) -> String {
    let env = home
        .map(|h| {
            format!(
                "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>{HOME_ENV}</key>\n    <string>{}</string>\n  </dict>\n",
                xml_escape(h)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.gigapair.gigapaird</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
{env}  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&exe.display().to_string()),
        xml_escape(&log_dir.join("launchd.log").display().to_string())
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Registers `exe` (the gigapaird binary) to start at login and starts it.
pub fn install(exe: &Path, log_dir: &Path) -> Result<String, ServiceError> {
    let _ = (exe, log_dir, home, run, custom_home);
    #[cfg(target_os = "linux")]
    {
        let path = unit_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, systemd_unit(exe, custom_home().as_deref()))?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run(
            "systemctl",
            &["--user", "enable", "--now", "gigapaird.service"],
        )?;
        return Ok(format!(
            "Installed {}. On a server without a desktop login, also run `loginctl enable-linger $USER` so it runs at boot.",
            path.display()
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let path = home()?.join(format!("Library/LaunchAgents/{LAUNCHD_LABEL}.plist"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(log_dir)?;
        std::fs::write(&path, launchd_plist(exe, custom_home().as_deref(), log_dir))?;
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        // Reinstall cleanly if it was loaded before.
        let _ = run(
            "launchctl",
            &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
        );
        run(
            "launchctl",
            &["bootstrap", &domain, &path.display().to_string()],
        )?;
        return Ok(format!("Installed {}", path.display()));
    }
    #[cfg(windows)]
    {
        let mut cmdline = format!("\"{}\"", exe.display());
        if let Some(h) = custom_home() {
            // The Run key has no environment; pass the folder as a flag.
            cmdline.push_str(&format!(" --home \"{h}\""));
        }
        run(
            "reg",
            &[
                "add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &cmdline, "/f",
            ],
        )?;
        return Ok(format!(
            "Gigapair will start at login ({RUN_KEY}\\{RUN_VALUE})"
        ));
    }
    #[allow(unreachable_code)]
    Err(ServiceError::Unsupported)
}

/// Stops the service manager from starting gigapaird and removes the entry.
pub fn uninstall() -> Result<String, ServiceError> {
    #[cfg(target_os = "linux")]
    {
        let path = unit_path()?;
        let _ = run(
            "systemctl",
            &["--user", "disable", "--now", "gigapaird.service"],
        );
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        return Ok(format!("Removed {}", path.display()));
    }
    #[cfg(target_os = "macos")]
    {
        let path = home()?.join(format!("Library/LaunchAgents/{LAUNCHD_LABEL}.plist"));
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let _ = run(
            "launchctl",
            &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
        );
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(format!("Removed {}", path.display()));
    }
    #[cfg(windows)]
    {
        let _ = run("reg", &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]);
        return Ok("Gigapair will no longer start at login".to_owned());
    }
    #[allow(unreachable_code)]
    Err(ServiceError::Unsupported)
}

/// Starts the installed service through the service manager. Returns
/// false if not installed (the caller then starts gigapaird directly).
pub fn start_managed() -> Result<bool, ServiceError> {
    if !status()?.installed {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    {
        run("systemctl", &["--user", "start", "gigapaird.service"])?;
        return Ok(true);
    }
    #[cfg(target_os = "macos")]
    {
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        run(
            "launchctl",
            &["kickstart", &format!("{domain}/{LAUNCHD_LABEL}")],
        )?;
        return Ok(true);
    }
    #[allow(unreachable_code)]
    Ok(false)
}

/// Stops the service through the service manager, so it isn't restarted.
/// Returns false if not managed (the caller then asks it to shut down).
pub fn stop_managed() -> Result<bool, ServiceError> {
    if !status()?.installed {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    {
        run("systemctl", &["--user", "stop", "gigapaird.service"])?;
        return Ok(true);
    }
    #[allow(unreachable_code)]
    Ok(false)
}

pub fn status() -> Result<ServiceStatus, ServiceError> {
    #[cfg(target_os = "linux")]
    {
        let path = unit_path()?;
        return Ok(ServiceStatus {
            installed: path.exists(),
            location: Some(path.display().to_string()),
        });
    }
    #[cfg(target_os = "macos")]
    {
        let path = home()?.join(format!("Library/LaunchAgents/{LAUNCHD_LABEL}.plist"));
        return Ok(ServiceStatus {
            installed: path.exists(),
            location: Some(path.display().to_string()),
        });
    }
    #[cfg(windows)]
    {
        let installed = run("reg", &["query", RUN_KEY, "/v", RUN_VALUE]).is_ok();
        return Ok(ServiceStatus {
            installed,
            location: Some(format!("{RUN_KEY}\\{RUN_VALUE}")),
        });
    }
    #[allow(unreachable_code)]
    Ok(ServiceStatus {
        installed: false,
        location: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_text() {
        let u = systemd_unit(Path::new("/opt/g/gigapaird"), Some("/srv/g"));
        assert!(u.contains("ExecStart=\"/opt/g/gigapaird\""));
        assert!(u.contains("Environment=\"GIGAPAIR_HOME=/srv/g\""));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn plist_text() {
        let p = launchd_plist(Path::new("/A&B/gigapaird"), None, Path::new("/logs"));
        assert!(p.contains("<string>/A&amp;B/gigapaird</string>"));
        assert!(p.contains("<string>dev.gigapair.gigapaird</string>"));
        assert!(!p.contains("EnvironmentVariables"));
    }
}
