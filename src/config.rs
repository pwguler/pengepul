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
    pub body_limit: BodyLimit,
    pub cloaking: CloakingConfig,
    pub timeouts: TimeoutConfig,
    pub stats_enabled: bool,
    pub debug: DebugMode,
    pub providers: BTreeMap<String, ConfiguredProvider>,
}

/// The largest request body the relay will read, resolved once at config load.
///
/// There is no `Invalid` arm: an unparseable `body-limit` is refused by `load_config`
/// rather than carried into the request path, because a running relay has no sane answer
/// for "the operator's limit did not parse".
///
/// Both the body-reading layer and the relay's own check are given this value, so the two
/// cannot be given *different* limits. They are not interchangeable, though, and the
/// difference is what the layer was added for: the extractor reads the body first, so an
/// honest oversized request is answered there, under axum's message rather than the relay's.
/// That leaves the handler's check running only on requests already read — and over HTTP a
/// declared length above the limit cannot reach it, because the extractor answers when the
/// bytes arrive or the read fails first. It is a fallback, not the answer a client sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLimit {
    Unlimited,
    Limited(u64),
}

/// The `body-limit` an omitted setting falls back to. An *empty* value is not this: it
/// parses to [`BodyLimit::Unlimited`], which is what the README documents.
pub const DEFAULT_BODY_LIMIT_BYTES: u64 = 200 * 1024 * 1024;

