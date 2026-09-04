use std::path::{Path, PathBuf};

use anyhow::Result;
use pengepul::cli::{CliRuntime, RunOutcome, ServiceInstallRequest, Style, run_with_env};
use pengepul::config::Config;
use pengepul::types::ProviderId;
use serde_json::{Value, json};
use tempfile::tempdir;

fn write_config(home: &Path, host: &str, port: u16) {
    let config_dir = home.join(".pengepul");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.yaml"),
        format!("host: \"{host}\"\nport: {port}\nauth-dir: ~/.pengepul\napi-keys:\n  - sk-test\n"),
    )
    .expect("write config");
}

#[derive(Default)]
struct FakeRuntime {
    service_status_text: Option<String>,
    service_status_error: Option<String>,
    server_host: Option<String>,
    server_port: Option<u16>,
    health_url: Option<String>,
    accounts_url: Option<String>,
    accounts_api_key: Option<String>,
    calls: Vec<String>,
    install_request: Option<ServiceInstallRequest>,
    login_provider: Option<ProviderId>,
    latest_tag: Option<String>,
    installed: Option<(String, String)>,
    accounts_payload: Option<Value>,
    rich: bool,
}

impl CliRuntime for FakeRuntime {
    fn latest_release_tag(&mut self) -> Result<String> {
        Ok(self
            .latest_tag
            .clone()
            .unwrap_or_else(|| "v0.0.1".to_string()))
    }

    fn install_release(&mut self, tag: &str, asset: &str) -> Result<std::path::PathBuf> {
        self.installed = Some((tag.to_string(), asset.to_string()));
        Ok(std::path::PathBuf::from("/usr/local/bin/pengepul"))
    }

    fn run_server(&mut self, config: &Config) -> Result<()> {
        self.server_host = Some(config.host.clone());
        self.server_port = Some(config.port);
        Ok(())
    }

    fn health(&mut self, base_url: &str) -> Result<Value> {
        self.health_url = Some(base_url.to_string());
        Ok(json!({"status": "ok"}))
    }

    fn accounts(&mut self, base_url: &str, api_key: &str) -> Result<Value> {
        self.calls.push(format!("accounts:{base_url}:{api_key}"));
        self.accounts_url = Some(base_url.to_string());
        self.accounts_api_key = Some(api_key.to_string());
        if let Some(payload) = &self.accounts_payload {
            return Ok(payload.clone());
        }
        Ok(json!({
            "providers": {
                "anthropic": {
                    "account_count": 1,
                    "accounts": [{
                        "email": "anthropic@example.com",
                        "available": true,
                        "failureCount": 0,
                        "planType": null
                    }]
                },
                "codex": {"account_count": 2, "accounts": []}
            }
        }))
    }

    fn stdout_is_tty(&mut self) -> bool {
        self.rich
    }

    fn reload_accounts(&mut self, base_url: &str, api_key: &str) -> Result<Value> {
        self.calls.push(format!("reload:{base_url}:{api_key}"));
        Ok(json!({"reloaded": {"anthropic": {"added": [], "updated": [], "unchanged": []}}}))
    }

    fn install_service(&mut self, request: ServiceInstallRequest) -> Result<PathBuf> {
        self.install_request = Some(request);
        Ok("/tmp/pengepul.service".into())
    }

    fn start_service(&mut self) -> Result<()> {
        self.calls.push("service:start".to_string());
        Ok(())
    }

    fn stop_service(&mut self) -> Result<()> {
        self.calls.push("service:stop".to_string());
        Ok(())
    }

    fn restart_service(&mut self) -> Result<()> {
        self.calls.push("service:restart".to_string());
        Ok(())
    }

    fn service_status(&mut self) -> Result<String> {
        self.calls.push("service:status".to_string());
        if let Some(error) = &self.service_status_error {
            return Err(anyhow::anyhow!(error.clone()));
        }
        Ok(self
            .service_status_text
            .clone()
            .unwrap_or_else(|| "active".to_string()))
    }

    fn uninstall_service(&mut self) -> Result<PathBuf> {
        self.calls.push("service:uninstall".to_string());
        Ok("/tmp/pengepul.service".into())
    }

    fn service_logs(&mut self, follow: bool, lines: u32) -> Result<()> {
        self.calls
            .push(format!("service:logs:follow={follow}:lines={lines}"));
        Ok(())
    }

    fn login(
        &mut self,
        _config: &Config,
        provider: ProviderId,
        _key: Option<&str>,
    ) -> Result<String> {
        let email = format!("{provider}@example.com");
        self.login_provider = Some(provider);
        Ok(email)
    }
}

fn run(argv: &[&str], home: &Path, runtime: &mut impl CliRuntime) -> RunOutcome {
    // Default test path is Plain; the rich tests call run_style explicitly.
    run_with_env(argv, home, home, runtime, Style::Plain).expect("cli run")
}

fn run_style(
    argv: &[&str],
    home: &Path,
    runtime: &mut impl CliRuntime,
    style: Style,
) -> RunOutcome {
    run_with_env(argv, home, home, runtime, style).expect("cli run")
}

#[test]
fn default_command_starts_server() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime::default();

    let outcome = run(&[], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    assert_eq!(runtime.server_host.as_deref(), Some("0.0.0.0"));
    assert_eq!(runtime.server_port, Some(8318));
    assert!(outcome.stderr.is_empty());
}

