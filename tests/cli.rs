use std::path::{Path, PathBuf};

use anyhow::Result;
use pengepul::cli::{CliRuntime, RunOutcome, ServiceInstallRequest, run_with_env};
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
        Ok("active".to_string())
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
    run_with_env(argv, home, home, runtime).expect("cli run")
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
    assert!(outcome.stdout.contains("config: "));
    assert!(outcome.stdout.contains("url: http://127.0.0.1:8318"));
    assert!(outcome.stdout.contains("server: ok"));
    assert!(outcome.stdout.contains("anthropic: 1 account"));
    assert!(outcome.stdout.contains("codex: 2 accounts"));
    assert_eq!(runtime.health_url.as_deref(), Some("http://127.0.0.1:8318"));
    assert_eq!(runtime.accounts_api_key.as_deref(), Some("sk-test"));
}

fn account(json: Value) -> Value {
    json
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
                            "cooldownUntil": soon(252.0),
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
    assert!(outcome.stdout.contains(
        "tokens in 45.2M  out 812.3K  cache-read 311.7M  cache-write 12.4M  reasoning 96.0K"
    ));
    // A provider whose account omits every total rolls up as zeros (AC-4).
    assert!(outcome.stdout.contains("groq: 1 account (1 available)"));
    assert!(outcome.stdout.contains("requests 0  (0 ok, 0 failed)"));
    assert!(
        outcome
            .stdout
            .contains("tokens in 0  out 0  cache-read 0  cache-write 0  reasoning 0")
    );
    // An empty pool prints only its bare header line, no rollup (AC-7).
    assert!(outcome.stdout.contains("deepseek: 0 accounts\n"));
    assert!(!outcome.stdout.contains("deepseek: 0 accounts ("));
    assert!(!outcome.stdout.contains("deepseek: 0 accounts\n  requests"));
    // Each rollup block opens on its own line after a blank one (AC-1).
    assert!(
        outcome
            .stdout
            .contains("\n\nanthropic: 3 accounts (2 available)")
    );
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
    assert!(stdout.contains(
        "    requests 640 (638 ok) in 22.1M out 401.2K cache-read 155.0M cache-write 6.0M reasoning 64.0K\n"
    ));
    assert!(stdout.contains("    requests 0 (0 ok) in 0 out 0 cache-read 0 cache-write 0\n"));
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
