//! The vendor CLI versions Cloaking presents upstream, learned from the CLIs' public
//! registries so they track what a real client would run. See
//! docs/specs/cloaking-versions.md.

use std::cmp::Ordering;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A `major.minor.patch` release version. Only the plain three-part form is accepted:
/// prerelease tags, build metadata, and `v` prefixes are not what the vendors ship on
/// their release channels, so a value that carries them is treated as not a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl FromStr for Version {
    type Err = InvalidVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.').map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(InvalidVersion(s.to_string()));
            }
            part.parse::<u64>()
                .map_err(|_| InvalidVersion(s.to_string()))
        });
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(InvalidVersion(s.to_string()));
        };
        Ok(Self {
            major: major?,
            minor: minor?,
            patch: patch?,
        })
    }
}

impl TryFrom<String> for Version {
    type Error = InvalidVersion;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Version> for String {
    fn from(version: Version) -> Self {
        version.to_string()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidVersion(String);

impl fmt::Display for InvalidVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a major.minor.patch version: {:?}", self.0)
    }
}

impl std::error::Error for InvalidVersion {}

/// The npm registry's `latest` dist-tag for a package document.
#[must_use]
pub fn npm_latest(body: &Value) -> Option<Version> {
    body.get("dist-tags")?.get("latest")?.as_str()?.parse().ok()
}

/// The version behind a GitHub release document for `openai/codex`, whose tags are
/// `rust-v<version>`.
#[must_use]
pub fn codex_release(body: &Value) -> Option<Version> {
    body.get("tag_name")?
        .as_str()?
        .strip_prefix("rust-v")?
        .parse()
        .ok()
}

/// The version Cloaking presents: the newer of the configured value and the fetched
/// one. The configured value is a floor, so a config that pins an old release is never
/// wrong, only redundant. A configured value that is not a version is the operator's
/// explicit choice and is used as written.
#[must_use]
pub fn effective(configured: &str, fetched: Option<&Version>) -> String {
    match (configured.parse::<Version>(), fetched) {
        (Ok(floor), Some(fetched)) => floor.max(*fetched).to_string(),
        _ => configured.to_string(),
    }
}

/// The versions the vendors currently ship, as last fetched. Kept on disk so an offline
/// restart presents what was current at the last successful fetch rather than the baked
/// defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CliVersions {
    pub claude: Option<Version>,
    pub codex: Option<Version>,
}

impl CliVersions {
    /// Read the cache at `path`. A missing or unreadable file is an empty cache: the
    /// loop refetches on its first tick either way.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write the cache atomically: a sibling temp file, then a rename over `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created or the file cannot be
    /// written or renamed.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        use anyhow::Context as _;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to move {} -> {}", tmp.display(), path.display()))
    }
}
