use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::render::{ActionGlyph, Fact, fact_panel, format_duration, status_glyph};
use anyhow::{Context, Result, bail};

pub const SYSTEMD_UNIT_NAME: &str = "pengepul.service";
pub const LAUNCHD_LABEL: &str = "dev.pwguler.pengepul";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceOptions {
    pub executable: PathBuf,
    pub config_path: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<u16>,
}

#[must_use]
pub fn build_service_command(options: &ServiceOptions) -> Vec<String> {
    let mut command = vec![
        options.executable.to_string_lossy().into_owned(),
        "serve".to_string(),
    ];
    if let Some(config_path) = &options.config_path {
        command.push("--config".to_string());
        command.push(config_path.to_string_lossy().into_owned());
    }
    if let Some(host) = &options.host {
        command.push("--host".to_string());
        command.push(host.clone());
    }
    if let Some(port) = options.port {
        command.push("--port".to_string());
        command.push(port.to_string());
    }
    command
}

#[must_use]
pub fn render_systemd_unit(options: &ServiceOptions) -> String {
    // systemd splits ExecStart on whitespace, so an install path containing a
    // space would otherwise arrive as two arguments. Only such arguments are
    // quoted, leaving the common unit file unadorned.
    let exec_start = build_service_command(options)
        .iter()
        .map(|arg| {
            if arg.contains([' ', '"', '\\']) {
                format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=pengepul API relay\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={exec_start}\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n"
    )
}

/// Render a launchd plist payload.
///
/// # Errors
///
/// Returns an error if plist XML serialization fails.
pub fn render_launchd_plist(
    options: &ServiceOptions,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<String> {
    let mut payload = BTreeMap::new();
    payload.insert(
        "Label".to_string(),
        plist::Value::String(LAUNCHD_LABEL.to_string()),
    );
    payload.insert(
        "ProgramArguments".to_string(),
        plist::Value::Array(
            build_service_command(options)
                .into_iter()
                .map(plist::Value::String)
                .collect(),
        ),
    );
    payload.insert("RunAtLoad".to_string(), plist::Value::Boolean(true));
    payload.insert("KeepAlive".to_string(), plist::Value::Boolean(true));
    payload.insert(
        "StandardOutPath".to_string(),
        plist::Value::String(stdout_path.to_string_lossy().into_owned()),
    );
    payload.insert(
        "StandardErrorPath".to_string(),
        plist::Value::String(stderr_path.to_string_lossy().into_owned()),
    );

    let mut buffer = Vec::new();
    plist::Value::Dictionary(payload.into_iter().collect())
        .to_writer_xml(&mut buffer)
        .context("failed to serialize launchd plist")?;
    String::from_utf8(buffer).context("plist serializer produced non-UTF-8")
}

/// Install a user systemd unit and optionally enable/start it.
///
/// # Errors
///
/// Returns an error when the unit cannot be written or a control command fails.
pub fn install_systemd_service(
    options: &ServiceOptions,
    home: &Path,
    start: bool,
    enable: bool,
    mut runner: impl FnMut(&[String]) -> Result<ExitStatus>,
) -> Result<PathBuf> {
    let path = home.join(".config/systemd/user").join(SYSTEMD_UNIT_NAME);
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::write(&path, render_systemd_unit(options))
        .with_context(|| format!("failed to write {}", path.display()))?;

    run(&mut runner, &["systemctl", "--user", "daemon-reload"])?;
    if enable {
        run(
            &mut runner,
            &["systemctl", "--user", "enable", SYSTEMD_UNIT_NAME],
        )?;
    }
    if start {
        run(
            &mut runner,
            &["systemctl", "--user", "start", SYSTEMD_UNIT_NAME],
        )?;
    }
    Ok(path)
}

/// Install a launchd agent plist and optionally bootstrap it.
///
/// # Errors
///
/// Returns an error when the plist cannot be written or a launchctl command fails.
pub fn install_launchd_service(
    options: &ServiceOptions,
    home: &Path,
    uid: u32,
    start: bool,
    mut runner: impl FnMut(&[String]) -> Result<ExitStatus>,
) -> Result<PathBuf> {
    let path = home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"));
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let logs = home.join(".pengepul/logs");
    fs::create_dir_all(&logs).with_context(|| format!("failed to create {}", logs.display()))?;
    let payload = render_launchd_plist(
        options,
        &logs.join("service.log"),
        &logs.join("service.err.log"),
    )?;
    fs::write(&path, payload).with_context(|| format!("failed to write {}", path.display()))?;

    // launchd has no daemon-reload. A service must be bootstrapped (loaded) before
    // kickstart, bootout or print will find it, and bootstrapping one that is
    // already loaded fails, so an existing copy is unloaded first. Without this,
    // every later `service` command reports "Could not find service".
    let domain = format!("gui/{uid}");
    let target = format!("{domain}/{LAUNCHD_LABEL}");
    let _ = runner(&["launchctl", "bootout", &target].map(String::from));
    run(
        &mut runner,
        &["launchctl", "bootstrap", &domain, &path.to_string_lossy()],
    )?;
    if start {
        run(&mut runner, &["launchctl", "kickstart", &target])?;
    }
    Ok(path)
}

/// Execute a service manager command.
///
/// # Errors
///
/// Returns an error if the command cannot be spawned or exits unsuccessfully.
pub fn run_command(command: &[String]) -> Result<ExitStatus> {
    let Some((program, args)) = command.split_first() else {
        bail!("empty command");
    };
    // Captured, not inherited: several of these commands are run for their effect
    // and their failure tolerated (booting out a service that was never loaded),
    // so their chatter must not reach the terminal. A failure that is *not*
    // tolerated carries the captured stderr instead of a bare exit code.
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        if detail.is_empty() {
            bail!("{program} exited with {}", output.status);
        }
        bail!("{program} exited with {}: {detail}", output.status);
    }
    Ok(output.status)
}

fn run(runner: &mut impl FnMut(&[String]) -> Result<ExitStatus>, command: &[&str]) -> Result<()> {
    let command = command.iter().map(ToString::to_string).collect::<Vec<_>>();
    let status = runner(&command)?;
    if !status.success() {
        bail!("{} exited with {}", command.join(" "), status);
    }
    Ok(())
}

/// Parse the platform service tool's text (systemctl on Linux, launchctl
/// on macOS) into the structured `service` panel. Unrecognized lines are
/// dropped; a text with no recognizable state renders as unknown rather
/// than failing — the panel is observability, never a gate.
pub(crate) fn service_status_panel(text: &str) -> Vec<String> {
    let mut state: Option<String> = None;
    let mut enabled: Option<String> = None;
    let mut since: Option<String> = None;
    let mut pid: Option<String> = None;
    let mut memory: Option<String> = None;
    let mut cpu: Option<String> = None;
    let mut tasks: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        // launchctl key = value pairs.
        if let Some((key, value)) = trimmed.split_once(" = ") {
            match key {
                "state" => state = Some(value.to_string()),
                "pid" => pid = Some(value.to_string()),
                _ => {}
            }
            continue;
        }
        // systemctl `Label: value` pairs.
        if let Some((key, value)) = trimmed.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "Active" => {
                    // `active (running) since Sat 2026-09-05 04:09:31 WIB; 4min 28s ago`
                    let mut fields = value.splitn(2, " since ");
                    let active = fields.next().unwrap_or(value).trim();
                    state = Some(active.to_string());
                    if let Some((_, ago)) = fields.next().and_then(|rest| rest.split_once("; ")) {
                        let elapsed = parse_relative_seconds(ago);
                        if elapsed > 0.0 {
                            since = Some(format_duration(elapsed));
                        }
                    }
                }
                "Loaded" => {
                    if let Some((_, rest)) = value.split_once("; ") {
                        let flag = rest.split(';').next().unwrap_or("").trim();
                        enabled = Some(flag.to_string());
                    } else {
                        enabled = Some(value.split(';').next().unwrap_or(value).trim().to_string());
                    }
                }
                "Main PID" => {
                    pid = Some(value.split_whitespace().next().unwrap_or(value).to_string());
                }
                "Memory" => {
                    memory = Some(value.split_whitespace().next().unwrap_or(value).to_string());
                }
                "CPU" => {
                    cpu = Some(value.split_whitespace().next().unwrap_or(value).to_string());
                }
                "Tasks" => {
                    tasks = Some(value.split_whitespace().next().unwrap_or(value).to_string());
                }
                _ => {}
            }
        }
    }

    let state_text = state.clone().unwrap_or_else(|| "unknown".to_string());
    let active = state_text.starts_with("active") || state_text == "running";
    let glyph = if active {
        status_glyph(ActionGlyph::Ok)
    } else if state_text.starts_with("failed") {
        status_glyph(ActionGlyph::Failed)
    } else {
        status_glyph(ActionGlyph::Attention)
    };
    let mut facts = vec![Fact::new("state", &format!("{glyph} {state_text}"))];
    if let Some(enabled) = enabled {
        facts.push(Fact::new("enabled", &enabled));
    }
    // systemd keeps printing the last Main PID of a dead unit
    // (`code=killed`); a pid row for a stopped service would mislead.
    if let (Some(pid), true) = (pid, active) {
        facts.push(Fact::new("pid", &pid));
    }
    if let Some(memory) = memory {
        facts.push(Fact::new("memory", &memory));
    }
    if let Some(cpu) = cpu {
        facts.push(Fact::new("cpu", &cpu));
    }
    if let Some(tasks) = tasks {
        facts.push(Fact::new("tasks", &tasks));
    }
    if let Some(since) = since {
        // The same "since" reads as uptime for a running unit and as
        // downtime for a stopped one.
        if active {
            facts.push(Fact::new("uptime", &since));
        } else {
            facts.push(Fact::new("stopped", &format!("{since} ago")));
        }
    }
    // The header carries the state as its qualifier (AC-4); the rows
    // carry the detail.
    let subject = state_text.split_whitespace().next().map_or_else(
        || "service".to_string(),
        |word| format!("service \u{2500} {word}"),
    );
    fact_panel(&subject, &facts)
}