/// Parse a `body-limit` value: a byte count with an optional `b`/`kb`/`mb`/`gb` suffix, a
/// bare number meaning bytes, or empty meaning unlimited.
///
/// This is an error rather than a silent fallback on purpose. The setting exists to choose
/// one number, and the failure mode of guessing is that the operator's `200mb` quietly
/// becomes the layer's own default while every panel still reports the configured value.
///
/// # Errors
///
/// Returns an error when the value is not a byte count, or when its suffix would overflow
/// a `u64`.
pub fn parse_body_limit(value: &str) -> Result<BodyLimit> {
    let raw = value.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Ok(BodyLimit::Unlimited);
    }
    let (number, multiplier) = match [
        ("gb", 1024_u64 * 1024 * 1024),
        ("mb", 1024_u64 * 1024),
        ("kb", 1024_u64),
        ("b", 1_u64),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| raw.strip_suffix(suffix).map(|n| (n, multiplier)))
    {
        Some((number, multiplier)) => (number.trim(), multiplier),
        // No suffix: a bare number is bytes, which is what `10b` already means.
        None => (raw.as_str(), 1),
    };
    let bytes = number
        .parse::<u64>()
        .ok()
        .and_then(|parsed| parsed.checked_mul(multiplier))
        .ok_or_else(|| anyhow::anyhow!("not a byte count"))?;
    Ok(BodyLimit::Limited(bytes))
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
pub fn register_provider(path: &Path, id: &str, base_url: &str) -> Result<String> {
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
    let base_url = normalize_base_url(base_url);
    // Before the file is read. `validate_providers` below would catch an
    // empty URL with the same message, but only after parsing the config
    // and inserting the entry: refusing here keeps a bad argument from
    // touching the operator's file at all.
    if base_url.is_empty() {
        bail!("providers: {id} is missing base-url");
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    // Returned so a caller that must undo this restores what the file held
    // *under this lock*. Reading it outside would capture a racing
    // writer's absence and erase their registration on undo.
    let before = text.clone();
    let mut raw: RawConfig = if text.trim().is_empty() {
        RawConfig::default()
    } else {
        serde_yaml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))?
    };
    if let Some(existing) = raw.providers.get(id) {
        let existing = normalize_base_url(&existing.base_url);
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
    write_config(path, &raw, false)?;
    Ok(before)
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
        body_limit: parse_body_limit(&raw.body_limit)
            .with_context(|| format!("body-limit: {:?}", raw.body_limit))?,
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

/// The stored form of a `base-url`: what two callers must agree on.
///
/// A trailing slash would make `{base_url}/chat/completions` a double
/// slash, which some hosts answer with a 404 that names nothing; the
/// second trim catches a space *before* that slash, which would otherwise
/// be stored and then trimmed again on load, so the stored and loaded
/// forms disagree and repeating a command refuses itself.
///
/// This exists as a function because it did not: `login` and
/// `register_provider` each carried their own copy, they drifted by one
/// call, and the guard that runs first was the one missing it.
///
/// One pass over a set, rather than a chain of trims, so the result is a
/// fixed point: `normalize(normalize(x)) == normalize(x)` for every
/// input. A chain is not — `.trim().trim_end_matches('/').trim_end()`
/// leaves the slash in `https://h/v1/ /`, and the stored form then
/// disagrees with the guard that compares against it.
#[must_use]
pub fn normalize_base_url(url: &str) -> &str {
    url.trim()
        .trim_end_matches(|c: char| c == '/' || c.is_whitespace())
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
    // Temp file plus rename, as `usage.json` is written. `fs::write`
    // truncates first, so a concurrent reader sees an empty file, decides
    // `api-keys` is missing, generates a fresh key and writes a whole new
    // config over everything — verified: eight concurrent registrations
    // left four, each having silently replaced the file.
    //
    // A rename replaces the file rather than writing through it, so a
    // read-only config would be silently replaced and its mode reset.
    // Refuse first: an operator who chmods a file means it.
    if let Ok(existing) = fs::metadata(path)
        && existing.permissions().readonly()
    {
        bail!("failed to write {}: Permission denied", path.display());
    }
    let temp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::write(&temp, text).with_context(|| format!("failed to write {}", temp.display()))?;
    fs::rename(&temp, path)
        .with_context(|| format!("failed to move {} into place", temp.display()))?;
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
            !dir.path().join("config.yaml.lock").exists(),
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

    /// An empty URL is refused before the config is read, not after it is
    /// parsed and the entry inserted. `validate_providers` catches it
    /// too, with the same message — so the message cannot tell the two
    /// apart, and only the untouched file can.
    #[test]
    fn an_empty_url_is_refused_without_reading_the_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        // Not valid YAML: reaching the parser at all is a failure, and it
        // would report `invalid config` rather than the missing URL.
        fs::write(&path, "{not yaml at all").expect("write config");

        let error = register_provider(&path, "groq", "   ").expect_err("an empty URL was accepted");

        let error = format!("{error:#}");
        assert!(
            error.contains("missing base-url"),
            "the config was read before the URL was checked: {error}"
        );
    }

    /// `fs::write` truncates before it writes, so a concurrent reader saw
    /// an empty file, concluded `api-keys` was missing, generated a fresh
    /// key and wrote a whole new config over everything. Eight concurrent
    /// registrations left four, each silently replacing the file — and
    /// the lock could not prevent it, because `load_config` runs before
    /// the lock is taken.
    #[test]
    fn a_reader_never_sees_a_half_written_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        let original =
            "host: \"127.0.0.1\"\nport: 8317\napi-keys:\n  - sk-original\nproviders: {}\n";
        fs::write(&path, original).expect("write config");

        std::thread::scope(|scope| {
            for index in 0..8 {
                let writer_path = path.clone();
                scope.spawn(move || {
                    register_provider(
                        &writer_path,
                        &format!("p{index}"),
                        &format!("https://h{index}/v1"),
                    )
                    .expect("register");
                });
                // A reader racing every writer: it must never observe a
                // file that parses as empty.
                let reader_path = path.clone();
                scope.spawn(move || {
                    for _ in 0..20 {
                        if let Ok(text) = fs::read_to_string(&reader_path) {
                            assert!(
                                text.contains("api-keys"),
                                "a reader saw a half-written config: {text:?}"
                            );
                        }
                    }
                });
            }
        });

        let after = fs::read_to_string(&path).expect("read config");
        assert!(
            after.contains("sk-original"),
            "the original api-key was replaced: {after}"
        );
        for index in 0..8 {
            assert!(after.contains(&format!("p{index}:")), "p{index} was lost");
        }
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

#[cfg(test)]
mod lock_tests {
    use super::register_provider;
    use std::fs;

    fn config(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("config.yaml");
        fs::write(
            &path,
            "host: \"127.0.0.1\"\nport: 8317\napi-keys:\n  - sk-test\nproviders: {}\n",
        )
        .expect("write config");
        path
    }

    /// A killed registration cannot run `Drop`, so its lock outlives it.
    /// README promises the next registration names the file and that
    /// removing it is the whole recovery — the one failure an operator
    /// actually meets, and nothing pinned it.
    #[test]
    fn a_stale_lock_names_itself_and_removing_it_is_the_recovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = config(dir.path());
        let lock = dir.path().join("config.yaml.lock");
        fs::write(&lock, "").expect("stale lock");

        let error = register_provider(&path, "groq", "https://api.groq.com/openai/v1")
            .expect_err("a held lock was ignored");
        let error = format!("{error:#}");
        assert!(
            error.contains("config.yaml.lock"),
            "the error does not name the file to remove: {error}"
        );

        fs::remove_file(&lock).expect("remove lock");
        register_provider(&path, "groq", "https://api.groq.com/openai/v1")
            .expect("removing the lock did not restore service");
    }

    /// And the lock is released by finishing, not only by the operator
    /// deleting it: without this, a `Drop` that does nothing looks
    /// exactly like a `Drop` that works until the second registration.
    #[test]
    fn a_finished_registration_releases_its_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = config(dir.path());

        register_provider(&path, "groq", "https://api.groq.com/openai/v1").expect("first");
        assert!(
            !dir.path().join("config.yaml.lock").exists(),
            "the lock outlived the registration that took it"
        );
        // The proof that matters: a second registration can still take it.
        register_provider(&path, "other", "https://other.host/v1")
            .expect("the lock was never released");
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_base_url;

    /// A stored form must be a fixed point, or the guard that compares
    /// against it disagrees with the writer that produced it. The first
    /// version chained three trims and was not one.
    #[test]
    fn normalizing_twice_changes_nothing() {
        for url in [
            "https://h/v1",
            "https://h/v1/",
            "https://h/v1 /",
            "https://h/v1/ /",
            "  https://h/v1//  ",
            "https://h/v1/ / / ",
            "/",
            "///",
            "   ",
            "",
        ] {
            let once = normalize_base_url(url);
            let twice = normalize_base_url(once);
            assert_eq!(once, twice, "not a fixed point: {url:?} -> {once:?}");
        }
    }

    /// And it still does the job it was extracted for.
    #[test]
    fn it_strips_what_the_upstream_join_would_double() {
        assert_eq!(normalize_base_url("https://h/v1/"), "https://h/v1");
        assert_eq!(normalize_base_url("https://h/v1 /"), "https://h/v1");
        assert_eq!(normalize_base_url("  https://h/v1  "), "https://h/v1");
        assert_eq!(normalize_base_url("/"), "");
        assert_eq!(normalize_base_url("   "), "");
    }
}

