use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;

use crate::config::{Config, load_config, selected_config_path};
pub use crate::render::Style;
use crate::render::{ActionGlyph, BOLD, DIM, Fact, Output, fact_panel, paint, status_glyph};
use crate::service::service_status_panel;
use crate::tokens::save_token;
use crate::types::{ProviderId, ProviderKind, TokenData};
use crate::usage_view::{
    Connection, print_accounts, print_pool_rich, print_relay_total_plain, print_relay_total_rich,
};
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
    style: Style,
) -> Result<RunOutcome> {
    // The Style decision is handed in, not made here: the CLI core never
    // reads the environment, so tests can drive either mode hermetically.
    let mut raw = Vec::with_capacity(argv.len() + 1);
    raw.push("pengepul");
    raw.extend_from_slice(argv);
    let parsed_args = Args::try_parse_from(raw)?;
    let mut output = Output::default();

    let root_env = CommandEnv::new(parsed_args.config.as_deref(), home, cwd);

    match parsed_args.command {
        None => serve(root_env, None, None, runtime)?,
        Some(Command::Serve {
            command_config,
            host,
            port,
        }) => serve(
            root_env.with_override(command_config.as_deref()),
            host,
            port,
            runtime,
        )?,
        Some(Command::Update { check }) => update(check, runtime, &mut output, style)?,
        Some(Command::Status { command_config }) => {
            status(
                root_env.with_override(command_config.as_deref()),
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
                root_env.with_override(command_config.as_deref()),
                reload,
                runtime,
                &mut output,
                style,
            )?;
        }
        Some(Command::Config { command }) => {
            config_command(command, root_env, &mut output, style)?;
        }
        Some(Command::Service { command }) => {
            service_command(command, root_env, runtime, &mut output, style)?;
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
                root_env.with_override(command_config.as_deref()),
                &provider,
                key.as_deref(),
                runtime,
                &mut output,
                style,
            )?;
        }
    }

    Ok(RunOutcome {
        code: 0,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Where a verb looks for its config: the `--config` override (command-
/// level first, then root), the home directory, and the working directory
/// `load_config` resolves relative paths against.
#[derive(Clone, Copy)]
struct CommandEnv<'a> {
    config_path: Option<&'a Path>,
    home: &'a Path,
    cwd: &'a Path,
}

impl<'a> CommandEnv<'a> {
    fn new(config_path: Option<&'a Path>, home: &'a Path, cwd: &'a Path) -> Self {
        Self {
            config_path,
            home,
            cwd,
        }
    }

    /// A command-level `--config` wins over the root one.
    fn with_override(self, command_config: Option<&'a Path>) -> Self {
        Self {
            config_path: command_config.or(self.config_path),
            ..self
        }
    }

    fn load(&self) -> Result<Config> {
        load_config(self.config_path, Some(self.home), self.cwd)
    }

    fn config_file(&self) -> PathBuf {
        selected_config_path(self.config_path, Some(self.home), self.cwd)
    }
}

fn serve(
    env: CommandEnv<'_>,
    host: Option<String>,
    port: Option<u16>,
    runtime: &mut impl CliRuntime,
) -> Result<()> {
    let mut config = env.load()?;
    if let Some(host) = host {
        config.host = host;
    }
    if let Some(port) = port {
        config.port = port;
    }
    runtime.run_server(&config)
}

fn status(
    env: CommandEnv<'_>,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let config = env.load()?;
    let base_url = base_url(&config);
    let health = runtime.health(&base_url)?;
    let server = health
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let connection = Connection {
        config: env.config_file().display().to_string(),
        url: base_url.clone(),
        server,
    };
    let accounts = runtime.accounts(&base_url, &first_api_key(&config)?)?;
    // status-total-only: the relay block is the whole view; per-pool and
    // per-account detail lives in `accounts`.
    match style {
        Style::Plain => print_relay_total_plain(&accounts, output, &connection),
        Style::Rich => print_relay_total_rich(&accounts, output, &connection),
    }
    Ok(())
}

fn accounts(
    env: CommandEnv<'_>,
    reload: bool,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let config = env.load()?;
    let base_url = base_url(&config);
    let api_key = first_api_key(&config)?;
    if reload {
        runtime.reload_accounts(&base_url, &api_key)?;
        output.line("reloaded accounts");
    }
    let accounts = runtime.accounts(&base_url, &api_key)?;
    let now = unix_now();
    match style {
        Style::Plain => print_accounts(&accounts, output, now),
        Style::Rich => print_pool_rich(&accounts, output, now),
    }
    Ok(())
}

fn config_command(
    command: ConfigCommand,
    env: CommandEnv<'_>,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let path = env.config_file();
    match command {
        ConfigCommand::Path => match style {
            Style::Plain => output.line(&path.display().to_string()),
            Style::Rich => {
                for line in fact_panel(
                    "config",
                    &[Fact::new("path", &paint(BOLD, &path.display().to_string()))],
                ) {
                    output.line(&line);
                }
            }
        },
        ConfigCommand::ApiKey => {
            let config = env.load()?;
            let key = first_api_key(&config)?;
            match style {
                Style::Plain => output.line(&key),
                Style::Rich => {
                    for line in fact_panel("config", &[Fact::new("api key", &paint(BOLD, &key))]) {
                        output.line(&line);
                    }
                }
            }
        }
        ConfigCommand::Show => {
            // Generates the config when absent, so `show` works on a fresh
            // install like every other config subcommand.
            env.load()?;
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            output.line(text.trim_end());
        }
    }
    Ok(())
}

fn service_command(
    command: ServiceCommand,
    env: CommandEnv<'_>,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
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
                config_path: env
                    .with_override(command_config.as_deref())
                    .config_path
                    .map(Path::to_path_buf),
                host,
                port,
                start,
                enable,
            })?;
            print_action_with_path("service", "installed", &path, output, style);
        }
        ServiceCommand::Start => {
            runtime.start_service()?;
            print_action(
                "service",
                "started service",
                "started",
                ActionGlyph::Ok,
                output,
                style,
            );
        }
        ServiceCommand::Stop => {
            runtime.stop_service()?;
            print_action(
                "service",
                "stopped service",
                "stopped",
                ActionGlyph::Ok,
                output,
                style,
            );
        }
        ServiceCommand::Restart => {
            runtime.restart_service()?;
            print_action(
                "service",
                "restarted service",
                "restarted",
                ActionGlyph::Ok,
                output,
                style,
            );
        }
        ServiceCommand::Status => match runtime.service_status() {
            Ok(text) => match style {
                Style::Plain => output.line(&text),
                Style::Rich => {
                    for line in service_status_panel(&text) {
                        output.line(&line);
                    }
                }
            },
            // Not installed: the panel turns it into an amber row instead of
            // failing the render; plain keeps the error so scripts see it.
            // Any other failure (tool missing, permission) stays an error
            // in both styles — a panel claiming "not installed" would lie.
            Err(error) => {
                let not_installed = error.to_string().contains("no service installed");
                match style {
                    Style::Rich if not_installed => {
                        for line in fact_panel(
                            "service",
                            &[
                                Fact::new(
                                    "state",
                                    &format!(
                                        "{} not installed",
                                        status_glyph(ActionGlyph::Attention)
                                    ),
                                ),
                                Fact::new("fix", &paint(DIM, "pengepul service install")),
                            ],
                        ) {
                            output.line(&line);
                        }
                    }
                    Style::Rich | Style::Plain => return Err(error),
                }
            }
        },
        ServiceCommand::Uninstall => {
            let path = runtime.uninstall_service()?;
            print_action_with_path("service", "uninstalled", &path, output, style);
        }
        ServiceCommand::Logs { follow, lines } => runtime.service_logs(follow, lines)?,
    }
    Ok(())
}