/// Parse systemd's relative time ("4min 28s ago", "3 days ago",
/// "1 day 5h ago", "2 weeks 1 day ago", "500ms ago") into seconds. Units
/// may sit against the number or in the next word. Unknown units drop
/// their number; unparsable text yields 0.
pub(crate) fn parse_relative_seconds(text: &str) -> f64 {
    let mut seconds = 0.0;
    let mut pending: Option<f64> = None;
    for token in text.split_whitespace() {
        let digits: String = token
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let unit = &token[digits.len()..];
        if digits.is_empty() {
            if let Some(value) = pending.take() {
                seconds += value * unit_seconds(unit);
            }
            continue;
        }
        let value: f64 = digits.parse().unwrap_or(0.0);
        if unit.is_empty() {
            pending = Some(value);
        } else {
            seconds += value * unit_seconds(unit);
        }
    }
    seconds
}

/// Seconds per unit word, singular or plural; sub-second units and
/// unknown words contribute nothing.
pub(crate) fn unit_seconds(unit: &str) -> f64 {
    match unit.trim_end_matches([',', ';']) {
        "s" | "sec" | "second" | "seconds" => 1.0,
        "min" | "minute" | "minutes" | "m" => 60.0,
        "h" | "hour" | "hours" => 3600.0,
        "d" | "day" | "days" => 86_400.0,
        "w" | "week" | "weeks" => 604_800.0,
        "month" | "months" => 30.0 * 86_400.0,
        "y" | "year" | "years" => 365.0 * 86_400.0,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_relative_seconds;
    use crate::render::format_duration;
    #[test]
    fn relative_seconds_reads_every_systemd_shape() {
        // Whole seconds: compare through the exact-integer formatter.
        let seconds = |text: &str| format_duration(parse_relative_seconds(text));
        assert_eq!(seconds("4min 28s ago"), "4m28s");
        assert_eq!(seconds("3 days ago"), "3d0h");
        assert_eq!(seconds("1 day 5h ago"), "1d5h");
        assert_eq!(seconds("2 weeks 1 day ago"), "15d0h");
        assert_eq!(seconds("1 month 3 days ago"), "33d0h");
        // Sub-second units never masquerade as minutes.
        assert_eq!(seconds("500ms ago"), "0s");
        assert_eq!(seconds("nonsense"), "0s");
    }
}
