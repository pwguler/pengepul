use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::utils::{generate_api_key, resolve_auth_dir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredProvider {
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutConfig {
    pub messages_ms: u64,
    pub stream_messages_ms: u64,
    pub count_tokens_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloakingConfig {
    pub cli_version: String,
    pub entrypoint: String,
    pub codex: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub auth_dir: PathBuf,
    pub api_keys: HashSet<String>,
    pub body_limit: String,
    pub cloaking: CloakingConfig,
    pub timeouts: TimeoutConfig,
    pub stats_enabled: bool,
    pub debug: DebugMode,
    pub providers: BTreeMap<String, ConfiguredProvider>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugMode {
    Off,
    Errors,
    Verbose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    host: String,
    port: u16,
    #[serde(rename = "auth-dir")]
    auth_dir: String,
    #[serde(rename = "api-keys")]
    api_keys: Vec<String>,
    #[serde(rename = "body-limit")]
    body_limit: String,
    cloaking: RawCloaking,
    timeouts: RawTimeouts,
    stats: RawStats,
    debug: serde_yaml::Value,
    providers: BTreeMap<String, RawConfiguredProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfiguredProvider {
    #[serde(rename = "base-url")]
    base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawCloaking {
    #[serde(rename = "cli-version")]
    cli_version: String,
    entrypoint: String,
    codex: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTimeouts {
    #[serde(rename = "messages-ms")]
    messages: u64,
    #[serde(rename = "stream-messages-ms")]
    stream_messages: u64,
    #[serde(rename = "count-tokens-ms")]
    count_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawStats {
    enabled: bool,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 8317,
            auth_dir: "~/.pengepul".to_string(),
            api_keys: Vec::new(),
            body_limit: "200mb".to_string(),
            cloaking: RawCloaking::default(),
            timeouts: RawTimeouts::default(),
            stats: RawStats::default(),
            debug: serde_yaml::Value::String("off".to_string()),
            providers: BTreeMap::new(),
        }
    }
}

impl Default for RawCloaking {
    fn default() -> Self {
        Self {
            cli_version: "2.1.88".to_string(),
            entrypoint: "cli".to_string(),
            codex: std::collections::BTreeMap::new(),
        }
    }
}

impl Default for RawTimeouts {
    fn default() -> Self {
        Self {
            messages: 120_000,
            stream_messages: 600_000,
            count_tokens: 30_000,
        }
    }
}

impl Default for RawStats {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[must_use]
pub fn default_config_path(home: &Path) -> PathBuf {
    home.join(".pengepul/config.yaml")
}

#[must_use]
pub fn legacy_config_path(cwd: &Path) -> PathBuf {
    cwd.join("config.yaml")
}

#[must_use]
pub fn selected_config_path(
    config_path: Option<&Path>,
    home_override: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    config_paths(config_path, home_override, cwd).1
}

/// Register an OpenAI-compatible provider in the config file `login` read.
///
/// Rewrites the file in place, leaving every other field as it was. The
/// provider must be new: an id already present with a different
/// `base-url` is an error rather than an overwrite, so one mistyped flag
/// cannot move a live provider's traffic to another host
/// (login-registers-a-provider, AC-3). Re-registering the same URL is
/// accepted, so repeating a command is safe.
///
/// # Errors
///
/// Returns an error when the config cannot be read, parsed, or written,
/// when `base_url` is empty, or when `id` is already registered with a
/// different `base-url`.
pub fn register_provider(path: &Path, id: &str, base_url: &str) -> Result<()> {
    // Read-modify-write on one shared file. Two registrations racing each
    // other each read, insert, and write the whole file back, so the
    // slower one silently drops the faster one's provider while both
    // report success and both leave a credential on disk. Verified: eight
    // concurrent registrations left eight pools and one provider.
    //
    // `create_new` is atomic in the OS, so it needs no dependency: the
    // process that creates the lock owns the file until it removes it.
    // Appended, not `with_extension`: that replaces, so `--config x.lock`
    // would lock the operator's own config and then advise deleting it.
    let lock_path = path.with_file_name(format!(
        "{}.lock",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _lock = FileLock::acquire(&lock_path)?;
    // A trailing slash would make `{base_url}/chat/completions` a double
    // slash, which some hosts answer with a 404 that names nothing
    // (AC-5).
    // Trimmed on both sides of the slash strip: `"https://h/v1 /"` would
    // otherwise be stored with a trailing space that `validate_providers`
    // trims on load, so the stored form and the loaded form disagree and
    // repeating the command refuses itself.
    let base_url = base_url.trim().trim_end_matches('/').trim_end();
    if base_url.is_empty() {
        bail!("providers: {id} is missing base-url");
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut raw: RawConfig = if text.trim().is_empty() {
        RawConfig::default()
    } else {
        serde_yaml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))?
    };
    if let Some(existing) = raw.providers.get(id) {
        let existing = existing.base_url.trim().trim_end_matches('/');
        if existing != base_url {
            bail!("{id} already points at {existing}; edit the config to change it");
        }
    }
    raw.providers.insert(
        id.to_string(),
        RawConfiguredProvider {
            base_url: base_url.to_string(),
        },
    );
    // The same validation the load path applies, so a name this file
    // would reject cannot enter through this door (AC-7).
    validate_providers(&raw.providers)?;
    write_config(path, &raw, false)
}

/// Load config from an explicit path, the default home config, or legacy workspace config.
///
/// # Errors
///
/// Returns an error when `HOME` is unavailable, config YAML is invalid, or the generated/migrated
/// config file cannot be written.
pub fn load_config(
    config_path: Option<&Path>,
    home_override: Option<&Path>,
    cwd: &Path,
) -> Result<Config> {
    let home = home_override
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .context("HOME is not set")?;
    let (read_path, write_path) = config_paths(config_path, Some(&home), cwd);
    let mut raw = if read_path.exists() {
        let text = fs::read_to_string(&read_path)
            .with_context(|| format!("failed to read {}", read_path.display()))?;
        if text.trim().is_empty() {
            RawConfig::default()
        } else {
            serde_yaml::from_str::<RawConfig>(&text)
                .with_context(|| format!("invalid config {}", read_path.display()))?
        }
    } else {
        RawConfig::default()
    };

    if raw.api_keys.is_empty() {
        raw.api_keys.push(generate_api_key());
        write_config(&write_path, &raw, config_path.is_none())?;
        println!(
            "\ngenerated API key and saved it to {}:\n\n  {}\n",
            write_path.display(),
            raw.api_keys[0]
        );
    } else if write_path != read_path {
        write_config(&write_path, &raw, true)?;
    }

    let providers = validate_providers(&raw.providers)?;

    Ok(Config {
        host: raw.host,
        port: raw.port,
        auth_dir: resolve_auth_dir(&raw.auth_dir, &home),
        api_keys: raw.api_keys.into_iter().collect(),
        body_limit: raw.body_limit,
        cloaking: CloakingConfig {
            cli_version: raw.cloaking.cli_version,
            entrypoint: raw.cloaking.entrypoint,
            codex: raw.cloaking.codex,
        },
        timeouts: TimeoutConfig {
            messages_ms: raw.timeouts.messages,
            stream_messages_ms: raw.timeouts.stream_messages,
            count_tokens_ms: raw.timeouts.count_tokens,
        },
        stats_enabled: raw.stats.enabled,
        debug: normalize_debug(&raw.debug),
        providers,
    })
}

/// Turn the raw `providers:` section into validated configured providers.
///
/// The entry name becomes the provider id a client's model prefix must match, so
/// Exclusive ownership of a config file for the length of a
/// read-modify-write, released on drop however the write ends.
struct FileLock {
    path: PathBuf,
}

impl FileLock {
    /// Take the lock, waiting briefly for a holder to finish.
    ///
    /// # Errors
    ///
    /// Returns an error when the lock is still held after the wait, which
    /// means another process is registering or one died holding it.
    fn acquire(path: &Path) -> Result<Self> {
        // Short and bounded: this guards a file write, not a network call.
        for _ in 0..50 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => {
                    return Err(anyhow::Error::new(error)
                        .context(format!("failed to lock {}", path.display())));
                }
            }
        }
        bail!(
            "{} is locked by another pengepul; remove it if no other command is running",
            path.display()
        )
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Reject a Provider id the registry cannot hold.
///
/// A caller that writes anything keyed by the id — a credential
/// directory, for instance — must apply this first: `storage_dir()`
/// returns the id verbatim, so a `/` in it escapes the Account's
/// directory and, with `..`, the auth-dir entirely.
///
/// # Errors
///
/// Returns an error when `id` names a built-in Provider or contains `/`.
pub fn validate_provider_id(id: &str) -> Result<()> {
    if matches!(id, "anthropic" | "codex" | "claude") {
        bail!("providers: {id} is a built-in provider name");
    }
    // An allowlist, not a denylist. This id becomes a directory name
    // verbatim (`ProviderId::storage_dir`), and a denylist of separators
    // let `..`, `\`, an empty id, and whitespace through — each one a
    // credential written somewhere the operator did not name. Letters,
    // digits, dot, dash and underscore are what a provider id has ever
    // needed; `.` and `..` are excluded by name because they are legal
    // under that rule and mean something else to a filesystem.
    if id.is_empty() {
        bail!("providers: an id cannot be empty");
    }
    if matches!(id, "." | "..") {
        bail!("providers: {id} is not a usable name");
    }
    if let Some(bad) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        let shown = if bad.is_control() {
            format!("{}", bad.escape_debug())
        } else {
            bad.to_string()
        };
        bail!("providers: {id} must not contain '{shown}'; use letters, digits, '.', '-' or '_'");
    }
    Ok(())
}

/// Turn the raw `providers:` section into validated configured
/// providers.
///
/// Each id passes `validate_provider_id`, and `base-url` is required; the
/// keys for the endpoint live in the auth-dir, not here.
fn validate_providers(
    raw: &BTreeMap<String, RawConfiguredProvider>,
) -> Result<BTreeMap<String, ConfiguredProvider>> {
    let mut providers = BTreeMap::new();
    for (id, entry) in raw {
        validate_provider_id(id)?;
        if entry.base_url.trim().is_empty() {
            bail!("providers: {id} is missing base-url");
        }
        providers.insert(
            id.clone(),
            ConfiguredProvider {
                base_url: entry.base_url.trim().to_string(),
            },
        );
    }
    Ok(providers)
}

fn config_paths(
    config_path: Option<&Path>,
    home_override: Option<&Path>,
    cwd: &Path,
) -> (PathBuf, PathBuf) {
    if let Some(path) = config_path {
        let expanded = expand_path(path, home_override);
        return (expanded.clone(), expanded);
    }

    let home = home_override
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let default_path = default_config_path(&home);
    if default_path.exists() {
        return (default_path.clone(), default_path);
    }

    let legacy_path = legacy_config_path(cwd);
    if legacy_path.exists() {
        return (legacy_path, default_path);
    }

    (default_path.clone(), default_path)
}

fn expand_path(path: &Path, home: Option<&Path>) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return home.map_or_else(|| PathBuf::from("~"), Path::to_path_buf);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.map_or_else(|| path.to_path_buf(), |home| home.join(rest));
    }
    path.to_path_buf()
}

fn normalize_debug(value: &serde_yaml::Value) -> DebugMode {
    match value {
        serde_yaml::Value::Bool(true) => DebugMode::Errors,
        serde_yaml::Value::String(value) if value == "errors" => DebugMode::Errors,
        serde_yaml::Value::String(value) if value == "verbose" => DebugMode::Verbose,
        _ => DebugMode::Off,
    }
}

fn write_config(path: &Path, raw: &RawConfig, private_parent: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        if private_parent {
            set_mode(parent, 0o700)?;
        }
    } else {
        bail!("config path has no parent: {}", path.display());
    }

    let text = serde_yaml::to_string(raw).context("failed to encode config YAML")?;
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    set_mode(path, 0o600)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to chmod {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod id_tests {
    use super::validate_provider_id;

    /// The id becomes a directory name verbatim, so the rule is an
    /// allowlist. A denylist of `/` let every one of these through, each
    /// writing a credential somewhere the operator did not name — found
    /// by probing the built binary, not by the suite.
    #[test]
    fn no_id_can_escape_its_own_directory() {
        for id in [
            "",
            " ",
            ".",
            "..",
            "/",
            "//",
            "/etc/pengepul",
            "\\",
            "..\\..\\x",
            "groq ",
            " groq",
            "groq/../groq",
            "a\nb",
            "x\tb",
            "a\0b",
            "üñïçø∂é",
        ] {
            assert!(
                validate_provider_id(id).is_err(),
                "an id that cannot be a directory name was accepted: {id:?}"
            );
        }
    }

    /// And the ids an operator actually types still work.
    #[test]
    fn ordinary_ids_are_accepted() {
        for id in [
            "openrouter",
            "groq",
            "open-router",
            "open_router",
            "openrouter2",
            "open.router",
            "GROQ",
        ] {
            validate_provider_id(id).unwrap_or_else(|error| {
                panic!("a usable id was refused: {id:?}: {error:#}");
            });
        }
    }

    /// A built-in is refused by name, not by shape.
    #[test]
    fn built_in_names_stay_refused() {
        for id in ["anthropic", "codex", "claude"] {
            assert!(validate_provider_id(id).is_err(), "{id} was accepted");
        }
    }
}

#[cfg(test)]
mod register_tests {
    use super::register_provider;
    use std::fs;

    /// The conflict rule lives here as well as in `login`, and `login`
    /// short-circuits it on every integration path — so without this
    /// test, deleting the check below breaks nothing. It is the backstop
    /// for the window between a caller's read and this write.
    #[test]
    fn a_known_id_with_a_different_url_is_refused_here_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            "host: \"127.0.0.1\"\nport: 8317\napi-keys:\n  - sk-test\nproviders:\n  groq:\n    base-url: https://api.groq.com/openai/v1\n",
        )
        .expect("write config");

        let error = register_provider(&path, "groq", "https://elsewhere.host/v1")
            .expect_err("a conflicting URL was accepted");

        assert!(
            format!("{error:#}").contains("https://api.groq.com/openai/v1"),
            "the error does not name the URL it kept: {error:#}"
        );
        let after = fs::read_to_string(&path).expect("read config");
        assert!(
            after.contains("https://api.groq.com/openai/v1") && !after.contains("elsewhere.host"),
            "the live URL was overwritten: {after}"
        );
    }

    /// Read-modify-write on one shared file loses everything but the last
    /// writer: eight concurrent registrations left eight credentials on
    /// disk and one provider in the config, every command reporting
    /// success. Threads here rather than processes, which is the same
    /// race through the same lock.
    #[test]
    fn concurrent_registrations_do_not_lose_each_other() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            "host: \"127.0.0.1\"\nport: 8317\napi-keys:\n  - sk-test\nproviders:\n  base:\n    base-url: https://base/v1\n",
        )
        .expect("write config");

        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = path.clone();
                scope.spawn(move || {
                    register_provider(&path, &format!("p{index}"), &format!("https://h{index}/v1"))
                        .expect("register");
                });
            }
        });

        let after = fs::read_to_string(&path).expect("read config");
        for index in 0..8 {
            assert!(
                after.contains(&format!("p{index}:")),
                "p{index} was lost to a concurrent registration: {after}"
            );
        }
        assert!(after.contains("base:"), "the original provider was lost");
        assert!(
            !path.with_extension("lock").exists(),
            "the lock outlived the registration"
        );
    }

    /// `with_extension` replaces rather than appends, so a config named
    /// `x.lock` derived a lock path identical to itself: the tool locked
    /// the operator's own config and then advised deleting it.
    #[test]
    fn a_config_named_lock_is_not_its_own_lock_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("x.lock");
        fs::write(
            &path,
            "host: \"127.0.0.1\"\nport: 8317\napi-keys:\n  - sk-test\nproviders: {}\n",
        )
        .expect("write config");

        register_provider(&path, "groq", "https://api.groq.com/openai/v1").expect("register");

        let after = fs::read_to_string(&path).expect("the config was consumed as a lock");
        assert!(
            after.contains("groq:"),
            "the provider was not written: {after}"
        );
        assert!(
            !dir.path().join("x.lock.lock").exists(),
            "the lock outlived the registration"
        );
    }

    /// The same URL is not a conflict, so a caller repeating itself is
    /// safe.
    #[test]
    fn the_same_url_is_accepted_here_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        fs::write(
            &path,
            "host: \"127.0.0.1\"\nport: 8317\napi-keys:\n  - sk-test\nproviders:\n  groq:\n    base-url: https://api.groq.com/openai/v1\n",
        )
        .expect("write config");

        register_provider(&path, "groq", "https://api.groq.com/openai/v1/").expect("same URL");

        let after = fs::read_to_string(&path).expect("read config");
        assert!(after.contains("https://api.groq.com/openai/v1"), "{after}");
    }
}
