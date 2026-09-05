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

/// usage-by-model: a success with no usage block (the count-tokens route,
/// or a 2xx whose usage will not parse) still belongs to the model that
/// served it. Without this, per-model `ok` counts drift below the
/// account's for reasons unrelated to the no-backfill gap.
#[tokio::test]
async fn a_success_without_usage_still_counts_against_its_model() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    manager.record_success("k@example.com", None, "claude-fable-5-1");
    manager.record_success(
        "k@example.com",
        Some(&UsageData {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 0,
        }),
        "claude-fable-5-1",
    );

    let snapshot = &manager.snapshots()[0];
    assert_eq!(snapshot["totalSuccesses"], 2);
    let models = snapshot["models"].clone();
    assert_eq!(models[0]["model"], "claude-fable-5-1");
    // Both successes counted; only the second carried tokens.
    assert_eq!(models[0]["successes"], 2);
    assert_eq!(models[0]["inputTokens"], 10);
    assert_eq!(models[0]["outputTokens"], 20);
}

/// usage-by-model AC-4: a `models` sub-object that is not an object, or
/// holds junk entries, degrades to no attribution — never a failed load.
#[tokio::test]
async fn a_corrupt_models_block_loads_as_no_attribution() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let usage_file = tmp.path().join("commandcode").join("usage.json");
    fs::create_dir_all(usage_file.parent().expect("parent")).expect("mkdir");
    fs::write(
        &usage_file,
        json!({
            "k@example.com": {
                "total_requests": 5,
                "total_successes": 5,
                "models": "not-an-object"
            }
        })
        .to_string(),
    )
    .expect("write");

    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    let snapshot = &manager.snapshots()[0];
    assert_eq!(snapshot["totalRequests"], 5);
    assert_eq!(snapshot["models"].as_array().expect("array").len(), 0);
}

/// usage-trend AC-1/AC-2: outcomes land in a bucket keyed by the local
/// calendar day, carrying the same eight counters as the cumulative
/// record. Two outcomes on one day accumulate into one bucket.
#[tokio::test]
async fn outcomes_accumulate_into_one_bucket_per_local_day() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    manager.record_attempt("k@example.com");
    manager.record_success(
        "k@example.com",
        Some(&UsageData {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 1,
            cache_read_input_tokens: 2,
            reasoning_output_tokens: 3,
        }),
        "claude-fable-5-1",
    );
    manager.record_attempt("k@example.com");
    manager.record_success(
        "k@example.com",
        Some(&UsageData {
            input_tokens: 100,
            output_tokens: 200,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 0,
        }),
        "claude-fable-5-1",
    );
    manager.record_attempt("k@example.com");
    manager.record_failure("k@example.com", "upstream", Some("boom"));

    let days = manager.snapshots()[0]["days"].clone();
    let days = days.as_array().expect("days array");
    // AC-1: one bucket, not one per outcome.
    assert_eq!(days.len(), 1, "one bucket per day: {days:?}");
    let today = &days[0];
    // AC-2: the same eight counters as the cumulative record.
    assert_eq!(today["requests"], 3);
    assert_eq!(today["successes"], 2);
    assert_eq!(today["failures"], 1);
    assert_eq!(today["inputTokens"], 110);
    assert_eq!(today["outputTokens"], 220);
    assert_eq!(today["cacheCreationInputTokens"], 1);
    assert_eq!(today["cacheReadInputTokens"], 2);
    assert_eq!(today["reasoningOutputTokens"], 3);
    // The key is a local calendar date.
    let date = today["date"].as_str().expect("date string");
    assert_eq!(date.len(), 10, "YYYY-MM-DD: {date}");
    assert_eq!(date, chrono::Local::now().format("%Y-%m-%d").to_string());
}