#[cfg(test)]
mod body_limit_tests {
    use super::{BodyLimit, DEFAULT_BODY_LIMIT_BYTES, RawConfig, parse_body_limit};

    #[test]
    fn suffixes_and_bare_numbers_mean_the_same_thing() {
        assert_eq!(
            parse_body_limit("10b").expect("bytes"),
            BodyLimit::Limited(10)
        );
        assert_eq!(
            parse_body_limit("10").expect("a bare number is bytes"),
            BodyLimit::Limited(10)
        );
        assert_eq!(
            parse_body_limit("2kb").expect("kb"),
            BodyLimit::Limited(2 * 1024)
        );
        assert_eq!(
            parse_body_limit("3mb").expect("mb"),
            BodyLimit::Limited(3 * 1024 * 1024)
        );
        assert_eq!(
            parse_body_limit("1gb").expect("gb"),
            BodyLimit::Limited(1024 * 1024 * 1024)
        );
        // The suffixes differ by a letter, so case and padding must not matter.
        assert_eq!(
            parse_body_limit("  200MB  ").expect("case and space"),
            BodyLimit::Limited(200 * 1024 * 1024)
        );
    }

    #[test]
    fn an_empty_setting_means_unlimited() {
        assert_eq!(parse_body_limit("").expect("empty"), BodyLimit::Unlimited);
        assert_eq!(
            parse_body_limit("   ").expect("blank"),
            BodyLimit::Unlimited
        );
    }

    #[test]
    fn an_unparseable_setting_is_an_error_not_a_default() {
        // The whole point: a typo must not quietly become some other limit. If this ever
        // returns `Ok`, the operator's setting is being replaced without being told.
        for bad in ["abc", "10xb", "mb", "1.5mb", "-1", "é"] {
            assert!(
                parse_body_limit(bad).is_err(),
                "{bad:?} must not parse to a limit"
            );
        }
    }

    #[test]
    fn a_space_before_the_suffix_is_tolerated() {
        // Pre-existing leniency, kept so this change is not also a behaviour change: the
        // parser trims between the number and its suffix. Pinned so that tightening it is
        // a decision rather than an accident.
        assert_eq!(
            parse_body_limit("10 mb").expect("space before suffix"),
            BodyLimit::Limited(10 * 1024 * 1024)
        );
    }

    #[test]
    fn an_overflowing_setting_is_an_error_not_a_wrapped_limit() {
        // u64::MAX bytes cannot be multiplied by 1024 without wrapping, and a wrapped
        // limit is smaller than the operator asked for.
        assert!(parse_body_limit("18446744073709551615gb").is_err());
        assert!(parse_body_limit("99999999999999999999999").is_err());
    }

    #[test]
    fn the_default_setting_and_the_default_constant_agree() {
        // Two spellings of one number: the string written into a fresh config, and the
        // constant the tests and the docs reason about. Nothing else ties them together,
        // so changing one without the other is caught here.
        assert_eq!(
            parse_body_limit(&RawConfig::default().body_limit).expect("the default must parse"),
            BodyLimit::Limited(DEFAULT_BODY_LIMIT_BYTES)
        );
    }
}
