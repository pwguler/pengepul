use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;

use crate::config::{Config, load_config, selected_config_path};
use crate::tokens::save_token;
use crate::types::{ProviderId, ProviderKind, TokenData};
use crate::utils::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInstallRequest {
    pub config_path: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub start: bool,
    pub enable: bool,
}

pub trait CliRuntime {
    /// Start the HTTP relay.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot be started.
    fn run_server(&mut self, config: &Config) -> Result<()>;

    /// Fetch local relay health.
    ///
    /// # Errors
    ///
    /// Returns an error if the admin request fails.
    fn health(&mut self, base_url: &str) -> Result<Value>;

    /// Fetch runtime account state.
    ///
    /// # Errors
    ///
    /// Returns an error if the admin request fails.
    fn accounts(&mut self, base_url: &str, api_key: &str) -> Result<Value>;

    /// Whether stdout is a terminal, so the pool views may render panels.
    /// Asked once at the edge; the CLI core only sees the answer.
    fn stdout_is_tty(&mut self) -> bool;

    /// Reload runtime account state.
    ///
    /// # Errors
    ///
    /// Returns an error if the admin request fails.
    fn reload_accounts(&mut self, base_url: &str, api_key: &str) -> Result<Value>;

    /// Install the user service.
    ///
    /// # Errors
    ///
    /// Returns an error if service installation fails.
    fn install_service(&mut self, request: ServiceInstallRequest) -> Result<PathBuf>;

    /// Start the installed user service.
    ///
    /// # Errors
    ///
    /// Returns an error if the service manager command fails.
    fn start_service(&mut self) -> Result<()>;

    /// Stop the installed user service.
    ///
    /// # Errors
    ///
    /// Returns an error if the service manager command fails.
    fn stop_service(&mut self) -> Result<()>;

    /// Restart the installed user service.
    ///
    /// # Errors
    ///
    /// Returns an error if the service manager command fails.
    fn restart_service(&mut self) -> Result<()>;

    /// Return service manager status text.
    ///
    /// # Errors
    ///
    /// Returns an error if the service manager command fails.
    fn service_status(&mut self) -> Result<String>;

    /// Uninstall the user service.
    ///
    /// # Errors
    ///
    /// Returns an error if the service cannot be stopped, disabled, or removed.
    fn uninstall_service(&mut self) -> Result<PathBuf>;

    /// Show the installed user service logs.
    ///
    /// # Errors
    ///
    /// Returns an error if the log viewer command cannot be spawned.
    fn service_logs(&mut self, follow: bool, lines: u32) -> Result<()>;

    /// Authorize and save an upstream account.
    ///
    /// A configured (static-key) provider carries its key in `key`; anthropic and
    /// codex ignore it.
    ///
    /// # Errors
    ///
    /// Returns an error if OAuth authorization, token exchange, or token persistence fails.
    fn login(&mut self, config: &Config, provider: ProviderId, key: Option<&str>)
    -> Result<String>;

    /// Resolve the tag of the newest published release.
    ///
    /// # Errors
    ///
    /// Returns an error if the release cannot be resolved.
    fn latest_release_tag(&mut self) -> Result<String>;

    /// Download `asset` from release `tag`, verify it, and replace this binary.
    ///
    /// # Errors
    ///
    /// Returns an error if the download, verification, or replacement fails.
    fn install_release(&mut self, tag: &str, asset: &str) -> Result<PathBuf>;
}