/// usage-trend AC-3/AC-4: buckets persist and reload, a file written
/// before this change loads with none, and buckets past the 90-day
/// window are dropped on write.
#[tokio::test]
async fn daily_buckets_persist_and_are_trimmed_to_the_window() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let usage_file = tmp.path().join("commandcode").join("usage.json");

    // AC-3: a bucket survives a manager rebuild.
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    manager.record_attempt("k@example.com");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let stored = persisted(&usage_file);
    assert_eq!(stored["k@example.com"]["days"][&today]["requests"], 1);

    let mut rebuilt = never_refresh_manager(tmp.path().to_path_buf());
    rebuilt.load().expect("reload");
    let days = rebuilt.snapshots()[0]["days"].clone();
    assert_eq!(days.as_array().expect("array").len(), 1);
    assert_eq!(days[0]["date"], today);
    assert_eq!(days[0]["requests"], 1);

    // AC-4: a file holding an ancient bucket and a recent one keeps only
    // what falls inside the window, once a write happens.
    let stale = (chrono::Local::now() - chrono::Duration::days(120))
        .format("%Y-%m-%d")
        .to_string();
    let recent = (chrono::Local::now() - chrono::Duration::days(10))
        .format("%Y-%m-%d")
        .to_string();
    fs::write(
        &usage_file,
        json!({
            "k@example.com": {
                "total_requests": 9,
                "days": {
                    stale.clone(): {"requests": 5, "input_tokens": 500},
                    recent.clone(): {"requests": 3, "input_tokens": 300}
                }
            }
        })
        .to_string(),
    )
    .expect("write");

    let mut trimmed = never_refresh_manager(tmp.path().to_path_buf());
    trimmed.load().expect("load");
    // Both load; the window is applied when the file is next written.
    trimmed.record_attempt("k@example.com");
    let stored = persisted(&usage_file);
    let days = stored["k@example.com"]["days"]
        .as_object()
        .expect("days map");
    assert!(!days.contains_key(&stale), "stale bucket kept: {days:?}");
    assert!(
        days.contains_key(&recent),
        "recent bucket dropped: {days:?}"
    );
    assert_eq!(days[&recent]["requests"], 3);

    // AC-3 second half: a file with no `days` key loads with none.
    fs::write(
        &usage_file,
        json!({"k@example.com": {"total_requests": 7, "total_successes": 7}}).to_string(),
    )
    .expect("write legacy");
    let mut legacy = never_refresh_manager(tmp.path().to_path_buf());
    legacy.load().expect("load legacy");
    let snapshot = &legacy.snapshots()[0];
    assert_eq!(snapshot["totalRequests"], 7);
    assert_eq!(snapshot["days"].as_array().expect("array").len(), 0);
}

/// usage-trend AC-1: every writer of a cumulative counter writes its
/// bucket too. `record_refresh_exhausted` is the fourth, and it was
/// missed: cumulative failures and the sum of daily failures would
/// diverge on every reauth lockout, permanently.
#[tokio::test]
async fn a_reauth_lockout_lands_in_the_daily_bucket_too() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    manager.record_failure("k@example.com", "upstream", Some("boom"));
    manager.record_refresh_exhausted("k@example.com", "expired");

    let snapshot = &manager.snapshots()[0];
    let cumulative = snapshot["totalFailures"].as_i64().expect("failures");
    let bucketed: i64 = snapshot["days"]
        .as_array()
        .expect("days")
        .iter()
        .map(|day| day["failures"].as_i64().unwrap_or(0))
        .sum();
    assert_eq!(cumulative, 2, "both failures counted cumulatively");
    assert_eq!(
        bucketed, cumulative,
        "daily failures must reconcile with the cumulative count"
    );
}