#[test]
fn top_level_help_uses_subcommands() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["help"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    assert!(!outcome.stdout.contains("--login"));
    assert!(!outcome.stdout.contains("--host HOST"));
    assert!(!outcome.stdout.contains("--port PORT"));
    assert!(outcome.stdout.contains("login"));
    assert!(outcome.stdout.contains("serve"));
    assert!(outcome.stderr.is_empty());
}

#[test]
fn help_command_prints_nested_help() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["help", "service", "install"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    assert!(
        outcome
            .stdout
            .starts_with("Usage: pengepul service install"),
        "{}",
        outcome.stdout
    );
    assert!(outcome.stdout.contains("--enable"));
    assert!(outcome.stderr.is_empty());
}

#[test]
fn serve_subcommand_starts_server_with_custom_host_port() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    let outcome = run(
        &["serve", "--host", "0.0.0.0", "--port", "9000"],
        tmp.path(),
        &mut runtime,
    );

    assert_eq!(outcome.code, 0);
    assert_eq!(runtime.server_host.as_deref(), Some("0.0.0.0"));
    assert_eq!(runtime.server_port, Some(9000));
}

#[test]
fn config_commands_print_path_and_api_key() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    let path = run(&["config", "path"], tmp.path(), &mut runtime);
    assert_eq!(
        path.stdout.trim(),
        tmp.path().join(".pengepul/config.yaml").to_string_lossy()
    );

    let api_key = run(&["config", "api-key"], tmp.path(), &mut runtime);
    assert_eq!(api_key.stdout.trim(), "sk-test");
}

#[test]
fn config_path_does_not_generate_config() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["config", "path"], tmp.path(), &mut runtime);

    assert_eq!(
        outcome.stdout.trim(),
        tmp.path().join(".pengepul/config.yaml").to_string_lossy()
    );
    assert!(!tmp.path().join(".pengepul/config.yaml").exists());
}

#[test]
fn status_reports_health_and_account_counts() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["status"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    // Header facts moved into the relay block (rich-everywhere AC-7).
    assert!(
        outcome
            .stdout
            .contains("url http://127.0.0.1:8318 \u{2014} server ok")
    );

    assert!(outcome.stdout.contains("anthropic: 1 account"));
    // Empty pools are hidden (AC-7, revised): codex has no loaded accounts.
    assert!(!outcome.stdout.contains("codex:"));
    assert_eq!(runtime.health_url.as_deref(), Some("http://127.0.0.1:8318"));
    assert_eq!(runtime.accounts_api_key.as_deref(), Some("sk-test"));
}

fn account(json: Value) -> Value {
    json
}

/// Strip ANSI escape sequences, leaving the visible text — assertions about
/// layout run on this, assertions about color run on the raw bytes.
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

/// An absolute instant `seconds` from now, for `cooldownUntil` fixtures.
fn soon(seconds: f64) -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
        + seconds
}