#[derive(Debug, Parser)]
#[command(name = "pengepul", version, disable_help_subcommand = true)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// start the API relay
    Serve {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
    },
    /// authorize an upstream account
    Login {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
        #[arg(long)]
        provider: String,
        /// static API key for a configured OpenAI-compatible provider
        #[arg(long)]
        key: Option<String>,
    },
    /// show local server status
    Status {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
    },
    /// install the latest release over this binary
    Update {
        /// report the available version without installing it
        #[arg(long)]
        check: bool,
    },
    /// show loaded provider accounts
    Accounts {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
        #[arg(long)]
        reload: bool,
    },
    /// inspect config
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// manage the user service
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// show help for a command
    Help {
        #[arg(trailing_var_arg = true)]
        topic: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum ConfigCommand {
    /// print config path
    Path,
    /// print config YAML
    Show,
    /// print the first configured API key
    ApiKey,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// install user service
    Install {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        start: bool,
        #[arg(long)]
        enable: bool,
    },
    /// start service
    Start,
    /// stop service
    Stop,
    /// restart service
    Restart,
    /// show service manager status
    Status,
    /// remove user service
    Uninstall,
    /// show service logs (uses the user journal on Linux)
    Logs {
        /// follow the log stream (Ctrl-C to stop)
        #[arg(long, short = 'f')]
        follow: bool,
        /// number of past log lines to show
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: u32,
    },
}

/// Run CLI logic against explicit filesystem roots and an injected runtime.
///
/// # Errors
///
/// Returns an error for invalid command args, invalid config, or runtime failures.
pub fn run_with_env(
    argv: &[&str],
    home: &Path,
    cwd: &Path,
    runtime: &mut impl CliRuntime,
) -> Result<RunOutcome> {
    let style = Style::from_tty(
        runtime.stdout_is_tty(),
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("TERM").as_deref(),
    );
    let mut raw = Vec::with_capacity(argv.len() + 1);
    raw.push("pengepul");
    raw.extend_from_slice(argv);
    let parsed_args = Args::try_parse_from(raw)?;
    let mut output = Output::default();

    match parsed_args.command {
        None => serve(
            parsed_args.config.as_deref(),
            None,
            None,
            home,
            cwd,
            runtime,
        )?,
        Some(Command::Serve {
            command_config,
            host,
            port,
        }) => serve(
            command_config.as_deref().or(parsed_args.config.as_deref()),
            host,
            port,
            home,
            cwd,
            runtime,
        )?,
        Some(Command::Update { check }) => update(check, runtime, &mut output)?,
        Some(Command::Status { command_config }) => {
            status(
                command_config.as_deref().or(parsed_args.config.as_deref()),
                home,
                cwd,
                runtime,
                &mut output,
                style,
            )?;
        }
        Some(Command::Accounts {
            command_config,
            reload,
        }) => {
            accounts(
                command_config.as_deref().or(parsed_args.config.as_deref()),
                reload,
                home,
                cwd,
                runtime,
                &mut output,
                style,
            )?;
        }
        Some(Command::Config { command }) => {
            config_command(
                command,
                parsed_args.config.as_deref(),
                home,
                cwd,
                &mut output,
            )?;
        }
        Some(Command::Service { command }) => {
            service_command(command, parsed_args.config.as_deref(), runtime, &mut output)?;
        }
        Some(Command::Help { topic }) => {
            output.line(&help_text(&topic)?);
        }
        Some(Command::Login {
            command_config,
            provider,
            key,
        }) => {
            login(
                command_config.as_deref().or(parsed_args.config.as_deref()),
                &provider,
                key.as_deref(),
                home,
                cwd,
                runtime,
                &mut output,
            )?;
        }
    }

    Ok(RunOutcome {
        code: 0,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn serve(
    config_path: Option<&Path>,
    host: Option<String>,
    port: Option<u16>,
    home: &Path,
    cwd: &Path,
    runtime: &mut impl CliRuntime,
) -> Result<()> {
    let mut config = load_config(config_path, Some(home), cwd)?;
    if let Some(host) = host {
        config.host = host;
    }
    if let Some(port) = port {
        config.port = port;
    }
    runtime.run_server(&config)
}

fn status(
    config_path: Option<&Path>,
    home: &Path,
    cwd: &Path,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let config = load_config(config_path, Some(home), cwd)?;
    let base_url = base_url(&config);
    output.line(&format!(
        "config: {}",
        selected_config_path(config_path, Some(home), cwd).display()
    ));
    output.line(&format!("url: {base_url}"));
    let health = runtime.health(&base_url)?;
    output.line(&format!(
        "server: {}",
        health
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));
    let accounts = runtime.accounts(&base_url, &first_api_key(&config)?)?;
    match style {
        Style::Plain => print_pool(&accounts, output),
        Style::Rich => print_pool_rich(&accounts, output, false),
    }
    Ok(())
}

fn accounts(
    config_path: Option<&Path>,
    reload: bool,
    home: &Path,
    cwd: &Path,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let config = load_config(config_path, Some(home), cwd)?;
    let base_url = base_url(&config);
    let api_key = first_api_key(&config)?;
    if reload {
        runtime.reload_accounts(&base_url, &api_key)?;
        output.line("reloaded accounts");
    }
    let accounts = runtime.accounts(&base_url, &api_key)?;
    match style {
        Style::Plain => print_accounts(&accounts, output),
        Style::Rich => print_pool_rich(&accounts, output, true),
    }
    Ok(())
}

fn config_command(
    command: ConfigCommand,
    config_path: Option<&Path>,
    home: &Path,
    cwd: &Path,
    output: &mut Output,
) -> Result<()> {
    let path = selected_config_path(config_path, Some(home), cwd);
    match command {
        ConfigCommand::Path => output.line(&path.display().to_string()),
        ConfigCommand::ApiKey => {
            let config = load_config(config_path, Some(home), cwd)?;
            output.line(&first_api_key(&config)?);
        }
        ConfigCommand::Show => {
            // Generates the config when absent, so `show` works on a fresh
            // install like every other config subcommand.
            load_config(config_path, Some(home), cwd)?;
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            output.line(text.trim_end());
        }
    }
    Ok(())
}

fn service_command(
    command: ServiceCommand,
    root_config_path: Option<&Path>,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
) -> Result<()> {
    match command {
        ServiceCommand::Install {
            command_config,
            host,
            port,
            start,
            enable,
        } => {
            let path = runtime.install_service(ServiceInstallRequest {
                config_path: command_config.or_else(|| root_config_path.map(Path::to_path_buf)),
                host,
                port,
                start,
                enable,
            })?;
            output.line(&format!("installed service: {}", path.display()));
        }
        ServiceCommand::Start => {
            runtime.start_service()?;
            output.line("started service");
        }
        ServiceCommand::Stop => {
            runtime.stop_service()?;
            output.line("stopped service");
        }
        ServiceCommand::Restart => {
            runtime.restart_service()?;
            output.line("restarted service");
        }
        ServiceCommand::Status => output.line(&runtime.service_status()?),
        ServiceCommand::Uninstall => {
            let path = runtime.uninstall_service()?;
            output.line(&format!("uninstalled service: {}", path.display()));
        }
        ServiceCommand::Logs { follow, lines } => runtime.service_logs(follow, lines)?,
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// Release asset for the platform this binary was built for.
///
/// # Errors
///
/// Returns an error on a platform with no published build.
pub fn release_asset() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("pengepul-linux-x86_64.tar.gz"),
        ("macos", "aarch64") => Ok("pengepul-macos-arm64.tar.gz"),
        (os, arch) => bail!(
            "no published build for {os} {arch}; install from source with \
             cargo install --git https://github.com/pwguler/pengepul.git --locked"
        ),
    }
}

/// Compare a release tag against this binary's version.
///
/// Tags carry a leading `v`. A tag that does not parse is treated as newer, so a
/// release naming scheme change still prompts an update rather than going silent.
#[must_use]
pub fn tag_is_newer(tag: &str, current: &str) -> bool {
    fn parts(value: &str) -> Option<(u64, u64, u64)> {
        let mut it = value.trim_start_matches('v').split('.');
        Some((
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
            it.next()?.split(['-', '+']).next()?.parse().ok()?,
        ))
    }
    match (parts(tag), parts(current)) {
        (Some(latest), Some(running)) => latest > running,
        _ => true,
    }
}

fn update(check: bool, runtime: &mut impl CliRuntime, output: &mut Output) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let asset = release_asset()?;
    let tag = runtime.latest_release_tag()?;

    if !tag_is_newer(&tag, current) {
        output.line(&format!("pengepul {current} is the latest release"));
        return Ok(());
    }
    if check {
        output.line(&format!(
            "pengepul {tag} is available (running {current}); run `pengepul update` to install it"
        ));
        return Ok(());
    }

    let path = runtime.install_release(&tag, asset)?;
    output.line(&format!("updated to {tag} at {}", path.display()));
    Ok(())
}

fn login(
    config_path: Option<&Path>,
    provider: &str,
    key: Option<&str>,
    home: &Path,
    cwd: &Path,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
) -> Result<()> {
    let config = load_config(config_path, Some(home), cwd)?;
    // anthropic and codex are always valid; anything else must name a configured
    // provider, and only a configured provider may carry a key.
    if let Ok(builtin) = provider.parse::<ProviderId>() {
        if key.is_some() {
            bail!("{builtin} uses OAuth; --key is for configured providers");
        }
        let email = runtime.login(&config, builtin.clone(), key)?;
        output.line(&format!("saved {builtin} account token for {email}"));
        return Ok(());
    }
    if !config.providers.contains_key(provider) {
        bail!(
            "{provider} is not configured; configured providers: {}",
            config
                .providers
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let key = key.context(format!("{provider} takes a static API key; pass --key"))?;
    let label = format!("key-{}", &sha256_hex(key)[..8]);
    let provider_id = ProviderId::new(ProviderKind::Generic, provider);
    let token = TokenData {
        access_token: key.to_string(),
        refresh_token: String::new(),
        email: label.clone(),
        expires_at: String::new(),
        account_uuid: sha256_hex(key),
        provider: provider_id,
        id_token: None,
        last_refresh_at: None,
        plan_type: None,
    };
    save_token(&config.auth_dir, &token)?;
    output.line(&format!("saved {provider} account token for {label}"));
    Ok(())
}

fn help_text(topic: &[String]) -> Result<String> {
    let mut command = Args::command();
    for item in topic {
        let Some(next) = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == item)
            .cloned()
        else {
            bail!("unknown help topic: {}", topic.join(" "));
        };
        command = next;
    }
    let bin_name = if topic.is_empty() {
        "pengepul".to_string()
    } else {
        format!("pengepul {}", topic.join(" "))
    };
    command = command.bin_name(bin_name);
    let text = command.render_help().to_string();
    if topic.is_empty() {
        Ok(text)
    } else if let Some(index) = text.find("Usage:") {
        Ok(text[index..].to_string())
    } else {
        Ok(text)
    }
}

/// Whether the pool views render panels or plain text. Decided once at the
/// edge from TTY and environment answers; the CLI core only sees the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Rich,
    Plain,
}

impl Style {
    /// `Rich` needs a TTY and no color-suppression variable; anything else —
    /// piped output, `NO_COLOR` set (even empty), `TERM=dumb` — is `Plain`.
    /// Plain is the only non-Rich mode: there is no uncolored panel.
    #[must_use]
    pub fn from_tty(is_tty: bool, no_color: Option<&OsStr>, term: Option<&OsStr>) -> Self {
        let no_color = no_color.is_some();
        let dumb = term == Some(OsStr::new("dumb"));
        if is_tty && !no_color && !dumb {
            Self::Rich
        } else {
            Self::Plain
        }
    }
}

fn base_url(config: &Config) -> String {
    let mut host = if config.host.is_empty() || matches!(config.host.as_str(), "0.0.0.0" | "::") {
        "127.0.0.1".to_string()
    } else {
        config.host.clone()
    };
    if host.contains(':') && !host.starts_with('[') {
        host = format!("[{host}]");
    }
    format!("http://{host}:{}", config.port)
}

fn first_api_key(config: &Config) -> Result<String> {
    config
        .api_keys
        .iter()
        .min()
        .cloned()
        .context("config has no API keys")
}

/// One decimal at K/M scale, integers under 1,000. Integer math throughout,
/// so the rendered value is exact and truncation-free.
fn format_count(value: i64) -> String {
    let magnitude = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };
    if magnitude >= 1_000_000 {
        let whole = magnitude / 1_000_000;
        let tenths = (magnitude % 1_000_000) / 100_000;
        format!("{sign}{whole}.{tenths}M")
    } else if magnitude >= 1_000 {
        let whole = magnitude / 1_000;
        let tenths = (magnitude % 1_000) / 100;
        format!("{sign}{whole}.{tenths}K")
    } else {
        format!("{sign}{magnitude}")
    }
}

/// Comma-grouped form for request counts, which stay exact on the rollup line.
fn format_exact(value: i64) -> String {
    let magnitude = value.unsigned_abs().to_string();
    let sign = if value < 0 { "-" } else { "" };
    let mut grouped = String::with_capacity(magnitude.len() + magnitude.len() / 3);
    for (index, character) in magnitude.chars().enumerate() {
        if index > 0 && (magnitude.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    format!("{sign}{grouped}")
}

/// `"on cooldown 4m12s"` while a cooldown lasts, `""` once it has cleared or
/// was never set (`cooldownUntil` is an absolute unix timestamp).
fn cooldown_label(now: f64, cooldown_until: f64) -> String {
    let remaining = (cooldown_until - now).max(0.0).floor();
    // Clamped non-negative and floored above, so the cast loses neither sign
    // nor a meaningful fraction.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let remaining = remaining as u64;
    if remaining == 0 {
        return String::new();
    }
    let (hours, rem) = (remaining / 3_600, remaining % 3_600);
    if hours > 0 {
        format!("on cooldown {hours}h{}m", rem / 60)
    } else {
        let (minutes, seconds) = (rem / 60, rem % 60);
        if minutes == 0 {
            format!("on cooldown {seconds}s")
        } else {
            format!("on cooldown {minutes}m{seconds}s")
        }
    }
}

/// The per-provider pool view under `status`: availability header (with
/// cooldown detail), summed request outcomes, summed token totals. Reads only
/// what `GET /admin/accounts` already serves; the server is untouched. An
/// empty pool prints only its bare header, matching the old count line.
fn print_pool(payload: &Value, output: &mut Output) {
    let mut first = true;
    for (provider_id, provider) in providers(payload) {
        let accounts = provider
            .get("accounts")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let count = provider
            .get("account_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let suffix = if count == 1 { "account" } else { "accounts" };
        if accounts.is_empty() {
            output.line(&format!("{provider_id}: {count} {suffix}"));
            continue;
        }
        if first {
            output.line("");
        }
        first = false;
        let available = accounts
            .iter()
            .filter(|account| {
                account
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count();
        let mut header = format!("{provider_id}: {count} {suffix} ({available} available)");
        let now = unix_now();
        let cooling: Vec<String> = accounts
            .iter()
            .filter_map(|account| {
                let available = account
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let until = account
                    .get("cooldownUntil")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                (!available && until > now)
                    .then(|| format!("{} {}", email(account), cooldown_label(now, until)))
            })
            .collect();
        if !cooling.is_empty() {
            write!(header, ", {}", cooling.join(", ")).expect("write to String cannot fail");
        }
        output.line(&header);

        let mut totals = PoolTotals::default();
        for account in accounts {
            totals.add(account);
        }
        output.line(&format!(
            "  requests {}  ({} ok, {} failed)",
            format_exact(totals.requests),
            format_exact(totals.successes),
            format_exact(totals.failures)
        ));
        output.line(&format!(
            "  tokens in {}  out {}  cache-read {}  cache-write {}  reasoning {}",
            format_count(totals.input),
            format_count(totals.output),
            format_count(totals.cache_read),
            format_count(totals.cache_write),
            format_count(totals.reasoning)
        ));
    }
}

fn print_accounts(payload: &Value, output: &mut Output) {
    for (provider_id, provider) in providers(payload) {
        let count = provider
            .get("account_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let suffix = if count == 1 { "account" } else { "accounts" };
        output.line(&format!("{provider_id}: {count} {suffix}"));
        let Some(accounts) = provider.get("accounts").and_then(Value::as_array) else {
            continue;
        };
        let now = unix_now();
        for account in accounts {
            let failures = account
                .get("failureCount")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let state = if account
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "available".to_string()
            } else {
                // Cooldown accounts show the remaining time; a snapshot with
                // `available: false` and no future `cooldownUntil` stays
                // "unavailable" (older relays, or a just-expired cooldown).
                let until = account
                    .get("cooldownUntil")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let label = cooldown_label(now, until);
                if label.is_empty() {
                    "unavailable".to_string()
                } else {
                    label
                }
            };
            let mut line = format!("  {} {state} failures={failures}", email(account));
            if let Some(plan_type) = account.get("planType").and_then(Value::as_str) {
                write!(line, " plan={plan_type}").expect("write to String cannot fail");
            }
            output.line(&line);
            let reasoning = i64_field(account, "totalReasoningOutputTokens");
            let mut detail = format!(
                "    requests {} ({} ok) in {} out {} cache-read {} cache-write {}",
                format_exact(i64_field(account, "totalRequests")),
                format_exact(i64_field(account, "totalSuccesses")),
                format_count(i64_field(account, "totalInputTokens")),
                format_count(i64_field(account, "totalOutputTokens")),
                format_count(i64_field(account, "totalCacheReadInputTokens")),
                format_count(i64_field(account, "totalCacheCreationInputTokens")),
            );
            if reasoning != 0 {
                write!(detail, " reasoning {}", format_count(reasoning))
                    .expect("write to String cannot fail");
            }
            output.line(&detail);
        }
    }
}

fn providers(payload: &Value) -> Vec<(&str, &Value)> {
    let Some(providers) = payload.get("providers").and_then(Value::as_object) else {
        return Vec::new();
    };
    providers
        .iter()
        .map(|(provider_id, provider)| (provider_id.as_str(), provider))
        .collect()
}

#[derive(Default)]
struct PoolTotals {
    requests: i64,
    successes: i64,
    failures: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
}

impl PoolTotals {
    fn add(&mut self, account: &Value) {
        self.requests += i64_field(account, "totalRequests");
        self.successes += i64_field(account, "totalSuccesses");
        self.failures += i64_field(account, "totalFailures");
        self.input += i64_field(account, "totalInputTokens");
        self.output += i64_field(account, "totalOutputTokens");
        self.cache_read += i64_field(account, "totalCacheReadInputTokens");
        self.cache_write += i64_field(account, "totalCacheCreationInputTokens");
        self.reasoning += i64_field(account, "totalReasoningOutputTokens");
    }
}

fn i64_field(account: &Value, field: &str) -> i64 {
    account.get(field).and_then(Value::as_i64).unwrap_or(0)
}

fn email(account: &Value) -> &str {
    account
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

/// Panel geometry: inner width (between the `│ ` and ` │` gutters), glyph
/// column, and the email column wide enough for most addresses before
/// truncation. Deriving the row layout from one table keeps the columns
/// aligned by construction.
/// Panel geometry, with the column budget summing to 59 of 60 inner
/// columns: email 16 + state 17 (glyph, space, text) + ok 9 + bar 10 +
/// pct 3, single spaces between.
const INNER_WIDTH: usize = PANEL_WIDTH - 4;
const EMAIL_WIDTH: usize = 16;
const STATE_SPAN_WIDTH: usize = 17; // "● " + "cooldown 23h59m"
const OK_WIDTH: usize = 9; // "999,999 ok"

/// One `│ … │` row, trimmed to the panel width if the content overruns —
/// box integrity wins over content, and overruns cannot happen for the
/// numbers this renderer prints (emails truncate via `pad`).
/// One `│ … │` row. The colored content is kept verbatim; only its *visible*
/// width is measured, and tail padding is added after it, so escape bytes
/// never fool the geometry. A row whose visible text overruns the fixed
/// width degrades to uncolored, clipped text — box integrity wins over
/// content, and the renderer's own columns never overrun.
fn panel_row(content: &str) -> String {
    let plain = strip_ansi(content);
    let visible = plain.chars().count();
    if visible > INNER_WIDTH {
        let clipped: String = plain.chars().take(INNER_WIDTH).collect();
        format!("│ {clipped} │")
    } else {
        let tail = " ".repeat(INNER_WIDTH - visible);
        format!("│ {content}{tail} │")
    }
}

/// Strip ANSI escape sequences, keeping the visible text. Lives with the
/// render code so measurement and tests share one definition.
fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' {
            while let Some(&next) = chars.peek() {
                chars.next();
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(character);
        }
    }
    out
}

fn glyph(available: bool, on_cooldown: bool) -> String {
    if available {
        paint(GREEN, "●")
    } else if on_cooldown {
        paint(AMBER, "●")
    } else {
        paint(RED, "●")
    }
}

/// The rich pool view: one panel per provider with rows and a footer rollup.
/// Pure over the payload and the clock value handed to it; `with_detail`
/// adds the per-account token line the `accounts` command shows.
fn print_pool_rich(payload: &Value, output: &mut Output, with_detail: bool) {
    let now = unix_now();
    for (provider_id, provider) in providers(payload) {
        let accounts = provider
            .get("accounts")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let count = provider
            .get("account_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let suffix = if count == 1 { "account" } else { "accounts" };
        if accounts.is_empty() {
            output.line(&format!(
                "{} {provider_id}: {count} {suffix} {}",
                paint(DIM, "·"),
                paint(DIM, "(no accounts loaded)")
            ));
            continue;
        }
        let available = accounts
            .iter()
            .filter(|account| {
                account
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count();

        output.line(&top_rule(provider_id, count, suffix, available));

        let pool_total: i64 = accounts.iter().map(account_tokens).sum();
        let mut totals = PoolTotals::default();
        for account in accounts {
            totals.add(account);
        }
        for account in accounts {
            output.line(&panel_row(&account_row(account, pool_total, now)));
            if with_detail {
                for line in account_detail_lines(account) {
                    output.line(&panel_row(&line));
                }
            }
        }

        output.line(&format!("├{}┤", "─".repeat(INNER_WIDTH + 2)));
        for line in footer_lines(&totals) {
            output.line(&panel_row(&line));
        }
        output.line(&format!("└{}┘", "─".repeat(INNER_WIDTH + 2)));
    }
}

/// Top rule with the pool header inside: `┌─ pool: <id> ─ <N, A> ─…┐`. The
/// fill brings the rule to the same width as a panel row.
fn top_rule(provider_id: &str, count: i64, suffix: &str, available: usize) -> String {
    let header = format!("pool: {provider_id} ─ {count} {suffix}, {available} available");
    let fill = INNER_WIDTH - header.chars().count() - 1;
    let mut top = format!("┌─ {header} ");
    top.extend(std::iter::repeat_n('─', fill));
    top.push('┐');
    top
}

/// One colored account row: email, glyph + state, ok count, share bar.
fn account_row(account: &Value, pool_total: i64, now: f64) -> String {
    let is_available = account
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let until = account
        .get("cooldownUntil")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let raw_cooldown = cooldown_label(now, until);
    let on_cooldown = !is_available && !raw_cooldown.is_empty();
    let label = if is_available {
        "available".to_string()
    } else if on_cooldown {
        // The glyph already says the condition; the row drops the plain
        // branch's leading "on".
        raw_cooldown
            .strip_prefix("on ")
            .unwrap_or(&raw_cooldown)
            .to_string()
    } else {
        "unavailable".to_string()
    };
    let state_color = if is_available {
        GREEN
    } else if on_cooldown {
        AMBER
    } else {
        RED
    };
    let ok = i64_field(account, "totalSuccesses");
    let state_span = format!(
        "{} {}",
        glyph(is_available, on_cooldown),
        paint(state_color, &pad(&label, STATE_SPAN_WIDTH - 2))
    );
    let ok_text = format!("{} ok", format_exact(ok));
    let (bar, share) = share_bar(account_tokens(account), pool_total);
    let share_text = share.map_or_else(|| " ".to_string(), |value| format!("{value}%"));
    [
        pad(email(account), EMAIL_WIDTH),
        state_span,
        paint(BOLD, &pad(&ok_text, OK_WIDTH)),
        bar,
        paint(DIM, &share_text),
    ]
    .join(" ")
}

/// The dim per-account token lines shown under `accounts`: in/out/read/write,
/// plus a reasoning line only when that total is non-zero.
fn account_detail_lines(account: &Value) -> Vec<String> {
    let mut lines = vec![paint(
        DIM,
        &format!(
            "in {}  out {}  read {}  write {}",
            format_count(i64_field(account, "totalInputTokens")),
            format_count(i64_field(account, "totalOutputTokens")),
            format_count(i64_field(account, "totalCacheReadInputTokens")),
            format_count(i64_field(account, "totalCacheCreationInputTokens")),
        ),
    )];
    let reasoning = i64_field(account, "totalReasoningOutputTokens");
    if reasoning != 0 {
        lines.push(paint(
            DIM,
            &format!("reasoning {}", format_count(reasoning)),
        ));
    }
    lines
}

/// Footer rollup lines: requests, tokens, and reasoning when non-zero.
fn footer_lines(totals: &PoolTotals) -> Vec<String> {
    let mut lines = vec![format!(
        "requests {}  ({} ok, {} failed)",
        paint(BOLD, &format_exact(totals.requests)),
        format_exact(totals.successes),
        format_exact(totals.failures)
    )];
    lines.push(format!(
        "tokens in {}  out {}  read {}  write {}",
        paint(BOLD, &format_count(totals.input)),
        paint(BOLD, &format_count(totals.output)),
        format_count(totals.cache_read),
        format_count(totals.cache_write),
    ));
    // Reasoning totals get their own line only when non-zero; one row
    // for all five fields cannot fit the fixed width.
    if totals.reasoning != 0 {
        lines.push(format!(
            "reasoning {}",
            paint(BOLD, &format_count(totals.reasoning))
        ));
    }
    lines
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// The panel is a fixed 64 columns; every row is padded or truncated to it.
/// (60 could not hold the row columns without truncating most emails.)
const PANEL_WIDTH: usize = 64;

/// ANSI: green / amber / red / dim / bold — the whole palette.
const GREEN: &str = "\x1b[32m";
const AMBER: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// Wrap one span in a color, always resetting after it, so color never
/// bleeds into neighbouring cells.
fn paint(color: &str, text: &str) -> String {
    format!("{color}{text}{RESET}")
}

/// Clamp to exactly `width` display columns: spaces pad short text and an
/// ellipsis replaces the last visible character of long text.
fn pad(text: &str, width: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() > width {
        let mut clipped: String = characters[..width - 1].iter().collect();
        clipped.push('…');
        return clipped;
    }
    let mut padded = text.to_string();
    padded.extend(std::iter::repeat_n(' ', width - characters.len()));
    padded
}

/// A 10-cell share bar: `█` per whole tenth of the share, `░` for the rest,
/// with the integer percentage — `None` only when the pool total is 0.
fn share_bar(tokens: i64, pool_total: i64) -> (String, Option<i64>) {
    if pool_total == 0 {
        return ("░░░░░░░░░░".to_string(), None);
    }
    let share = (tokens.max(0).min(pool_total) * 100).div_euclid(pool_total);
    // share is clamped to 0..=100 above, so the division is exact and the
    // cast loses nothing.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = (share as usize).div_euclid(10);
    let mut bar = String::with_capacity(30);
    bar.extend(std::iter::repeat_n('█', filled));
    bar.extend(std::iter::repeat_n('░', 10 - filled));
    (bar, Some(share))
}

/// The account's carried load: every token that crossed it. This is what the
/// share bar divides; there is no quota in the domain to fill one with.
fn account_tokens(account: &Value) -> i64 {
    i64_field(account, "totalInputTokens")
        + i64_field(account, "totalOutputTokens")
        + i64_field(account, "totalCacheReadInputTokens")
        + i64_field(account, "totalCacheCreationInputTokens")
}

#[derive(Default)]
struct Output {
    stdout: String,
    stderr: String,
}

impl Output {
    fn line(&mut self, value: &str) {
        self.stdout.push_str(value);
        self.stdout.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::{Style, cooldown_label, format_count, format_exact, pad, paint, share_bar};
    use std::ffi::OsStr;

    #[test]
    fn style_is_rich_only_on_a_color_tty() {
        // Via the free function so the non-test build carries no dead shim.
        assert_eq!(Style::from_tty(true, None, None), Style::Rich);
        assert_eq!(Style::from_tty(false, None, None), Style::Plain);
        assert_eq!(
            Style::from_tty(true, Some(OsStr::new("1")), None),
            Style::Plain
        );
        assert_eq!(
            Style::from_tty(true, Some(OsStr::new("")), None),
            Style::Plain
        );
        assert_eq!(
            Style::from_tty(true, None, Some(OsStr::new("dumb"))),
            Style::Plain
        );
        assert_eq!(
            Style::from_tty(true, None, Some(OsStr::new("xterm-256color"))),
            Style::Rich
        );
    }

    #[test]
    fn cooldown_label_rounds_down_to_minutes_and_seconds() {
        let now = 1_000_000.0;
        assert_eq!(cooldown_label(now, now + 252.0), "on cooldown 4m12s");
        assert_eq!(cooldown_label(now, now + 61.0), "on cooldown 1m1s");
        assert_eq!(cooldown_label(now, now + 60.0), "on cooldown 1m0s");
        assert_eq!(cooldown_label(now, now + 59.0), "on cooldown 59s");
        assert_eq!(cooldown_label(now, now + 1.0), "on cooldown 1s");
        // A sub-second remainder floors to cleared, so both commands agree.
        assert_eq!(cooldown_label(now, now + 0.5), "");
    }

    #[test]
    fn cooldown_label_switches_to_hours_past_an_hour() {
        let now = 1_000_000.0;
        assert_eq!(cooldown_label(now, now + 3_600.0), "on cooldown 1h0m");
        assert_eq!(cooldown_label(now, now + 3_661.0), "on cooldown 1h1m");
        assert_eq!(cooldown_label(now, now + 86_399.0), "on cooldown 23h59m");
    }

    #[test]
    fn cooldown_label_clears_for_elapsed_or_missing_cooldown() {
        let now = 1_000_000.0;
        assert_eq!(cooldown_label(now, now), "");
        assert_eq!(cooldown_label(now, now - 10.0), "");
        assert_eq!(cooldown_label(now, 0.0), "");
    }

    #[test]
    fn format_count_scales_tokens_with_one_decimal() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(7), "7");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0K");
        assert_eq!(format_count(155_300), "155.3K");
        assert_eq!(format_count(812_300), "812.3K");
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(45_200_000), "45.2M");
        assert_eq!(format_count(-1), "-1");
    }

    #[test]
    fn format_exact_groups_request_counts_with_commas() {
        assert_eq!(format_exact(0), "0");
        assert_eq!(format_exact(6), "6");
        assert_eq!(format_exact(999), "999");
        assert_eq!(format_exact(1_000), "1,000");
        assert_eq!(format_exact(1_204), "1,204");
        assert_eq!(format_exact(999_999), "999,999");
        assert_eq!(format_exact(1_000_000), "1,000,000");
        assert_eq!(format_exact(-1), "-1");
    }

    #[test]
    fn pad_pads_to_the_fixed_width() {
        assert_eq!(pad("abc", 6), "abc   ");
        assert_eq!(pad("", 3), "   ");
        assert_eq!(pad("abcdef", 6), "abcdef");
    }

    #[test]
    fn pad_truncates_with_an_ellipsis_never_exceeding_width() {
        assert_eq!(pad("abcdefg", 5), "abcd…");
        assert_eq!(pad("abcdefg", 1), "…");
        assert_eq!(pad("abcdefg", 6).chars().count(), 6);
    }

    #[test]
    fn share_bar_fills_whole_tenths_of_the_share() {
        assert_eq!(share_bar(50, 100), ("█████░░░░░".to_string(), Some(50)));
        assert_eq!(share_bar(45, 100), ("████░░░░░░".to_string(), Some(45)));
        assert_eq!(share_bar(1, 3), ("███░░░░░░░".to_string(), Some(33)));
        assert_eq!(share_bar(7, 7), ("██████████".to_string(), Some(100)));
    }

    #[test]
    fn share_bar_without_a_pool_total_is_empty_cells() {
        assert_eq!(share_bar(0, 0), ("░░░░░░░░░░".to_string(), None));
    }

    #[test]
    fn paint_wraps_the_span_and_resets() {
        assert_eq!(paint("\x1b[33m", "x"), "\x1b[33mx\x1b[0m");
    }
}