/// usage-trend AC-4: the window bounds memory too, not only the file.
/// `persist_usage` trimmed the copy it wrote while `state.days` kept every
/// bucket for the life of the process, so a long-lived relay served an
/// admin payload holding more history than its own file.
#[tokio::test]
async fn in_memory_buckets_are_trimmed_not_only_the_written_copy() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let usage_file = tmp.path().join("commandcode").join("usage.json");
    let stale = (chrono::Local::now() - chrono::Duration::days(200))
        .format("%Y-%m-%d")
        .to_string();
    fs::create_dir_all(usage_file.parent().expect("parent")).expect("mkdir");
    fs::write(
        &usage_file,
        json!({"k@example.com": {"days": {stale.clone(): {"requests": 5}}}}).to_string(),
    )
    .expect("write");

    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    manager.record_attempt("k@example.com");

    let dates: Vec<String> = manager.snapshots()[0]["days"]
        .as_array()
        .expect("days")
        .iter()
        .filter_map(|day| day["date"].as_str().map(str::to_string))
        .collect();
    assert!(
        !dates.contains(&stale),
        "the payload serves a bucket the file no longer holds: {dates:?}"
    );
}

/// Every attempt reaches an outcome: `requests` must equal
/// `successes + failures`, or the three panels report a gap the operator
/// cannot account for ("1,404 requests, 1,398 ok, 0 failed").
#[tokio::test]
async fn requests_reconcile_with_successes_and_failures() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    // Every attempt the relay makes reaches an outcome: a success, a
    // failure, or a reauth lockout. The counters must add up for all of
    // them, and the daily buckets must agree with the cumulative ones.
    manager.record_attempt("k@example.com");
    manager.record_success("k@example.com", None, "claude-fable-5-1");
    manager.record_attempt("k@example.com");
    manager.record_failure("k@example.com", "upstream", Some("boom"));
    manager.record_attempt("k@example.com");
    manager.record_refresh_exhausted("k@example.com", "expired");

    let snapshot = &manager.snapshots()[0];
    let requests = snapshot["totalRequests"].as_i64().expect("requests");
    let ok = snapshot["totalSuccesses"].as_i64().expect("successes");
    let failed = snapshot["totalFailures"].as_i64().expect("failures");
    assert_eq!(
        requests,
        ok + failed,
        "an attempt with no outcome leaves an unaccountable gap: \
         {requests} requests, {ok} ok, {failed} failed"
    );
    // The same reconciliation inside the daily buckets.
    let day = &snapshot["days"][0];
    assert_eq!(
        day["requests"].as_i64().expect("day requests"),
        day["successes"].as_i64().expect("day ok") + day["failures"].as_i64().expect("day failed"),
        "daily buckets must reconcile too: {day}"
    );
}

/// A refusal must survive a restart like every other outcome. Its
/// `record_attempt` is persisted; if the refusal is not, the reloaded
/// state shows a request with no outcome — the permanent gap this seam
/// was written to close.
#[tokio::test]
async fn a_refusal_survives_a_restart() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");

    manager.record_attempt("k@example.com");
    manager.record_refusal("k@example.com");

    let mut rebuilt = never_refresh_manager(tmp.path().to_path_buf());
    rebuilt.load().expect("reload");
    let snapshot = &rebuilt.snapshots()[0];
    let requests = snapshot["totalRequests"].as_i64().expect("requests");
    let ok = snapshot["totalSuccesses"].as_i64().expect("ok");
    let failed = snapshot["totalFailures"].as_i64().expect("failed");
    assert_eq!(
        requests,
        ok + failed,
        "a refusal lost on restart leaves a permanent gap: \
         {requests} requests, {ok} ok, {failed} failed"
    );
}

