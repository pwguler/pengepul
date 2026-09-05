use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pengepul::accounts::{AccountManager, RefreshPolicy, RefreshPolicyKind};
use pengepul::tokens::save_token;
use pengepul::types::{ProviderId, RefreshTokenExhaustedError, TokenData, UsageData};
use serde_json::{Value, json};
use tempfile::tempdir;

#[tokio::test]
async fn since_last_refresh_refreshes_legacy_token_without_last_refresh() {
    let tmp = tempdir().expect("tempdir");
    let codex_dir = tmp.path().join("codex");
    fs::create_dir_all(&codex_dir).expect("codex dir");
    fs::write(
        codex_dir.join("bob_example_com.json"),
        json!({
            "access_token": "old-access",
            "refresh_token": "old-refresh",
            "email": "bob@example.com",
            "type": "codex",
            "expired": "2030-01-01T00:00:00Z",
            "account_uuid": "acct-codex"
        })
        .to_string(),
    )
    .expect("write token");
    let refresh_calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&refresh_calls);

    let mut manager = AccountManager::new(
        tmp.path().to_path_buf(),
        "codex".parse().unwrap(),
        move |refresh_token| {
            let captured = Arc::clone(&captured);
            Box::pin(async move {
                captured.lock().unwrap().push(refresh_token);
                Ok(TokenData {
                    access_token: "new-access".to_string(),
                    refresh_token: "new-refresh".to_string(),
                    email: "bob@example.com".to_string(),
                    expires_at: "2030-01-01T00:00:00Z".to_string(),
                    account_uuid: "acct-codex".to_string(),
                    provider: "codex".parse().unwrap(),
                    id_token: None,
                    last_refresh_at: None,
                    plan_type: None,
                })
            })
        },
        RefreshPolicy {
            kind: RefreshPolicyKind::SinceLastRefresh,
            seconds: 8 * 24 * 60 * 60,
        },
    );
    manager.load().expect("load accounts");

    assert!(
        manager
            .refresh_if_due("bob@example.com")
            .await
            .expect("refresh")
    );
    assert_eq!(*refresh_calls.lock().unwrap(), ["old-refresh"]);

    let snapshots = manager.snapshots();
    assert!(snapshots[0]["lastRefreshAt"].is_string());
    assert_eq!(snapshots[0]["email"], "bob@example.com");
}

#[tokio::test]
async fn exhausted_refresh_token_marks_account_for_reauth() {
    let tmp = tempdir().expect("tempdir");
    let codex_dir = tmp.path().join("codex");
    fs::create_dir_all(&codex_dir).expect("codex dir");
    fs::write(
        codex_dir.join("bob_example_com.json"),
        json!({
            "access_token": "old-access",
            "refresh_token": "old-refresh",
            "email": "bob@example.com",
            "type": "codex",
            "expired": "2000-01-01T00:00:00Z",
            "account_uuid": "acct-codex"
        })
        .to_string(),
    )
    .expect("write token");
    let mut manager = AccountManager::new(
        tmp.path().to_path_buf(),
        "codex".parse().unwrap(),
        |_refresh_token| {
            Box::pin(async move {
                Err(RefreshTokenExhaustedError::new(
                    "invalid_grant",
                    Some(400),
                    Some("invalid_grant".to_string()),
                )
                .into())
            })
        },
        RefreshPolicy::default(),
    );
    manager.load().expect("load accounts");

    assert!(
        !manager
            .refresh_if_due("bob@example.com")
            .await
            .expect("refresh result")
    );

    let snapshots = manager.snapshots();
    assert_eq!(snapshots[0]["available"], false);
    assert_eq!(snapshots[0]["failureCount"], 1);
    assert_eq!(snapshots[0]["totalFailures"], 1);
    assert_eq!(
        snapshots[0]["lastError"],
        "refresh token invalid_grant; re-run login for codex"
    );
}

#[tokio::test]
async fn failure_cooldown_doubles_from_one_second() {
    let tmp = tempdir().expect("tempdir");
    let codex_dir = tmp.path().join("codex");
    fs::create_dir_all(&codex_dir).expect("codex dir");
    fs::write(
        codex_dir.join("key.json"),
        json!({
            "access_token": "sk-codex",
            "refresh_token": "",
            "email": "codex-abc12345",
            "type": "codex",
            "expired": "9999-12-31T23:59:59Z",
            "account_uuid": ""
        })
        .to_string(),
    )
    .expect("write codex token");
    let mut manager = AccountManager::new(
        tmp.path().to_path_buf(),
        "codex".parse().unwrap(),
        |_refresh_token| Box::pin(async { anyhow::bail!("unused refresh") }),
        RefreshPolicy::default(),
    );
    manager.load().expect("load accounts");

    // regardless of failure kind, consecutive failures back off 1s, 2s, 4s, …
    for expected in [1.0, 2.0, 4.0] {
        manager.record_failure("codex-abc12345", "auth", Some("Insufficient balance"));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs_f64();
        let snapshot = manager.snapshots().remove(0);
        assert_eq!(snapshot["available"], false);
        let remaining = snapshot["cooldownUntil"].as_f64().expect("cooldownUntil") - now;
        assert!(
            (expected - 0.5..=expected).contains(&remaining),
            "failure expected ~{expected}s cooldown, got {remaining}s"
        );
    }
}

