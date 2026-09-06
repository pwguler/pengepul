use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::Value;

use crate::config::{Config, load_config, register_provider, selected_config_path};
pub use crate::render::Style;
use crate::render::{ActionGlyph, BOLD, DIM, Fact, Output, fact_panel, paint, status_glyph};
use crate::service::service_status_panel;
use crate::tokens::save_token;
use crate::types::{ProviderId, ProviderKind, TokenData};
use crate::usage_view::{
    Connection, print_accounts, print_pool_rich, print_relay_total_plain, print_relay_total_rich,
    print_trend_plain, print_trend_rich,
};
use crate::utils::{local_today, sha256_hex};

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

/// The process `launch` becomes: the harness binary, its arguments, and the
/// variables that point it at the relay. Everything the verb decided, so the
/// runtime decides nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
    /// Added to the operator's environment rather than replacing it: the
    /// harness keeps its own configuration, and only its upstream moves.
    pub env: Vec<(String, String)>,
    /// How to install `program`, for the one failure the runtime can name
    /// better than the operating system does.
    pub install_hint: String,
}

/// One row of the model picker: an id and the two facts that tell two ids
/// apart. The facts stay numbers — rendering them is the picker's business,
/// and it is the only thing that knows how wide a column may be. `None`
/// where the catalog does not carry them, which is not the same as zero.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    pub id: String,
    pub context_window: Option<u64>,
    pub price: Option<ModelPrice>,
}