/// AC-4's window is 90 days inclusive of today. Nothing pinned the size
/// itself: `RETENTION_DAYS` could move to 80 or 100 and every test stayed
/// green, because the others test the comparison, not the span.
#[tokio::test]
async fn the_retention_window_is_ninety_days_inclusive() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let usage_file = tmp.path().join("commandcode").join("usage.json");
    let day = |back: i64| {
        (chrono::Local::now() - chrono::Duration::days(back))
            .format("%Y-%m-%d")
            .to_string()
    };
    // 89 back is the oldest day inside a 90-day inclusive window; 90 is
    // the first one outside it.
    let (inside, edge, outside) = (day(88), day(89), day(90));
    fs::create_dir_all(usage_file.parent().expect("parent")).expect("mkdir");
    fs::write(
        &usage_file,
        json!({
            "k@example.com": {
                "days": {
                    inside.clone(): {"requests": 1},
                    edge.clone(): {"requests": 1},
                    outside.clone(): {"requests": 1}
                }
            }
        })
        .to_string(),
    )
    .expect("write");

    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    manager.record_attempt("k@example.com");

    let stored = persisted(&usage_file);
    let days = stored["k@example.com"]["days"]
        .as_object()
        .expect("days map");
    assert!(days.contains_key(&inside), "88 days back must survive");
    assert!(days.contains_key(&edge), "89 days back is the window edge");
    assert!(
        !days.contains_key(&outside),
        "90 days back is outside a 90-day inclusive window: {days:?}"
    );
}

/// Counters written by an older binary can hold attempts that never
/// reached an outcome. At load there is nothing in flight, so the gap is
/// knowable: a request that did not succeed failed. Reconciling on load
/// makes the panels add up instead of carrying the old leak forever.
#[tokio::test]
async fn a_historical_gap_is_reconciled_on_load() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let usage_file = tmp.path().join("commandcode").join("usage.json");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    fs::create_dir_all(usage_file.parent().expect("parent")).expect("mkdir");
    fs::write(
        &usage_file,
        json!({
            "k@example.com": {
                "total_requests": 1530,
                "total_successes": 1524,
                "total_failures": 0,
                "days": {
                    today.clone(): {"requests": 11, "successes": 10, "failures": 0}
                }
            }
        })
        .to_string(),
    )
    .expect("write");

    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    let snapshot = &manager.snapshots()[0];

    let requests = snapshot["totalRequests"].as_i64().expect("requests");
    let ok = snapshot["totalSuccesses"].as_i64().expect("ok");
    let failed = snapshot["totalFailures"].as_i64().expect("failed");
    assert_eq!(requests, 1530, "attempts are not rewritten");
    assert_eq!(ok, 1524, "successes are not invented");
    assert_eq!(failed, 6, "the gap becomes what it was: failures");
    assert_eq!(requests, ok + failed);

    let day = &snapshot["days"][0];
    assert_eq!(day["requests"], 11);
    assert_eq!(day["successes"], 10);
    assert_eq!(day["failures"], 1, "the bucket reconciles too");
}

/// The payload never serves a bucket outside the window, even on a
/// process that has recorded nothing since the window moved past it.
/// `today()` trims, but only a recorder calls it.
#[tokio::test]
async fn an_idle_process_does_not_serve_stale_buckets() {
    let tmp = tempdir().expect("tempdir");
    save_token(tmp.path(), &static_token("k@example.com")).expect("save token");
    let usage_file = tmp.path().join("commandcode").join("usage.json");
    let stale = (chrono::Local::now() - chrono::Duration::days(200))
        .format("%Y-%m-%d")
        .to_string();
    fs::create_dir_all(usage_file.parent().expect("parent")).expect("mkdir");
    fs::write(
        &usage_file,
        json!({"k@example.com": {"days": {stale.clone(): {"requests": 5, "failures": 5}}}})
            .to_string(),
    )
    .expect("write");

    let mut manager = never_refresh_manager(tmp.path().to_path_buf());
    manager.load().expect("load");
    // No recorder runs: the process is idle.
    let dates: Vec<String> = manager.snapshots()[0]["days"]
        .as_array()
        .expect("days")
        .iter()
        .filter_map(|day| day["date"].as_str().map(str::to_string))
        .collect();
    assert!(
        !dates.contains(&stale),
        "an idle process serves a bucket outside the window: {dates:?}"
    );
}