fn static_token(email: &str) -> TokenData {
    TokenData {
        access_token: format!("access-{email}"),
        refresh_token: String::new(),
        email: email.to_string(),
        expires_at: "2030-01-01T00:00:00Z".to_string(),
        account_uuid: format!("acct-{email}"),
        provider: ProviderId::generic("commandcode"),
        id_token: None,
        last_refresh_at: None,
        plan_type: None,
    }
}

fn never_refresh_manager(auth_dir: PathBuf) -> AccountManager {
    AccountManager::new(
        auth_dir,
        ProviderId::generic("commandcode"),
        |_refresh_token| {
            Box::pin(async { Err(anyhow::anyhow!("refresh must never run for static keys")) })
        },
        RefreshPolicy {
            kind: RefreshPolicyKind::Never,
            seconds: 0,
        },
    )
}

fn persisted(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read usage.json"))
        .expect("parse usage.json")
}

#[tokio::test]
async fn usage_counters_survive_a_manager_rebuild() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    manager.record_attempt("k@example.com");
    manager.record_attempt("k@example.com");
    manager.record_success(
        "k@example.com",
        Some(&UsageData {
            input_tokens: 33,
            output_tokens: 120,
            cache_creation_input_tokens: 7,
            cache_read_input_tokens: 4,
            reasoning_output_tokens: 13,
        }),
        "claude-fable-5-1",
    );
    manager.record_failure("k@example.com", "upstream", Some("boom"));

    // AC-1..AC-3: a fresh manager over the same auth dir sees the totals.
    let mut rebuilt = never_refresh_manager(tmp.path().to_path_buf());
    rebuilt.load().expect("reload");
    let snapshot = &rebuilt.snapshots()[0];
    assert_eq!(snapshot["totalRequests"], 2);
    assert_eq!(snapshot["totalSuccesses"], 1);
    assert_eq!(snapshot["totalFailures"], 1);
    assert_eq!(snapshot["totalInputTokens"], 33);
    assert_eq!(snapshot["totalOutputTokens"], 120);
    assert_eq!(snapshot["totalCacheCreationInputTokens"], 7);
    assert_eq!(snapshot["totalCacheReadInputTokens"], 4);
    assert_eq!(snapshot["totalReasoningOutputTokens"], 13);
    // AC-2: the failure wall itself is not persisted — fresh process retries.
    assert_eq!(snapshot["available"], true);
}

#[tokio::test]
async fn usage_file_lies_per_provider_and_ignores_strangers() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    // AC-4: a usage.json entry for an email with no token is ignored...
    let provider_dir = tmp.path().join("commandcode");
    fs::create_dir_all(&provider_dir).expect("provider dir");
    let usage_path = provider_dir.join("usage.json");
    fs::write(
        &usage_path,
        json!({
            "k@example.com": {"total_requests": 5, "total_input_tokens": 50},
            "ghost@example.com": {"total_requests": 99}
        })
        .to_string(),
    )
    .expect("write usage");

    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    let snapshot = &manager.snapshots()[0];
    assert_eq!(snapshot["totalRequests"], 5);
    assert_eq!(snapshot["totalInputTokens"], 50);
    assert!(manager.account("ghost@example.com").is_none());

    // ...and the next write drops the stranger instead of keeping it.
    manager.record_attempt("k@example.com");
    let stored = persisted(&usage_path);
    assert!(stored.get("ghost@example.com").is_none());
}

#[tokio::test]
async fn corrupted_usage_file_starts_from_zero() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let provider_dir = tmp.path().join("commandcode");
    fs::create_dir_all(&provider_dir).expect("provider dir");
    fs::write(provider_dir.join("usage.json"), "{not json").expect("write junk");

    // AC-5: permissive load, zeros, and startup is not broken.
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load survives corruption");
    let snapshot = &manager.snapshots()[0];
    assert_eq!(snapshot["totalRequests"], 0);

    // The next write replaces the junk with valid JSON.
    manager.record_attempt("k@example.com");
    assert_eq!(
        persisted(&provider_dir.join("usage.json"))["k@example.com"]["total_requests"],
        1
    );
}