/// What a million tokens costs, in and out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
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

    /// Whether there is an operator to put a question to. Separate from
    /// `stdout_is_tty` because the picker paints to stderr and reads
    /// stdin, and those are what must be terminals for it to work.
    fn can_ask(&mut self) -> bool;

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

    /// Replace this process with the harness the plan names.
    ///
    /// # Errors
    ///
    /// Returns an error if the program cannot be run. It does not return
    /// when the program starts: the harness has the process from there on.
    fn launch(&mut self, plan: &LaunchPlan) -> Result<()>;

    /// The relay's advertised model catalog.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    fn models(&mut self, base_url: &str, api_key: &str) -> Result<Value>;

    /// Let the operator move through `choices` and pick one, typing to
    /// narrow the list. `harness` is what the picker says it is launching,
    /// and `style` decides whether it paints in colour — the same decision
    /// every other surface takes, made once at the edge. `Ok(None)` means
    /// they cancelled.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal cannot be driven.
    fn select_model(
        &mut self,
        harness: &str,
        choices: &[ModelChoice],
        style: Style,
    ) -> Result<Option<String>>;
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
        /// register a new OpenAI-compatible provider at this URL; needs --key
        #[arg(long = "base-url")]
        base_url: Option<String>,
    },
    /// run a coding harness on the relay
    Launch {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
        /// harness to run
        #[arg(value_enum)]
        harness: Harness,
        /// model the harness runs on; required for pi
        #[arg(long)]
        model: Option<String>,
        /// arguments forwarded to the harness, after `--`
        #[arg(last = true)]
        forwarded: Vec<String>,
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
    /// show the last 30 days of token usage
    Usage {
        #[arg(long = "config")]
        command_config: Option<PathBuf>,
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

/// The harnesses `launch` knows how to point at the relay. Each one is a
/// table entry in `launch_plan`: a binary, the variables that redirect it,
/// and how it is told which model to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Harness {
    /// Claude Code
    Claude,
    /// pi
    Pi,
}

impl Harness {
    /// The name the operator typed, for the surfaces that name it back.
    const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
        }
    }
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
        Some(Command::Usage { command_config }) => {
            usage(
                root_env.with_override(command_config.as_deref()),
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
        Some(Command::Launch {
            command_config,
            harness,
            model,
            forwarded,
        }) => {
            launch(
                root_env.with_override(command_config.as_deref()),
                harness,
                model.as_deref(),
                &forwarded,
                runtime,
                style,
            )?;
        }
        Some(Command::Login {
            command_config,
            provider,
            key,
            base_url,
        }) => {
            login(
                root_env.with_override(command_config.as_deref()),
                &provider,
                key.as_deref(),
                base_url.as_deref(),
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

/// The usage trend: the relay's daily buckets, summed across every pool.
/// `today` is sampled once here so the renderer stays clock-free
/// (usage-trend AC-10).
fn usage(
    env: CommandEnv<'_>,
    runtime: &mut impl CliRuntime,
    output: &mut Output,
    style: Style,
) -> Result<()> {
    let config = env.load()?;
    let base_url = base_url(&config);
    let accounts = runtime.accounts(&base_url, &first_api_key(&config)?)?;
    let today = local_today();
    match style {
        Style::Plain => print_trend_plain(&accounts, output, &today),
        Style::Rich => print_trend_rich(&accounts, output, &today),
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
    base_url: Option<&str>,
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
        // AC-6: a built-in's endpoint is fixed, so there is nothing to
        // register.
        if base_url.is_some() {
            bail!("{builtin} uses OAuth; --base-url is for configured providers");
        }
        let email = runtime.login(&config, builtin.clone(), key)?;
        print_login_saved(&builtin.to_string(), &email, false, output, style);
        return Ok(());
    }
    // AC-2: registering without a credential leaves a provider that has
    // no account, which is the half-done state this flag exists to avoid.
    if base_url.is_some() && key.is_none() {
        bail!("--base-url registers {provider} and needs --key to be usable");
    }
    // Every rule about the id, the URL and a conflicting registration
    // lives in `register_provider`, which now runs before `save_token`
    // and refuses without writing. `login` used to carry its own copies
    // because the credential was written first; two of them drifted, and
    // that was rounds 5 and 6.
    if base_url.is_none() && !config.providers.contains_key(provider) {
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
    // Pre-existing on main, fixed here because this function's guards are
    // already this branch's: an empty key saved a credential with no
    // secret in it, which then joined rotation and failed every request
    // it was handed.
    if key.trim().is_empty() {
        bail!("{provider} takes a static API key; --key is empty");
    }
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
    // The token first: if the config write then fails, what is left is an
    // orphan token in the auth-dir, which is harmless because the
    // provider is still unconfigured. The other order leaves a registered
    // provider with no account (AC-9).
    // What the rollback below is allowed to take back: only a file this
    // command created. The label is `key-<hash of the key>`, so
    // re-running the same command overwrites the same filename, and
    // rolling that back would destroy a credential that predates the
    // command.
    //
    // The question is about the filesystem, so it is asked of the
    // filesystem. An earlier version asked whether the provider was in
    // the config, which disagrees whenever a pool outlives its config
    // entry — the normal state after a hand-removal, since removing a
    // provider is not a verb this tool has.
    // Config first, credential second.
    //
    // The other order needs a rollback: a credential written before a
    // failed registration has to be taken back, which means deciding
    // whether this command created that file. Four review rounds found a
    // different wrong answer to that question — the config disagreeing
    // with the filesystem, a pool that outlived its entry, a rollback
    // that deleted what it had only overwritten.
    //
    // This way the undo is restoring bytes already in hand. A registered
    // provider whose credential never arrived is visible in `accounts`,
    // costs nothing, and is fixed by re-running the command; a credential
    // in a pool the config never gained is invisible and joins rotation.
    let mut registered = false;
    let mut restore: Option<(PathBuf, String)> = None;
    if let Some(base_url) = base_url {
        // The path `env.load()` read. With a legacy config that load has
        // just migrated it to the home path, so this resolves there — the
        // file the next load will read, not the shadowed original.
        let path = selected_config_path(env.config_path, Some(env.home), env.cwd);
        // The bytes `register_provider` read under its own lock, not a
        // read of our own: reading here would capture the file before a
        // racing registration and undo theirs along with ours.
        let before = register_provider(&path, provider, base_url)?;
        restore = Some((path, before));
        registered = true;
    }
    if let Err(error) = save_token(&config.auth_dir, &token) {
        if let Some((path, before)) = restore {
            // Bytes this command read moments ago, under the same lock
            // `register_provider` took. Nothing is deduced about what a
            // file means.
            if let Err(undo) = fs::write(&path, before) {
                return Err(error.context(format!(
                    "left {provider} registered in {} and could not undo it: {undo}",
                    path.display()
                )));
            }
        }
        return Err(error);
    }
    print_login_saved(provider, &label, registered, output, style);
    Ok(())
}

/// The login outcome: the plain line when piped, a `login: <provider>`
/// panel when rich.
/// AC-11: a registration is a fact of the login it arrived with, not an
/// event of its own. Plain gets its own parseable line; rich gets a row
/// inside the existing panel, because a bare line above a 64-column box
/// is neither the panel language nor a second panel (CONTEXT.md, Panel).
fn print_login_saved(
    provider: &str,
    label: &str,
    registered: bool,
    output: &mut Output,
    style: Style,
) {
    match style {
        Style::Plain => {
            if registered {
                output.line(&format!("registered {provider}"));
            }
            output.line(&format!("saved {provider} account token for {label}"));
        }
        Style::Rich => {
            let mut facts = vec![Fact::new(
                "state",
                &format!("{} saved", status_glyph(ActionGlyph::Ok)),
            )];
            if registered {
                facts.push(Fact::new("registered", &paint(BOLD, provider)));
            }
            facts.push(Fact::new("account", &paint(BOLD, label)));
            for line in fact_panel(&format!("login {provider}"), &facts) {
                output.line(&line);
            }
        }
    }
}

/// The provider name the pi extension registers the relay under, and the
/// package that registers it. pi resolves `--provider` against what its
/// extensions declared, so without the package a launch dies at pi's own
/// provider lookup — loudly, which is why nothing here checks for it first.
const PI_PROVIDER: &str = "pengepul";
const PI_PROVIDER_PACKAGE: &str = "npm:@pwguler/pi-pengepul-provider";

/// Run a harness on the relay.
///
/// Nothing persists. The harness is handed an environment and an argument
/// list for one process, so the same binary started without `launch` still
/// finds its own accounts and its own models.
fn launch(
    env: CommandEnv<'_>,
    harness: Harness,
    model: Option<&str>,
    forwarded: &[String],
    runtime: &mut impl CliRuntime,
    style: Style,
) -> Result<()> {
    let config = env.load()?;
    let base_url = base_url(&config);
    let api_key = first_api_key(&config)?;
    // The relay is the whole of what this verb hands over, so a dead one is
    // its refusal to make. Left to the harness the same fact arrives as a
    // connection error inside a TUI, several screens from anything that
    // names pengepul.
    runtime.health(&base_url).with_context(|| {
        format!(
            "relay is not answering at {base_url}; start it with `pengepul serve` \
             or `pengepul service start`"
        )
    })?;
    let chosen = match model {
        Some(model) => Some(model.to_string()),
        // Nobody named a model, so offer the ones the relay actually
        // serves. With no terminal there is nobody to ask, and raw mode
        // would seize one nobody is watching, so the picker is skipped and
        // each harness does what it does with no model at all.
        None if runtime.can_ask() => {
            let catalog = runtime.models(&base_url, &api_key)?;
            let choices = model_choices(&catalog);
            if choices.is_empty() {
                None
            } else {
                match runtime.select_model(harness.name(), &choices, style)? {
                    Some(model) => Some(model),
                    // Backing out of the picker cancels the command. It used
                    // to fall through and start claude on its own default,
                    // which is not what the footer offered.
                    None => return Ok(()),
                }
            }
        }
        None => None,
    };
    let plan = launch_plan(harness, &base_url, &api_key, chosen.as_deref(), forwarded)?;
    runtime.launch(&plan)
}

/// Every model the relay advertises, in the order it advertises them.
/// Every one of them is a fair choice for either harness: the relay serves
/// Messages from a configured endpoint by translating onto its Chat
/// Completions dialect, so what is on the list is what can be run.
fn model_choices(catalog: &Value) -> Vec<ModelChoice> {
    let Some(entries) = catalog.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?;
            // A carriage return or an escape in an id repaints the row it
            // sits on: the list would show one model and enter would launch
            // another, with nothing printed afterwards to catch it. No real
            // model id carries one, and the catalog is a remote document.
            if id.chars().any(char::is_control) {
                return None;
            }
            Some(ModelChoice {
                id: id.to_string(),
                context_window: entry.get("context_window").and_then(Value::as_u64),
                price: model_price(entry),
            })
        })
        .collect()
}

/// What a million tokens costs at this model, when the catalog says. A
/// configured endpoint publishes what it publishes, and half a rate is not
/// a price: both halves or nothing.
fn model_price(entry: &Value) -> Option<ModelPrice> {
    let pricing = entry.get("pricing")?;
    Some(ModelPrice {
        input: pricing.get("input_per_million").and_then(Value::as_f64)?,
        output: pricing.get("output_per_million").and_then(Value::as_f64)?,
    })
}

/// Rows whose id carries every word of the filter, case folded. Words
/// narrow together, so `opus 4` finds the 4-series opus ids without naming
/// their exact shape.
pub(crate) fn matching_choices(choices: &[ModelChoice], filter: &str) -> Vec<ModelChoice> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    choices
        .iter()
        .filter(|choice| {
            let id = choice.id.to_lowercase();
            words.iter().all(|word| id.contains(word))
        })
        .cloned()
        .collect()
}

/// What each harness needs to run on the relay instead of its own upstream.
fn launch_plan(
    harness: Harness,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    forwarded: &[String],
) -> Result<LaunchPlan> {
    match harness {
        Harness::Claude => {
            let mut env = vec![
                ("ANTHROPIC_BASE_URL".to_string(), base_url.to_string()),
                ("ANTHROPIC_AUTH_TOKEN".to_string(), api_key.to_string()),
                // Emptied rather than left alone. A key already in the
                // operator's environment outranks the token above, and
                // sends the harness to api.anthropic.com on a per-token
                // meter — the one outcome this verb exists to prevent.
                ("ANTHROPIC_API_KEY".to_string(), String::new()),
            ];
            if let Some(model) = model {
                // Every tier, not only the default one. The operator named
                // one model; a `/model sonnet` that quietly went somewhere
                // else would be the same silence this verb removes.
                for name in [
                    "ANTHROPIC_MODEL",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    "CLAUDE_CODE_SUBAGENT_MODEL",
                ] {
                    env.push((name.to_string(), model.to_string()));
                }
            }
            Ok(LaunchPlan {
                program: "claude".to_string(),
                args: forwarded.to_vec(),
                env,
                install_hint: "npm install -g @anthropic-ai/claude-code".to_string(),
            })
        }
        Harness::Pi => {
            // pi binds `--provider` only when a model comes with it: run
            // with a provider alone it answers from the one in its own
            // settings and says nothing. Without a model this verb would
            // start a pi that looked pointed at the relay and was not.
            let model = model.context(
                "pi needs --model: it binds a provider only together with one, and \
                 without it pi keeps the provider from its own settings",
            )?;
            let mut args = vec![
                "--provider".to_string(),
                PI_PROVIDER.to_string(),
                "--model".to_string(),
                model.to_string(),
            ];
            args.extend_from_slice(forwarded);
            Ok(LaunchPlan {
                program: "pi".to_string(),
                args,
                env: vec![
                    ("PENGEPUL_BASE_URL".to_string(), base_url.to_string()),
                    ("PENGEPUL_API_KEY".to_string(), api_key.to_string()),
                ],
                install_hint: format!(
                    "npm install -g @earendil-works/pi-coding-agent, then \
                     pi install {PI_PROVIDER_PACKAGE}"
                ),
            })
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

#[cfg(test)]
mod tests {
    use super::{ModelChoice, matching_choices};

    fn rows(ids: &[&str]) -> Vec<ModelChoice> {
        ids.iter()
            .map(|id| ModelChoice {
                id: (*id).to_string(),
                context_window: None,
                price: None,
            })
            .collect()
    }

    fn ids(choices: &[ModelChoice]) -> Vec<&str> {
        choices.iter().map(|choice| choice.id.as_str()).collect()
    }

    #[test]
    fn an_empty_filter_keeps_every_row_in_order() {
        let all = rows(&["anthropic/claude-opus-5", "commandcode/z-ai/glm-5.3-flash"]);
        assert_eq!(
            ids(&matching_choices(&all, "")),
            ["anthropic/claude-opus-5", "commandcode/z-ai/glm-5.3-flash"]
        );
    }

    #[test]
    fn a_filter_matches_anywhere_in_the_id_and_ignores_case() {
        let all = rows(&["commandcode/zai-org/GLM-5.3", "anthropic/claude-opus-5"]);
        assert_eq!(
            ids(&matching_choices(&all, "glm")),
            ["commandcode/zai-org/GLM-5.3"]
        );
        assert_eq!(
            ids(&matching_choices(&all, "OPUS")),
            ["anthropic/claude-opus-5"]
        );
    }

    #[test]
    fn words_narrow_together_rather_than_replacing_each_other() {
        // The picker appends what is typed to the filter, so `glm` then
        // `flash` must mean the flash one among the glm rows. An earlier
        // version replaced instead, and widened 6 matches back to 14.
        let all = rows(&[
            "commandcode/z-ai/glm-5.3-flash",
            "commandcode/zai-org/GLM-5.3",
            "commandcode/google/gemini-3.8-flash",
        ]);

        assert_eq!(matching_choices(&all, "glm").len(), 2);
        assert_eq!(matching_choices(&all, "flash").len(), 2);
        assert_eq!(
            ids(&matching_choices(&all, "glm flash")),
            ["commandcode/z-ai/glm-5.3-flash"],
            "two words must intersect, not union"
        );
    }

    #[test]
    fn a_filter_nothing_carries_matches_nothing() {
        let all = rows(&["anthropic/claude-opus-5"]);
        assert!(matching_choices(&all, "zzz").is_empty());
    }
}