#[test]
fn status_rolls_up_pool_health_and_token_totals_per_provider() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime {
        accounts_payload: Some(json!({
            "providers": {
                "anthropic": {
                    "account_count": 3,
                    "accounts": [
                        account(json!({
                            "email": "a@x.com",
                            "available": true,
                            "failureCount": 0,
                            "totalRequests": 640,
                            "totalSuccesses": 638,
                            "totalFailures": 2,
                            "totalInputTokens": 22_100_000,
                            "totalOutputTokens": 401_200,
                            "totalCacheCreationInputTokens": 6_000_000,
                            "totalCacheReadInputTokens": 155_000_000,
                            "totalReasoningOutputTokens": 64_000,
                            "planType": "max"
                        })),
                        account(json!({
                            "email": "b@x.com",
                            "available": true,
                            "failureCount": 4,
                            "totalRequests": 564,
                            "totalSuccesses": 560,
                            "totalFailures": 4,
                            "totalInputTokens": 23_100_000,
                            "totalOutputTokens": 411_100,
                            "totalCacheCreationInputTokens": 6_400_000,
                            "totalCacheReadInputTokens": 156_700_000,
                            "totalReasoningOutputTokens": 32_000,
                            "planType": "pro"
                        })),
                        account(json!({
                            "email": "c@x.com",
                            "available": false,
                            "cooldownUntil": soon(252.9),
                            "failureCount": 9,
                            "planType": "pro"
                        }))
                    ]
                },
                "groq": {
                    "account_count": 1,
                    "accounts": [
                        account(json!({
                            "email": "g@x.com",
                            "available": true,
                            "failureCount": 0,
                            "planType": null
                        }))
                    ]
                },
                "deepseek": {
                    "account_count": 0,
                    "accounts": []
                }
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run(&["status"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    assert!(
        outcome
            .stdout
            .contains("anthropic: 3 accounts (2 available)")
    );
    assert!(outcome.stdout.contains("on cooldown"));
    assert!(
        outcome
            .stdout
            .contains("requests 1,204  (1,198 ok, 6 failed)")
    );
    assert!(
        outcome
            .stdout
            .contains("tokens in 45.2M  out 812.3K  cache 324.1M  reasoning 96.0K")
    );
    // A provider whose account omits every total rolls up as zeros (AC-4).
    assert!(outcome.stdout.contains("groq: 1 account (1 available)"));
    assert!(outcome.stdout.contains("requests 0  (0 ok, 0 failed)"));
    assert!(
        outcome
            .stdout
            .contains("tokens in 0  out 0  cache 0  reasoning 0")
    );
    // An empty pool is hidden entirely, not even a bare header (AC-7,
    // revised: the user asked for empty pools not to be shown).
    assert!(!outcome.stdout.contains("deepseek"));
    // Each rollup block opens on its own line after a blank one (AC-1).
    assert!(
        outcome
            .stdout
            .starts_with("anthropic: 3 accounts (2 available")
    );
}

#[test]
fn status_renders_panels_on_a_tty() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime {
        rich: true,
        accounts_payload: Some(json!({
            "providers": {
                "anthropic": {
                    "account_count": 3,
                    "accounts": [
                        account(json!({
                            "email": "a@x.com",
                            "available": true,
                            "failureCount": 0,
                            "totalRequests": 640,
                            "totalSuccesses": 638,
                            "totalInputTokens": 22_100_000,
                            "totalOutputTokens": 401_200,
                            "totalCacheCreationInputTokens": 6_000_000,
                            "totalCacheReadInputTokens": 155_000_000,
                            "totalReasoningOutputTokens": 64_000,
                            "planType": "max"
                        })),
                        account(json!({
                            "email": "b@x.com",
                            "available": false,
                            "cooldownUntil": soon(252.9),
                            "failureCount": 2,
                            "planType": "pro"
                        }))
                    ]
                },
                "deepseek": {"account_count": 0, "accounts": []}
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run_style(&["status"], tmp.path(), &mut runtime, Style::Rich);

    assert_eq!(outcome.code, 0);
    let stdout = outcome.stdout.clone();
    let visible = strip_ansi(&stdout);
    // Color is present in the raw bytes: green glyph, amber cooldown glyph,
    // bold numbers (AC-5).
    assert!(stdout.contains("\x1b[32m●\x1b[0m"));
    assert!(stdout.contains("\x1b[33m●\x1b[0m"));
    assert!(stdout.contains("\x1b[1m"));
    // Panel rules with the pool header inside the top rule (AC-3).
    assert!(visible.contains("┌─ pool: anthropic"));
    assert!(visible.contains("─ 3 accounts, 1 available"));
    assert!(visible.contains('└'));
    // Rows: email, glyph, state, ok count, share bar, percentage (AC-3/5).
    assert!(visible.contains('│'));
    assert!(visible.contains("a@x.com"));
    assert!(visible.contains("● available"));
    assert!(visible.contains("638 ok"));
    // Row state drops the plain branch's leading "on"; the glyph carries it.
    assert!(visible.contains("● cooldown 4m12s"));
    assert!(visible.contains("100%"));
    assert!(visible.contains("█"));
    assert!(visible.contains("░"));
    // Footer carries this fixture's summed rollup (AC-3).
    assert!(visible.contains("requests 640"));
    assert!(visible.contains("tokens in 22.1M"));
    assert!(visible.contains("reasoning 64.0K"));
    // Empty pool is hidden entirely, not a note and not a box (AC-3,
    // revised: the user asked for empty pools not to be shown).
    assert!(!visible.contains("deepseek"));
    // Every panel line fits the fixed width, ANSI excluded (AC-3/AC-7).
    for line in visible.lines() {
        assert!(line.chars().count() <= 64, "panel line too wide: {line}");
    }
}

#[test]
fn accounts_renders_panels_with_detail_lines_on_a_tty() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime {
        rich: true,
        accounts_payload: Some(json!({
            "providers": {
                "anthropic": {
                    "account_count": 2,
                    "accounts": [
                        account(json!({
                            "email": "a@x.com",
                            "available": true,
                            "failureCount": 0,
                            "totalRequests": 640,
                            "totalSuccesses": 638,
                            "totalInputTokens": 22_100_000,
                            "totalOutputTokens": 401_200,
                            "totalCacheCreationInputTokens": 6_000_000,
                            "totalCacheReadInputTokens": 155_000_000,
                            "totalReasoningOutputTokens": 64_000,
                            "planType": "max"
                        })),
                        account(json!({
                            "email": "b@x.com",
                            "available": false,
                            "failureCount": 2
                        }))
                    ]
                }
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run_style(&["accounts"], tmp.path(), &mut runtime, Style::Rich);

    assert_eq!(outcome.code, 0);
    let stdout = outcome.stdout.clone();
    let visible = strip_ansi(&stdout);
    // Same panel frame as status (AC-6).
    assert!(visible.contains("┌─ pool: anthropic"));
    assert!(visible.contains('└'));
    assert!(visible.contains("● available"));
    assert!(visible.contains("● unresponsive"));
    // Per-account detail lines beneath each row (AC-6); reasoning gets its
    // own line because five fields cannot fit the fixed width.
    assert!(visible.contains("in 22.1M  out 401.2K  cache 161.0M"));
    assert!(visible.contains("reasoning 64.0K"));
    // The no-reasoning account omits the reasoning line (AC-6).
    assert!(visible.contains("in 0  out 0  cache 0"));
    assert!(!visible.contains("reasoning 0"));
}

#[test]
fn accounts_reload_then_prints_runtime_accounts() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["accounts", "--reload"], tmp.path(), &mut runtime);

    assert_eq!(
        runtime.calls,
        [
            "reload:http://127.0.0.1:8317:sk-test",
            "accounts:http://127.0.0.1:8317:sk-test"
        ]
    );
    assert!(outcome.stdout.contains("reloaded accounts"));
    assert!(
        outcome
            .stdout
            .contains("anthropic@example.com available failures=0")
    );
}

#[test]
fn accounts_detail_prints_usage_and_cooldown_per_account() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime {
        accounts_payload: Some(json!({
            "providers": {
                "anthropic": {
                    "account_count": 3,
                    "accounts": [
                        account(json!({
                            "email": "a@x.com",
                            "available": true,
                            "failureCount": 0,
                            "totalRequests": 640,
                            "totalSuccesses": 638,
                            "totalInputTokens": 22_100_000,
                            "totalOutputTokens": 401_200,
                            "totalCacheCreationInputTokens": 6_000_000,
                            "totalCacheReadInputTokens": 155_000_000,
                            "totalReasoningOutputTokens": 64_000,
                            "planType": "max"
                        })),
                        account(json!({
                            "email": "b@x.com",
                            "available": false,
                            "cooldownUntil": soon(191.0),
                            "failureCount": 2,
                            "planType": "pro"
                        })),
                        account(json!({
                            "email": "c@x.com",
                            "available": false,
                            "failureCount": 5
                        }))
                    ]
                }
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run(&["accounts"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    let stdout = outcome.stdout.clone();
    // Account header keeps its legacy shape; the cooldown account reads
    // "on cooldown" with remaining time, not "unavailable" (AC-6).
    assert!(stdout.contains("anthropic: 3 accounts\n"));
    assert!(stdout.contains("  a@x.com available failures=0 plan=max\n"));
    assert!(stdout.contains("  b@x.com on cooldown 3m10s failures=2 plan=pro\n"));
    // An unavailable snapshot with no future cooldownUntil keeps the old word.
    assert!(stdout.contains("  c@x.com unavailable failures=5\n"));
    // Detail line under each account: requests (ok) plus token totals;
    // reasoning prints only when non-zero (AC-5).
    assert!(
        stdout.contains(
            "    requests 640 (638 ok) in 22.1M out 401.2K cache 161.0M reasoning 64.0K\n"
        )
    );
    assert!(stdout.contains("    requests 0 (0 ok) in 0 out 0 cache 0\n"));
    assert!(!stdout.contains("reasoning 0"));
}

#[test]
fn service_install_delegates_custom_host_port() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    let outcome = run(
        &[
            "service",
            "install",
            "--host",
            "127.0.0.1",
            "--port",
            "8318",
        ],
        tmp.path(),
        &mut runtime,
    );

    assert_eq!(outcome.code, 0);
    let request = runtime.install_request.expect("install request");
    assert_eq!(request.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(request.port, Some(8318));
    assert!(!request.start);
    assert!(!request.enable);
}

#[test]
fn service_control_subcommands_delegate_to_runtime() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let start = run(&["service", "start"], tmp.path(), &mut runtime);
    let status = run(&["service", "status"], tmp.path(), &mut runtime);
    let restart = run(&["service", "restart"], tmp.path(), &mut runtime);
    let stop = run(&["service", "stop"], tmp.path(), &mut runtime);
    let uninstall = run(&["service", "uninstall"], tmp.path(), &mut runtime);

    assert_eq!(
        runtime.calls,
        [
            "service:start",
            "service:status",
            "service:restart",
            "service:stop",
            "service:uninstall"
        ]
    );
    assert_eq!(start.stdout.trim(), "started service");
    assert_eq!(status.stdout.trim(), "active");
    assert_eq!(restart.stdout.trim(), "restarted service");
    assert_eq!(stop.stdout.trim(), "stopped service");
    assert_eq!(
        uninstall.stdout.trim(),
        "uninstalled service: /tmp/pengepul.service"
    );
}

#[test]
fn login_delegates_provider() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["login", "--provider", "codex"], tmp.path(), &mut runtime);

    assert_eq!(runtime.login_provider, Some(ProviderId::codex()));
    assert_eq!(
        outcome.stdout.trim(),
        "saved codex account token for codex@example.com"
    );
}

#[test]
fn service_logs_passes_follow_and_lines() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let outcome = run(
        &["service", "logs", "-f", "-n", "100"],
        tmp.path(),
        &mut runtime,
    );

    assert_eq!(outcome.code, 0);
    assert_eq!(runtime.calls, ["service:logs:follow=true:lines=100"]);
}

#[test]
fn service_logs_defaults_to_recent_lines_without_follow() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let outcome = run(&["service", "logs"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    assert_eq!(runtime.calls, ["service:logs:follow=false:lines=50"]);
}

fn write_config_with_providers(home: &Path, providers: &str) {
    let config_dir = home.join(".pengepul");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.yaml"),
        format!("host: \"127.0.0.1\"\nport: 8317\nauth-dir: ~/.pengepul\napi-keys:\n  - sk-test\nproviders:\n{providers}"),
    )
    .expect("write config");
}

#[test]
fn login_rejects_removed_provider() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    // The removed provider fails at argument parsing, before any runtime call.
    assert!(
        run_with_env(
            &["login", "--provider", "opencode"],
            tmp.path(),
            tmp.path(),
            &mut runtime,
            Style::Plain,
        )
        .is_err(),
        "removed provider must be rejected at parse time"
    );
    assert!(runtime.login_provider.is_none());
}

#[test]
fn login_with_key_saves_a_configured_provider_key() {
    let tmp = tempdir().expect("tempdir");
    write_config_with_providers(
        tmp.path(),
        "  groq:\n    base-url: https://api.groq.com/openai/v1\n",
    );
    let mut runtime = FakeRuntime::default();

    let outcome = run(
        &["login", "--provider", "groq", "--key", "gsk-secret-key"],
        tmp.path(),
        &mut runtime,
    );

    assert_eq!(outcome.code, 0);
    // Static keys are saved by the CLI itself; the OAuth runtime is never entered.
    assert!(runtime.login_provider.is_none(), "no OAuth login for a key");
    let token_file = tmp.path().join(".pengepul/groq");
    assert!(token_file.exists(), "key token saved under auth-dir/groq");
    let saved: serde_json::Value = {
        let entry = std::fs::read_dir(&token_file)
            .expect("groq dir")
            .next()
            .expect("one token file")
            .expect("read entry");
        let text = std::fs::read_to_string(entry.path()).expect("token json");
        serde_json::from_str(&text).expect("parse token json")
    };
    assert_eq!(saved["access_token"], "gsk-secret-key");
    assert_eq!(saved["type"], "generic");
    assert!(
        outcome.stdout.contains("groq"),
        "{} {}",
        outcome.stdout,
        outcome.stderr
    );
    assert!(
        !outcome.stdout.contains("gsk-secret-key"),
        "the raw key must never be printed: {}",
        outcome.stdout
    );
}

#[test]
fn login_without_key_for_a_configured_provider_fails() {
    let tmp = tempdir().expect("tempdir");
    write_config_with_providers(
        tmp.path(),
        "  groq:\n    base-url: https://api.groq.com/openai/v1\n",
    );
    let mut runtime = FakeRuntime::default();

    assert!(
        run_with_env(
            &["login", "--provider", "groq"],
            tmp.path(),
            tmp.path(),
            &mut runtime,
            Style::Plain,
        )
        .is_err(),
        "a configured provider needs a --key"
    );
    assert!(runtime.login_provider.is_none());
}

#[test]
fn login_for_an_unconfigured_provider_lists_the_configured_ones() {
    let tmp = tempdir().expect("tempdir");
    write_config_with_providers(
        tmp.path(),
        "  groq:\n    base-url: https://api.groq.com/openai/v1\n",
    );
    let mut runtime = FakeRuntime::default();

    let error = run_with_env(
        &["login", "--provider", "mistral", "--key", "x"],
        tmp.path(),
        tmp.path(),
        &mut runtime,
        Style::Plain,
    )
    .expect_err("unconfigured provider must be rejected");
    let full = format!("{error:#}");
    assert!(
        full.contains("groq"),
        "the error lists the configured providers: {full}"
    );
    assert!(runtime.login_provider.is_none());
}

#[test]
fn tag_is_newer_compares_versions_numerically() {
    assert!(pengepul::cli::tag_is_newer("v0.2.0", "0.1.0"));
    assert!(pengepul::cli::tag_is_newer("v0.1.1", "0.1.0"));
    assert!(pengepul::cli::tag_is_newer("v1.0.0", "0.9.9"));
    // 10 > 9 numerically, though "v0.10.0" sorts before "v0.9.0" as a string
    assert!(pengepul::cli::tag_is_newer("v0.10.0", "0.9.0"));

    assert!(!pengepul::cli::tag_is_newer("v0.1.0", "0.1.0"));
    assert!(!pengepul::cli::tag_is_newer("v0.1.0", "0.2.0"));

    // an unparseable tag prompts rather than silently never updating
    assert!(pengepul::cli::tag_is_newer("nightly", "0.1.0"));
}

#[test]
fn update_check_reports_without_installing() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        latest_tag: Some("v99.0.0".to_string()),
        ..FakeRuntime::default()
    };

    let outcome = run(&["update", "--check"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    assert!(outcome.stdout.contains("v99.0.0"), "{}", outcome.stdout);
    assert!(runtime.installed.is_none(), "--check must not install");
}

#[test]
fn update_installs_when_a_newer_release_exists() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        latest_tag: Some("v99.0.0".to_string()),
        ..FakeRuntime::default()
    };

    let outcome = run(&["update"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    let (tag, asset) = runtime.installed.expect("must install");
    assert_eq!(tag, "v99.0.0");
    assert!(asset.ends_with(".tar.gz"), "asset was {asset}");
}

#[test]
fn update_is_a_noop_when_already_current() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        latest_tag: Some("v0.0.1".to_string()),
        ..FakeRuntime::default()
    };

    let outcome = run(&["update"], tmp.path(), &mut runtime);

    assert!(outcome.stdout.contains("latest"), "{}", outcome.stdout);
    assert!(runtime.installed.is_none(), "must not reinstall");
}

#[test]
fn status_ends_with_relay_total_block_in_plain() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime {
        accounts_payload: Some(json!({
            "providers": {
                "anthropic": {
                    "account_count": 1,
                    "accounts": [account(json!({
                        "email": "a@x.com",
                        "available": true,
                        "totalRequests": 640,
                        "totalInputTokens": 33,
                        "totalOutputTokens": 120,
                        "totalCacheReadInputTokens": 7,
                        "totalCacheCreationInputTokens": 0
                    }))]
                },
                "commandcode": {
                    "account_count": 1,
                    "accounts": [account(json!({
                        "email": "k@x.com",
                        "available": true,
                        "totalRequests": 10,
                        "totalInputTokens": 10,
                        "totalOutputTokens": 5
                    }))]
                }
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run(&["status"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    // AC-1: blank line, header, two totals.
    assert!(
        outcome
            .stdout
            .contains("\nrelay total: 2 pools, 2 accounts\n")
    );
    assert!(outcome.stdout.contains("total requests 650\n"));
    assert!(outcome.stdout.contains("total tokens 175\n"));
    // The block is last: nothing after `total tokens`.
    assert!(outcome.stdout.trim_end().ends_with("total tokens 175"));
}

#[test]
fn status_relay_total_block_in_rich_has_64_wide_rule() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime {
        rich: true,
        accounts_payload: Some(json!({
            "providers": {
                "anthropic": {
                    "account_count": 1,
                    "accounts": [account(json!({
                        "email": "a@x.com",
                        "available": true,
                        "totalRequests": 640,
                        "totalInputTokens": 33,
                        "totalOutputTokens": 120
                    }))]
                }
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run_style(&["status"], tmp.path(), &mut runtime, Style::Rich);

    assert_eq!(outcome.code, 0);
    let visible = strip_ansi(&outcome.stdout);
    // AC-2: rule of exactly 64 with the header inside, then the two totals.
    let lines: Vec<&str> = visible.lines().collect();
    let rule_index = lines
        .iter()
        .position(|line| line.contains("relay total:"))
        .expect("relay total rule present");
    let last_panel = lines
        .iter()
        .rposition(|line| line.starts_with('└'))
        .expect("panel bottom rule present");
    // AC-2: the block sits after the panels, not before them.
    assert!(rule_index > last_panel);
    let rule = lines[rule_index];
    assert_eq!(rule.chars().count(), 64, "rule line: {rule}");
    assert!(rule.starts_with('─'));
    assert!(visible.contains("total requests 640"));
    assert!(visible.contains("total tokens 153"));
    assert!(visible.trim_end().ends_with("total tokens 153"));
}

#[test]
fn status_relay_total_covers_empty_pools_and_zero_relay() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime {
        accounts_payload: Some(json!({
            "providers": {
                // Loaded accounts with no traffic (AC-3 second half).
                "anthropic": {
                    "account_count": 1,
                    "accounts": [account(json!({
                        "email": "a@x.com",
                        "available": true
                    }))]
                },
                // Empty pool: counts toward pools (AC-5), adds no tokens.
                "codex": {"account_count": 0, "accounts": []},
                "commandcode": {"account_count": 0, "accounts": []}
            }
        })),
        ..FakeRuntime::default()
    };

    let outcome = run(&["status"], tmp.path(), &mut runtime);

    assert_eq!(outcome.code, 0);
    // AC-4: block prints even at a zero relay; AC-5: empty pools count.
    // Empty pools are hidden, so the header counts only shown pools.
    assert!(outcome.stdout.contains("relay total: 1 pool, 1 account\n"));
    assert!(outcome.stdout.contains("total requests 0\n"));
    assert!(outcome.stdout.contains("total tokens 0\n"));
}

#[test]
fn service_actions_render_a_panel_when_rich_and_plain_bytes_when_piped() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime::default();

    let rich = run_style(
        &["service", "restart"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );
    assert_eq!(rich.code, 0);
    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("┌─ service "), "panel header: {visible}");
    assert!(visible.contains("●"));
    assert!(visible.contains("restarted"));
    for line in visible.lines() {
        assert!(line.chars().count() <= 64, "too wide: {line}");
    }

    // Piped: today's exact bytes (AC-1).
    runtime.calls.clear();
    let plain = run(&["service", "restart"], tmp.path(), &mut runtime);
    assert_eq!(plain.stdout.trim(), "restarted service");
}

#[test]
fn login_renders_a_panel_when_rich_and_plain_bytes_when_piped() {
    let tmp = tempdir().expect("tempdir");
    write_config_with_providers(
        tmp.path(),
        "
  commandcode:
    base-url: https://api.commandcode.ai/v1",
    );
    let mut runtime = FakeRuntime::default();

    let rich = run_style(
        &["login", "--provider", "commandcode", "--key", "sk-secret"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );
    assert_eq!(rich.code, 0);
    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("┌─ login: commandcode "), "{visible}");
    assert!(visible.contains("key-"));
    assert!(visible.contains("●"));
    assert!(visible.contains("saved"));

    let plain = run(
        &["login", "--provider", "commandcode", "--key", "sk-secret"],
        tmp.path(),
        &mut runtime,
    );
    assert!(
        plain
            .stdout
            .contains("saved commandcode account token for key-")
    );
}

#[test]
fn update_renders_a_panel_when_rich_and_plain_bytes_when_piped() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        latest_tag: Some("v99.0.0".to_string()),
        ..FakeRuntime::default()
    };

    let rich = run_style(
        &["update", "--check"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );
    assert_eq!(rich.code, 0);
    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("┌─ update "), "{visible}");
    assert!(visible.contains("v99.0.0"));
    assert!(visible.contains("●"));

    runtime.latest_tag = Some("v0.0.1".to_string());
    let plain = run(&["update", "--check"], tmp.path(), &mut runtime);
    assert!(plain.stdout.contains(&format!(
        "pengepul {} is the latest release",
        env!("CARGO_PKG_VERSION")
    )));
}

#[test]
fn config_path_and_api_key_render_a_panel_when_rich_and_bare_when_piped() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "127.0.0.1", 8317);
    let mut runtime = FakeRuntime::default();

    let rich = run_style(&["config", "path"], tmp.path(), &mut runtime, Style::Rich);
    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("┌─ config "), "{visible}");
    assert!(visible.contains("config.yaml"));

    let plain = run(&["config", "path"], tmp.path(), &mut runtime);
    assert_eq!(
        plain.stdout.trim(),
        tmp.path().join(".pengepul/config.yaml").to_string_lossy()
    );
}

#[test]
fn service_status_parses_systemctl_into_a_panel_when_rich() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        service_status_text: Some(
            "● pengepul.service - pengepul API relay\n     Loaded: loaded (/home/kognos/.config/systemd/user/pengepul.service; enabled; preset: enabled)\n     Active: active (running) since Sat 2026-09-05 04:09:31 WIB; 4min 28s ago\n   Main PID: 3162166 (pengepul)\n      Tasks: 7 (limit: 14306)\n     Memory: 2.3M (peak: 14.6M)\n        CPU: 2.614s\n     CGroup: /user.slice\n"
                .to_string(),
        ),
        ..FakeRuntime::default()
    };

    let rich = run_style(
        &["service", "status"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    assert_eq!(rich.code, 0);
    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("┌─ service "), "{visible}");
    assert!(visible.contains("active (running)"));
    assert!(visible.contains("enabled"));
    assert!(visible.contains("3162166"));
    assert!(visible.contains("2.3M"));
    assert!(visible.contains("2.614s"));
    assert!(visible.contains("tasks  7"));
    assert!(visible.contains("4m28s"));
    for line in visible.lines() {
        assert!(line.chars().count() <= 64, "too wide: {line}");
    }

    // Piped: the platform tool's text verbatim (AC-3).
    runtime.service_status_text = Some("active".to_string());
    let plain = run(&["service", "status"], tmp.path(), &mut runtime);
    assert_eq!(plain.stdout.trim(), "active");
}

#[test]
fn service_status_parses_launchctl_into_the_same_panel_shape() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        service_status_text: Some("pid = 411\nstate = running\nprogram = pengepul\n".to_string()),
        ..FakeRuntime::default()
    };

    let rich = run_style(
        &["service", "status"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("running"), "{visible}");
    assert!(visible.contains("411"));
}

#[test]
fn service_status_without_a_service_renders_an_amber_panel_when_rich() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        service_status_error: Some(
            "no service installed; run `pengepul service install`".to_string(),
        ),
        ..FakeRuntime::default()
    };

    let rich = run_style(
        &["service", "status"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    assert_eq!(rich.code, 0);
    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("not installed"), "{visible}");
    assert!(visible.contains("pengepul service install"));

    // Piped: the error path stays an error (AC-4).
    let plain = run_with_env(
        &["service", "status"],
        tmp.path(),
        tmp.path(),
        &mut runtime,
        Style::Plain,
    );
    assert!(plain.is_err());
    assert!(
        plain
            .expect_err("plain errors")
            .to_string()
            .contains("no service installed")
    );
}

#[test]
fn status_moves_the_header_facts_into_the_relay_block() {
    let tmp = tempdir().expect("tempdir");
    write_config(tmp.path(), "0.0.0.0", 8318);
    let mut runtime = FakeRuntime::default();

    // Rich: no header lines before the first panel.
    let rich = run_style(&["status"], tmp.path(), &mut runtime, Style::Rich);
    let visible = strip_ansi(&rich.stdout);
    let first_panel = visible.find('\u{250c}').expect("panel present");
    assert!(
        !visible[..first_panel].contains("config:"),
        "header must not precede panels: {}",
        &visible[..first_panel]
    );
    assert!(!visible[..first_panel].contains("url:"));
    assert!(!visible[..first_panel].contains("server:"));
    // Facts live in the relay block.
    assert!(visible.contains("relay total:"));
    assert!(visible.contains("server ok"));
    assert!(visible.contains("url http://127.0.0.1:8318 \u{2014} server ok"));

    // Plain: same facts, same place.
    let plain = run(&["status"], tmp.path(), &mut runtime);
    let body = plain.stdout;
    let first_pool = body.find("anthropic:").expect("pool line");
    assert!(!body[..first_pool].contains("config:"));
    assert!(body.contains("relay total:"));
    assert!(body.contains("url http://127.0.0.1:8318 \u{2014} server ok"));
}

#[test]
fn service_status_renders_a_stopped_unit_without_claiming_not_installed() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        service_status_text: Some(
            "○ pengepul.service - pengepul API relay\n     Loaded: loaded (/x/pengepul.service; enabled; preset: enabled)\n     Active: inactive (dead) since Sat 2026-09-05 04:09:31 WIB; 3 days ago\n"
                .to_string(),
        ),
        ..FakeRuntime::default()
    };

    let rich = run_style(
        &["service", "status"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    let visible = strip_ansi(&rich.stdout);
    assert!(visible.contains("inactive (dead)"), "{visible}");
    assert!(!visible.contains("not installed"));
    assert!(visible.contains("stopped  3d0h ago"), "{visible}");
    // Amber glyph on raw bytes for a non-active unit (AC-3 color contract).
    assert!(rich.stdout.contains("\u{1b}[33m●"), "{}", rich.stdout);
}

#[test]
fn service_status_other_errors_stay_errors_even_when_rich() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        service_status_error: Some("failed to run systemctl".to_string()),
        ..FakeRuntime::default()
    };

    let rich = run_with_env(
        &["service", "status"],
        tmp.path(),
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    assert!(rich.is_err(), "a tool failure must not render as a panel");
}

#[test]
fn action_panels_hold_the_width_and_paint_the_glyph() {
    let tmp = tempdir().expect("tempdir");
    write_config_with_providers(
        tmp.path(),
        "\n  commandcode:\n    base-url: https://api.commandcode.ai/v1",
    );
    let mut runtime = FakeRuntime {
        latest_tag: Some("v99.0.0".to_string()),
        ..FakeRuntime::default()
    };

    let login = run_style(
        &["login", "--provider", "commandcode", "--key", "sk-secret"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );
    let update_check = run_style(
        &["update", "--check"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );
    let update = run_style(&["update"], tmp.path(), &mut runtime, Style::Rich);
    let config = run_style(
        &["config", "api-key"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    for outcome in [&login, &update_check, &update, &config] {
        for line in strip_ansi(&outcome.stdout).lines() {
            assert_eq!(line.chars().count(), 64, "panel line: {line}");
        }
    }
    // Green for saved/updated, amber for an available update (AC-2/AC-5).
    assert!(login.stdout.contains("\u{1b}[32m●"));
    assert!(update.stdout.contains("\u{1b}[32m●"));
    assert!(update_check.stdout.contains("\u{1b}[33m●"));
    assert!(strip_ansi(&update.stdout).contains("updated v99.0.0"));
}

#[test]
fn update_plain_bytes_are_pinned() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        latest_tag: Some("v99.0.0".to_string()),
        ..FakeRuntime::default()
    };

    let check = run(&["update", "--check"], tmp.path(), &mut runtime);
    assert_eq!(
        check.stdout,
        format!(
            "pengepul v99.0.0 is available (running {}); run `pengepul update` to install it\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    let install = run(&["update"], tmp.path(), &mut runtime);
    assert_eq!(
        install.stdout,
        "updated to v99.0.0 at /usr/local/bin/pengepul\n"
    );
}

#[test]
fn service_status_paints_a_failed_unit_red() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = FakeRuntime {
        service_status_text: Some(
            "× pengepul.service - pengepul API relay\n     Loaded: loaded (/x/pengepul.service; enabled; preset: enabled)\n     Active: failed (Result: exit-code) since Sat 2026-09-05 04:09:31 WIB; 2h ago\n"
                .to_string(),
        ),
        ..FakeRuntime::default()
    };

    let rich = run_style(
        &["service", "status"],
        tmp.path(),
        &mut runtime,
        Style::Rich,
    );

    assert!(strip_ansi(&rich.stdout).contains("failed (Result: exit-code)"));
    assert!(rich.stdout.contains("\u{1b}[31m●"), "{}", rich.stdout);
}

#[test]
fn a_command_level_config_wins_over_the_root_one() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().join("root.yaml");
    let command = tmp.path().join("command.yaml");
    std::fs::write(
        &root,
        "host: \"127.0.0.1\"\nport: 8317\nauth-dir: ~/.pengepul\napi-keys:\n  - sk-root\n",
    )
    .expect("write root config");
    std::fs::write(
        &command,
        "host: \"127.0.0.1\"\nport: 8318\nauth-dir: ~/.pengepul\napi-keys:\n  - sk-command\n",
    )
    .expect("write command config");
    let mut runtime = FakeRuntime::default();

    let outcome = run(
        &[
            "--config",
            root.to_str().expect("utf-8"),
            "status",
            "--config",
            command.to_str().expect("utf-8"),
        ],
        tmp.path(),
        &mut runtime,
    );

    assert_eq!(outcome.code, 0);
    assert_eq!(runtime.health_url.as_deref(), Some("http://127.0.0.1:8318"));
    assert_eq!(runtime.accounts_api_key.as_deref(), Some("sk-command"));

    // Without the command-level flag the root one applies.
    let mut runtime = FakeRuntime::default();
    run(
        &["--config", root.to_str().expect("utf-8"), "status"],
        tmp.path(),
        &mut runtime,
    );
    assert_eq!(runtime.health_url.as_deref(), Some("http://127.0.0.1:8317"));
    assert_eq!(runtime.accounts_api_key.as_deref(), Some("sk-root"));
}