#[tokio::test]
async fn token_refresh_keeps_the_counters() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    manager.record_attempt("k@example.com");
    manager.record_success("k@example.com", None, "claude-fable-5-1");
    assert_eq!(manager.snapshots()[0]["totalRequests"], 1);

    // Spec: token rotation must not reset usage counters. Rotate for
    // real so reload() takes the `updated` branch (where the resets live).
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "rotated-access".to_string(),
            ..static_token("k@example.com")
        },
    )
    .expect("rotate token");
    manager.reload().expect("reload");
    assert_eq!(manager.snapshots()[0]["totalRequests"], 1);
    assert_eq!(manager.snapshots()[0]["totalSuccesses"], 1);
}

/// usage-by-model AC-1/AC-2: successes accumulate per model, per account.
#[tokio::test]
async fn per_model_counters_accumulate_per_account() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("a@example.com")).expect("save token");
    save_token(tmp.path(), &static_token("b@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    let usage = |input: i64| UsageData {
        input_tokens: input,
        output_tokens: 10,
        cache_creation_input_tokens: 1,
        cache_read_input_tokens: 2,
        reasoning_output_tokens: 3,
    };
    manager.record_success("a@example.com", Some(&usage(100)), "claude-fable-5-1");
    manager.record_success("a@example.com", Some(&usage(200)), "claude-fable-5-1");
    manager.record_success("a@example.com", Some(&usage(400)), "claude-sonnet-4-5");
    manager.record_success("b@example.com", Some(&usage(800)), "claude-fable-5-1");

    let snapshots = manager.snapshots();
    let by_email = |email: &str| -> Value {
        snapshots
            .iter()
            .find(|snapshot| snapshot["email"] == email)
            .expect("account present")
            .clone()
    };

    // AC-3: name-sorted array of per-model counters.
    let a_models = by_email("a@example.com")["models"].clone();
    assert_eq!(a_models[0]["model"], "claude-fable-5-1");
    assert_eq!(a_models[0]["successes"], 2);
    assert_eq!(a_models[0]["inputTokens"], 300);
    assert_eq!(a_models[0]["outputTokens"], 20);
    assert_eq!(a_models[0]["cacheCreationInputTokens"], 2);
    assert_eq!(a_models[0]["cacheReadInputTokens"], 4);
    assert_eq!(a_models[0]["reasoningOutputTokens"], 6);
    assert_eq!(a_models[1]["model"], "claude-sonnet-4-5");
    assert_eq!(a_models[1]["successes"], 1);
    assert_eq!(a_models[1]["inputTokens"], 400);

    // AC-2: the same model on another account stays separate.
    let b_models = by_email("b@example.com")["models"].clone();
    assert_eq!(b_models.as_array().expect("array").len(), 1);
    assert_eq!(b_models[0]["inputTokens"], 800);
}

/// usage-by-model AC-4: per-model counters survive a manager rebuild, and a
/// pre-existing file without `models` loads with totals intact.
#[tokio::test]
async fn per_model_counters_persist_and_tolerate_legacy_files() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    manager.record_success(
        "k@example.com",
        Some(&UsageData {
            input_tokens: 33,
            output_tokens: 120,
            cache_creation_input_tokens: 7,
            cache_read_input_tokens: 4,
            reasoning_output_tokens: 13,
        }),
        "claude-fable-5-1",
    );

    let usage_file = tmp.path().join("commandcode").join("usage.json");
    let stored = persisted(&usage_file);
    assert_eq!(
        stored["k@example.com"]["models"]["claude-fable-5-1"]["input_tokens"],
        33
    );

    let mut rebuilt = never_refresh_manager(tmp.path().to_path_buf());
    rebuilt.load().expect("reload");
    let models = rebuilt.snapshots()[0]["models"].clone();
    assert_eq!(models[0]["model"], "claude-fable-5-1");
    assert_eq!(models[0]["successes"], 1);
    assert_eq!(models[0]["outputTokens"], 120);

    // AC-4 second half: a file written before this change has no `models`
    // key; the account totals still load and the model list is empty.
    fs::write(
        &usage_file,
        json!({
            "k@example.com": {
                "total_requests": 9,
                "total_successes": 8,
                "total_failures": 1,
                "total_input_tokens": 500,
                "total_output_tokens": 600,
                "total_cache_creation_input_tokens": 1,
                "total_cache_read_input_tokens": 2,
                "total_reasoning_output_tokens": 3
            }
        })
        .to_string(),
    )
    .expect("write legacy usage.json");
    let mut legacy = never_refresh_manager(tmp.path().to_path_buf());
    legacy.load().expect("load legacy");
    let snapshot = &legacy.snapshots()[0];
    assert_eq!(snapshot["totalRequests"], 9);
    assert_eq!(snapshot["totalInputTokens"], 500);
    assert_eq!(
        snapshot["models"].as_array().expect("array").len(),
        0,
        "no attribution for pre-existing history"
    );
}