/// Print an install/uninstall outcome whose detail carries a path.
fn print_action_with_path(
    subject: &str,
    verb: &str,
    path: &Path,
    output: &mut Output,
    style: Style,
) {
    match style {
        Style::Plain => output.line(&format!("{verb} service: {}", path.display())),
        Style::Rich => {
            for line in fact_panel(
                subject,
                &[
                    Fact::new(
                        "state",
                        &format!("{} {verb}", status_glyph(ActionGlyph::Ok)),
                    ),
                    Fact::new("path", &paint(DIM, &path.display().to_string())),
                ],
            ) {
                output.line(&line);
            }
        }
    }
}

/// Print one action outcome: the plain line when piped, a one-row panel
/// when rich. `plain` is today's exact bytes (AC-1/AC-8).
fn print_action(
    subject: &str,
    plain: &str,
    state: &str,
    glyph: ActionGlyph,
    output: &mut Output,
    style: Style,
) {
    match style {
        Style::Plain => output.line(plain),
        Style::Rich => {
            for line in fact_panel(
                subject,
                &[Fact::new(
                    "state",
                    &format!("{} {state}", status_glyph(glyph)),
                )],
            ) {
                output.line(&line);
            }
        }
    }
}

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

fn update(
    check: bool,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let asset = release_asset()?;
    let tag = runtime.latest_release_tag()?;

    if !tag_is_newer(&tag, current) {
        match style {
            Style::Plain => output.line(&format!("pengepul {current} is the latest release")),
            Style::Rich => {
                for line in fact_panel(
                    "update",
                    &[
                        Fact::new(
                            "state",
                            &format!("{} latest", status_glyph(ActionGlyph::Ok)),
                        ),
                        Fact::new("version", &paint(BOLD, current)),
                    ],
                ) {
                    output.line(&line);
                }
            }
        }
        return Ok(());
    }
    if check {
        let plain = format!(
            "pengepul {tag} is available (running {current}); run `pengepul update` to install it"
        );
        match style {
            Style::Plain => output.line(&plain),
            Style::Rich => {
                for line in fact_panel(
                    "update",
                    &[
                        Fact::new(
                            "state",
                            &format!("{} available", status_glyph(ActionGlyph::Attention)),
                        ),
                        Fact::new("running", current),
                        Fact::new("version", &paint(BOLD, &tag)),
                    ],
                ) {
                    output.line(&line);
                }
            }
        }
        return Ok(());
    }

    let path = runtime.install_release(&tag, asset)?;
    let plain = format!("updated to {tag} at {}", path.display());
    match style {
        Style::Plain => output.line(&plain),
        Style::Rich => {
            for line in fact_panel(
                "update",
                &[
                    Fact::new(
                        "state",
                        &format!("{} updated", status_glyph(ActionGlyph::Ok)),
                    ),
                    Fact::new("version", &paint(BOLD, &tag)),
                    Fact::new("path", &paint(DIM, &path.display().to_string())),
                ],
            ) {
                output.line(&line);
            }
        }
    }
    Ok(())
}

fn login(
    env: CommandEnv<'_>,
    provider: &str,
    key: Option<&str>,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let config = env.load()?;
    // anthropic and codex are always valid; anything else must name a configured
    // provider, and only a configured provider may carry a key.
    if let Ok(builtin) = provider.parse::<ProviderId>() {
        if key.is_some() {
            bail!("{builtin} uses OAuth; --key is for configured providers");
        }
        let email = runtime.login(&config, builtin.clone(), key)?;
        print_login_saved(&builtin.to_string(), &email, output, style);
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
    print_login_saved(provider, &label, output, style);
    Ok(())
}

/// The login outcome: the plain line when piped, a `login: <provider>`
/// panel when rich.
fn print_login_saved(provider: &str, label: &str, output: &mut Output, style: Style) {
    match style {
        Style::Plain => output.line(&format!("saved {provider} account token for {label}")),
        Style::Rich => {
            for line in fact_panel(
                &format!("login {provider}"),
                &[
                    Fact::new("state", &format!("{} saved", status_glyph(ActionGlyph::Ok))),
                    Fact::new("account", &paint(BOLD, label)),
                ],
            ) {
                output.line(&line);
            }
        }
    }
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

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}
