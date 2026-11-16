//! "Check for updates": asks GitHub which Gigapair releases exist and
//! whether one is newer than this version.
//!
//! Only when the user asks (Settings → Check for updates, `gigapair
//! check-update`): besides contacts and relays, Gigapair never contacts anything
//! on its own (no telemetry). The request carries no data about this
//! device beyond what any HTTPS request does (the IP address and a
//! `gigapair/<version>` user agent). Nothing is downloaded or installed.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// This program's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// `https://github.com/<owner>/<repo>`, from the workspace manifest.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The result of a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// The version that answered (the service's; every Gigapair program is
    /// built from one workspace version).
    pub current: String,
    /// The newest release after `current`, if any.
    pub newer: Option<Release>,
    /// The page that lists every release.
    pub releases_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// Without the tag's `v`, e.g. `0.1.0-beta.5`.
    pub version: String,
    /// The release's page, with its notes and downloads.
    pub url: String,
    pub prerelease: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(
        "could not reach GitHub to check for updates ({0}); check the internet connection and try again"
    )]
    Network(String),
    #[error("GitHub answered {status} when asked for releases; try again later or see {url}")]
    Status { status: u16, url: String },
    #[error("could not read GitHub's list of releases ({error}); try again later or see {url}")]
    Parse { error: String, url: String },
}

/// One entry of GitHub's `GET /repos/{owner}/{repo}/releases`.
#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub fn releases_url() -> String {
    format!("{REPOSITORY}/releases")
}

/// Asks GitHub for the releases and compares them with [`CURRENT`].
pub async fn check() -> Result<UpdateInfo, UpdateError> {
    let body = fetch_releases().await?;
    let releases: Vec<GhRelease> = serde_json::from_str(&body).map_err(|e| UpdateError::Parse {
        error: e.to_string(),
        url: releases_url(),
    })?;
    let newer = pick_newer(CURRENT, &releases);
    tracing::info!(
        current = CURRENT,
        newer = newer.as_ref().map(|r| r.version.as_str()),
        "checked for updates"
    );
    Ok(UpdateInfo {
        current: CURRENT.to_owned(),
        newer,
        releases_url: releases_url(),
    })
}

async fn fetch_releases() -> Result<String, UpdateError> {
    // The same TLS stack iroh already uses (rustls with ring and the
    // Mozilla roots), set up here rather than through a process-wide
    // default provider.
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| UpdateError::Network(e.to_string()))?
    .with_root_certificates(rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    })
    .with_no_client_auth();
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .user_agent(concat!("gigapair/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(network)?;
    let response = client
        .get(api_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(network)?;
    let status = response.status();
    if !status.is_success() {
        return Err(UpdateError::Status {
            status: status.as_u16(),
            url: releases_url(),
        });
    }
    response.text().await.map_err(network)
}

fn api_url() -> String {
    let repo = REPOSITORY
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/');
    format!("https://api.github.com/repos/{repo}/releases?per_page=30")
}

/// reqwest's own message is only the outermost error ("error sending
/// request"); the cause (DNS, TLS, timeout) is in its sources.
fn network(e: reqwest::Error) -> UpdateError {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    UpdateError::Network(msg)
}

/// The newest published release after `current`. Someone on a stable
/// version is only told about stable releases; someone on a pre-release
/// (a beta) about both.
fn pick_newer(current: &str, releases: &[GhRelease]) -> Option<Release> {
    let current = semver::Version::parse(current).ok()?;
    let stable_only = current.pre.is_empty();
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = semver::Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            let prerelease = r.prerelease || !version.pre.is_empty();
            (version > current && !(stable_only && prerelease)).then_some((version, r, prerelease))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(version, r, prerelease)| Release {
            version: version.to_string(),
            url: r.html_url.clone(),
            prerelease,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, prerelease: bool, draft: bool) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            html_url: format!("https://example.com/{tag}"),
            draft,
            prerelease,
        }
    }

    fn newer(current: &str, list: &[GhRelease]) -> Option<String> {
        pick_newer(current, list).map(|r| r.version)
    }

    #[test]
    fn newest_beta_for_a_beta() {
        let list = [
            rel("v0.1.0-beta.5", true, false),
            rel("v0.1.0-beta.4", true, false),
            rel("v0.1.0-beta.10", true, false),
        ];
        // Numeric pre-release parts compare as numbers: 10 > 5.
        assert_eq!(newer("0.1.0-beta.4", &list), Some("0.1.0-beta.10".into()));
    }

    #[test]
    fn nothing_newer() {
        let list = [
            rel("v0.1.0-beta.4", true, false),
            rel("v0.1.0-beta.3", true, false),
        ];
        assert_eq!(newer("0.1.0-beta.4", &list), None);
    }

    #[test]
    fn stable_after_beta() {
        let list = [
            rel("v0.1.0", false, false),
            rel("v0.1.0-beta.5", true, false),
        ];
        assert_eq!(newer("0.1.0-beta.4", &list), Some("0.1.0".into()));
    }

    #[test]
    fn stable_users_skip_prereleases() {
        let list = [
            rel("v0.3.0-beta.1", true, false),
            rel("v0.2.1", false, false),
            // Marked as a pre-release on GitHub without a semver suffix.
            rel("v0.4.0", true, false),
        ];
        assert_eq!(newer("0.2.0", &list), Some("0.2.1".into()));
    }

    #[test]
    fn drafts_and_odd_tags_are_ignored() {
        let list = [
            rel("v9.0.0", false, true),
            rel("nightly", false, false),
            rel("0.2.0", false, false),
        ];
        assert_eq!(newer("0.1.0", &list), Some("0.2.0".into()));
    }

    #[test]
    fn reads_githubs_json() {
        let body = r#"[{"tag_name":"v0.1.0-beta.5","html_url":"https://github.com/o/r/releases/tag/v0.1.0-beta.5","draft":false,"prerelease":true,"assets":[]}]"#;
        let list: Vec<GhRelease> = serde_json::from_str(body).unwrap();
        let r = pick_newer("0.1.0-beta.4", &list).unwrap();
        assert_eq!(r.version, "0.1.0-beta.5");
        assert!(r.prerelease);
        assert!(r.url.ends_with("v0.1.0-beta.5"));
    }

    #[test]
    fn api_url_comes_from_the_repository() {
        assert!(api_url().starts_with("https://api.github.com/repos/"));
        assert!(api_url().ends_with("/releases?per_page=30"));
        assert!(!api_url().contains("github.com/repos/https"));
    }
}
