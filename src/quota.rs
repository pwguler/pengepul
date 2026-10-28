//! Per-account quota: the vendor's own utilization numbers, normalized to a
//! percent and a reset instant.
//!
//! Two sources feed one snapshot per account. Every response's headers carry
//! the numbers for an account that is serving traffic; a poll of the vendor's
//! usage endpoint covers an account that is idle. Whichever observed later
//! wins. A window with no number is absent, never zero: the panels print
//! `n/a`, because a made-up zero reads as headroom.
//!
//! Nothing here talks to the network or reads a clock. The callers pass the
//! observation instant in, so the parsers and the store stay testable.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::http::HeaderMap;
use chrono::{DateTime, Datelike as _, Local, TimeZone as _, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::ProviderId;

/// Window keys, and what the panels call them. One vocabulary across the
/// store, the admin payload and the tray: the key is the wire name.
pub const WINDOW_5H: &str = "5h";
pub const WINDOW_WEEK: &str = "week";
pub const WINDOW_MONTH: &str = "month";

/// The providers this feature covers, and nothing else. A pool outside this
/// list carries no `quota` field and prints no bar rows.
#[must_use]
pub fn quota_windows(provider: &str) -> &'static [&'static str] {
    match provider {
        // Claude Code shows two windows and no monthly one.
        "anthropic" => &[WINDOW_5H, WINDOW_WEEK],
        "commandcode" | "opencode" => &[WINDOW_5H, WINDOW_WEEK, WINDOW_MONTH],
        _ => &[],
    }
}

/// The width of a quota bar, in cells. Wider than the token share bar
/// because utilization moves in single points and 20 cells show five of those.
pub const BAR_WIDTH: usize = 20;

/// One window's utilization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// Percent used, 0..=100 (a vendor may report more; the panels clamp).
    #[serde(rename = "usedPct")]
    pub used_pct: f64,
    /// When the window rolls, Unix epoch seconds.
    #[serde(rename = "resetsAt", default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// The vendor's own word for the window: `allowed`, `allowed_warning`,
    /// `rejected`, `exceeded`, `ok`. Display reads only the limiting values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Where one account's snapshot came from, so a stale figure can be traced to
/// the call that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    /// `headers`, `usage-api` or `billing`.
    pub source: String,
    /// When the relay observed it, RFC 3339 UTC.
    #[serde(rename = "observedAt")]
    pub observed_at: String,
    /// The window the vendor says is binding, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representative: Option<String>,
    pub windows: BTreeMap<String, QuotaWindow>,
}

impl QuotaSnapshot {
    fn new(source: &str, observed_at: &str) -> Self {
        Self {
            source: source.to_string(),
            observed_at: observed_at.to_string(),
            representative: None,
            windows: BTreeMap::new(),
        }
    }

    /// Empty snapshots are dropped by the callers: a poll that parsed nothing
    /// must not overwrite the numbers an earlier one produced.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// The admin payload's shape for this snapshot.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn header_f64(headers: &HeaderMap, name: &str) -> Option<f64> {
    header(headers, name)?.parse().ok()
}

/// Anthropic's undocumented `anthropic-ratelimit-unified-*` response headers,
/// the ones Claude Code reads for its status line. Utilization is a fraction
/// there (`0.77`), so it is scaled to the percent every other source reports.
/// Reset is Unix epoch seconds. Unknown headers are ignored.
#[must_use]
pub fn anthropic_headers(headers: &HeaderMap, observed_at: &str) -> Option<QuotaSnapshot> {
    let mut snapshot = QuotaSnapshot::new("headers", observed_at);
    for (field, window) in [("5h", WINDOW_5H), ("7d", WINDOW_WEEK)] {
        let Some(utilization) = header_f64(
            headers,
            &format!("anthropic-ratelimit-unified-{field}-utilization"),
        ) else {
            continue;
        };
        snapshot.windows.insert(
            window.to_string(),
            QuotaWindow {
                used_pct: utilization * 100.0,
                resets_at: header_f64(
                    headers,
                    &format!("anthropic-ratelimit-unified-{field}-reset"),
                )
                .map(epoch_seconds),
                status: header(
                    headers,
                    &format!("anthropic-ratelimit-unified-{field}-status"),
                )
                .map(ToOwned::to_owned),
            },
        );
    }
    snapshot.representative =
        header(headers, "anthropic-ratelimit-unified-representative-claim").map(ToOwned::to_owned);
    (!snapshot.is_empty()).then_some(snapshot)
}

/// `GET /api/oauth/usage`: `five_hour` and `seven_day`, each with a percent
/// utilization and an ISO reset. Claude Code's own numbers.
#[must_use]
pub fn anthropic_usage(body: &Value, observed_at: &str) -> Option<QuotaSnapshot> {
    let mut snapshot = QuotaSnapshot::new("usage-api", observed_at);
    for (field, window) in [("five_hour", WINDOW_5H), ("seven_day", WINDOW_WEEK)] {
        if let Some(entry) = body.get(field).filter(|entry| !entry.is_null()) {
            insert_window(&mut snapshot, window, entry, "utilization", "resets_at");
        }
    }
    (!snapshot.is_empty()).then_some(snapshot)
}

/// `GET /zen/go/v1/usage`: rolling (5h), weekly and monthly, each a percent
/// plus an ISO reset. The one provider that reports the percent directly.
#[must_use]
pub fn opencode_usage(body: &Value, observed_at: &str) -> Option<QuotaSnapshot> {
    let mut snapshot = QuotaSnapshot::new("usage-api", observed_at);
    let usage = body.get("usage").unwrap_or(body);
    for (field, window) in [
        ("rolling", WINDOW_5H),
        ("weekly", WINDOW_WEEK),
        ("monthly", WINDOW_MONTH),
    ] {
        if let Some(entry) = usage.get(field).filter(|entry| !entry.is_null()) {
            insert_window(&mut snapshot, window, entry, "percent", "resetsAt");
        }
    }
    (!snapshot.is_empty()).then_some(snapshot)
}

/// The monthly credit pool each Command Code plan grants, by `planId`.
///
/// The billing response reports what is left of the pool, not how big it is,
/// and no route publishes the size. These are the plans' credit grants, and
/// the live account is what corroborates this table: a GOAT pool of 70 less
/// the period's spend equals the remaining credits the API reports, to within
/// a tenth. A plan id absent here leaves the month window unmeasured rather
/// than guessed — that is what `n/a` is for.
fn commandcode_monthly_grant(plan_id: &str) -> Option<f64> {
    // A plan id carries a version suffix (`individual-go-v1`) that names the
    // same grant.
    let base = plan_id.split("-v").next().unwrap_or(plan_id);
    match base {
        "individual-go" => Some(10.0),
        "individual-goat" => Some(70.0),
        "individual-provider" => Some(15.0),
        "individual-max" => Some(150.0),
        "individual-ultra" => Some(300.0),
        "teams-pro" => Some(40.0),
        _ => None,
    }
}

/// `/alpha/billing/credits`, with `/alpha/billing/subscriptions` when it
/// answered. The rolling windows report dollars spent against a dollar cap;
/// the month is the credit pool, whose remaining balance the same response
/// carries. `subscription` is the `data` object of the subscriptions answer.
#[must_use]
pub fn commandcode_usage(
    credits: &Value,
    subscription: Option<&Value>,
    observed_at: &str,
) -> Option<QuotaSnapshot> {
    let mut snapshot = QuotaSnapshot::new("billing", observed_at);
    if let Some(limits) = credits.get("windowLimits") {
        for (field, window) in [("fiveHour", WINDOW_5H), ("weekly", WINDOW_WEEK)] {
            let Some(entry) = limits.get(field).filter(|entry| !entry.is_null()) else {
                continue;
            };
            let (Some(used), Some(cap)) = (number(entry, "used"), number(entry, "cap")) else {
                continue;
            };
            if cap <= 0.0 {
                continue;
            }
            let exceeded = entry
                .get("exceeded")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            snapshot.windows.insert(
                window.to_string(),
                QuotaWindow {
                    used_pct: used / cap * 100.0,
                    // Millisecond epoch, the one unit in this file that is
                    // not seconds.
                    resets_at: number(entry, "resetAt").map(millis_to_seconds),
                    status: exceeded.then(|| "exceeded".to_string()),
                },
            );
        }
    }
    let plan_id = subscription
        .and_then(|data| data.get("planId"))
        .and_then(Value::as_str);
    let pool = &credits["credits"];
    let grant = number(pool, "monthlyCreditsGranted")
        .or_else(|| number(credits, "monthlyCreditsGranted"))
        .or_else(|| plan_id.and_then(commandcode_monthly_grant));
    if let (Some(grant), Some(remaining)) = (grant, number(pool, "monthlyCredits"))
        && grant > 0.0
    {
        let used = (grant - remaining).clamp(0.0, grant);
        snapshot.windows.insert(
            WINDOW_MONTH.to_string(),
            QuotaWindow {
                used_pct: used / grant * 100.0,
                resets_at: subscription
                    .and_then(|data| data.get("currentPeriodEnd"))
                    .and_then(Value::as_str)
                    .and_then(parse_iso_epoch),
                status: None,
            },
        );
    }
    (!snapshot.is_empty()).then_some(snapshot)
}

/// One window from an object that names its percent and its reset.
fn insert_window(
    snapshot: &mut QuotaSnapshot,
    window: &str,
    entry: &Value,
    percent_field: &str,
    reset_field: &str,
) {
    let Some(used_pct) = number(entry, percent_field) else {
        return;
    };
    snapshot.windows.insert(
        window.to_string(),
        QuotaWindow {
            used_pct,
            resets_at: entry
                .get(reset_field)
                .and_then(Value::as_str)
                .and_then(parse_iso_epoch),
            status: entry
                .get("status")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        },
    );
}

fn number(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_f64)
}

/// A fractional epoch second as the whole one a reset instant is kept in.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a reset instant is kept in whole seconds; the fraction is sub-second noise"
)]
fn epoch_seconds(seconds: f64) -> i64 {
    seconds as i64
}

/// A millisecond epoch, truncated to the second a reset instant is kept in.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a reset instant is kept in whole seconds; the fraction is sub-second noise"
)]
fn millis_to_seconds(millis: f64) -> i64 {
    (millis / 1000.0) as i64
}

/// When a snapshot was observed, as Unix epoch seconds. A string that does not
/// parse yields `None`; the poller reads that as "no known age, refresh it".
#[must_use]
pub fn observed_epoch(snapshot: &QuotaSnapshot) -> Option<i64> {
    parse_iso_epoch(&snapshot.observed_at)
}

/// An RFC 3339 instant as Unix epoch seconds.
fn parse_iso_epoch(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp())
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub(crate) fn quota_path(auth_dir: &Path, provider: &ProviderId) -> PathBuf {
    auth_dir.join(provider.storage_dir()).join("quota.json")
}

/// A provider's snapshots by account email. A missing or corrupt file is an
/// empty map: quota is observability, never worth failing a startup over.
pub(crate) fn load_quota(
    auth_dir: &Path,
    provider: &ProviderId,
) -> BTreeMap<String, QuotaSnapshot> {
    let path = quota_path(auth_dir, provider);
    let Ok(raw) = fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&raw) else {
        return BTreeMap::new();
    };
    root.get("accounts")
        .and_then(Value::as_object)
        .map(|accounts| {
            accounts
                .iter()
                .filter_map(|(email, entry)| {
                    let snapshot = serde_json::from_value::<QuotaSnapshot>(entry.clone()).ok()?;
                    (!snapshot.is_empty()).then(|| (email.clone(), snapshot))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Write a provider's snapshots, atomically and private, the way `usage.json`
/// is written: a reader never sees half a file, and the file names accounts
/// the operator has.
pub(crate) fn save_quota(
    auth_dir: &Path,
    provider: &ProviderId,
    quota: &BTreeMap<String, QuotaSnapshot>,
) -> Result<()> {
    let path = quota_path(auth_dir, provider);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let mut accounts = serde_json::Map::new();
    for (email, snapshot) in quota {
        accounts.insert(email.clone(), snapshot.to_json());
    }
    let temp = path.with_extension("json.tmp");
    let file = serde_json::json!({"accounts": accounts});
    fs::write(&temp, serde_json::to_string_pretty(&file)?)
        .with_context(|| format!("failed to write {}", temp.display()))?;
    fs::rename(&temp, &path)
        .with_context(|| format!("failed to move {} into place", temp.display()))?;
    set_private(&path)?;
    Ok(())
}

#[cfg(unix)]
fn set_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to chmod {}", path.display()))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Display rules
// ---------------------------------------------------------------------------

/// A window whose reset instant has passed belongs to a cycle that already
/// rolled: the number describes the old window, so the panels print `n/a`
/// until a later observation replaces it.
#[must_use]
pub fn rolled(window: &QuotaWindow, now: i64) -> bool {
    window.resets_at.is_some_and(|at| at <= now)
}

/// At the cap, or the vendor says so.
#[must_use]
pub fn limited(window: &QuotaWindow) -> bool {
    window.used_pct >= 100.0
        || matches!(
            window.status.as_deref(),
            Some("rejected" | "exceeded" | "limited" | "blocked")
        )
}

/// A [`BAR_WIDTH`]-cell bar, whole cells filled in proportion. Clamped both
/// ways: a vendor reporting 104% must not overrun the panel.
#[must_use]
pub fn bar(used_pct: f64) -> String {
    bar_cells(used_pct, BAR_WIDTH)
}

/// The same bar at a width the caller's media can hold. The rich panel is a
/// fixed 64 columns, so it carries a shorter bar than a plain line does.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "a bar's width is a display column count, orders of magnitude under 2^52"
)]
pub fn bar_cells(used_pct: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let clamped = used_pct.clamp(0.0, 100.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=100 and rounded, so the cast is exact"
    )]
    let filled = ((clamped / 100.0 * width as f64).round() as usize).min(width);
    let mut bar = String::with_capacity(width * 3);
    bar.extend(std::iter::repeat_n('█', filled));
    bar.extend(std::iter::repeat_n('░', width - filled));
    bar
}

/// `72%`, right-aligned into a four-cell field so percentages line up.
#[must_use]
pub fn percent_text(used_pct: f64) -> String {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a percentage is printed as a whole number; the cast rounds"
    )]
    let whole = used_pct.clamp(0.0, 100.0).round() as i64;
    format!("{whole:>3}%")
}

/// `reset 16:50 (18m)`: local wall-clock time plus a compact countdown.
/// The date is named only when it is not today: `Mon 07:00` inside a week,
/// `Nov 1 00:00` beyond it.
#[must_use]
pub fn reset_text(resets_at: i64, now: i64) -> String {
    let Some(at) = Utc.timestamp_opt(resets_at, 0).single() else {
        return String::new();
    };
    let local = at.with_timezone(&Local);
    let today = Utc
        .timestamp_opt(now, 0)
        .single()
        .map(|now| now.with_timezone(&Local).date_naive());
    let remaining = resets_at - now;
    let clock = local.format("%H:%M").to_string();
    let when = if Some(local.date_naive()) == today {
        clock
    } else if remaining < 7 * 86_400 {
        format!("{} {clock}", local.format("%a"))
    } else {
        format!("{} {} {clock}", local.format("%b"), local.day())
    };
    format!("{when} ({})", countdown(remaining))
}

/// `18m`, `4h28m`, `4d14h`: the countdown's own units, under a minute in
/// seconds so a reset one tick away does not read as `0m`.
fn countdown(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{}s", seconds % 60)
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "these compare a parsed number against the literal the vendor sent: exact by construction"
)]
mod tests {
    use super::{
        BAR_WIDTH, QuotaSnapshot, QuotaWindow, WINDOW_5H, WINDOW_MONTH, WINDOW_WEEK,
        anthropic_headers, anthropic_usage, bar, commandcode_usage, countdown, limited, load_quota,
        opencode_usage, percent_text, quota_windows, reset_text, rolled, save_quota,
    };
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        headers
    }

    #[test]
    fn every_provider_covers_the_windows_it_reports() {
        assert_eq!(quota_windows("anthropic"), &[WINDOW_5H, WINDOW_WEEK]);
        assert_eq!(
            quota_windows("commandcode"),
            &[WINDOW_5H, WINDOW_WEEK, WINDOW_MONTH]
        );
        assert_eq!(
            quota_windows("opencode"),
            &[WINDOW_5H, WINDOW_WEEK, WINDOW_MONTH]
        );
        assert!(quota_windows("codex").is_empty());
        assert!(quota_windows("groq").is_empty());
    }

    #[test]
    fn anthropic_headers_scale_the_fraction_and_keep_the_reset() {
        let headers = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.77"),
            ("anthropic-ratelimit-unified-5h-reset", "1793188800"),
            ("anthropic-ratelimit-unified-5h-status", "allowed"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.85"),
            ("anthropic-ratelimit-unified-7d-reset", "1793293200"),
            ("anthropic-ratelimit-unified-7d-status", "allowed_warning"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "seven_day",
            ),
            ("anthropic-ratelimit-unified-upgrade-paths", "upgrade_plan"),
        ]);
        let snapshot = anthropic_headers(&headers, "2026-10-28T09:47:39Z").expect("snapshot");
        assert_eq!(snapshot.source, "headers");
        assert_eq!(snapshot.representative.as_deref(), Some("seven_day"));
        assert_eq!(snapshot.windows[WINDOW_5H].used_pct, 77.0);
        assert_eq!(snapshot.windows[WINDOW_5H].resets_at, Some(1_793_188_800));
        assert_eq!(
            snapshot.windows[WINDOW_5H].status.as_deref(),
            Some("allowed")
        );
        assert_eq!(snapshot.windows[WINDOW_WEEK].used_pct, 85.0);
        assert_eq!(
            snapshot.windows[WINDOW_WEEK].status.as_deref(),
            Some("allowed_warning")
        );
    }

    #[test]
    fn anthropic_headers_without_utilization_are_no_snapshot() {
        let status_only = headers(&[("anthropic-ratelimit-unified-status", "allowed")]);
        assert!(anthropic_headers(&status_only, "2026-10-28T09:47:39Z").is_none());
        // A single window still yields a snapshot; the other is simply absent.
        let one_window = headers(&[("anthropic-ratelimit-unified-5h-utilization", "1")]);
        let snapshot = anthropic_headers(&one_window, "2026-10-28T09:47:39Z").expect("snapshot");
        assert_eq!(snapshot.windows[WINDOW_5H].used_pct, 100.0);
        assert!(!snapshot.windows.contains_key(WINDOW_WEEK));
    }

    #[test]
    fn anthropic_usage_reads_percent_and_iso_reset() {
        let body = json!({
            "five_hour": {
                "utilization": 77.0,
                "resets_at": "2026-10-28T12:00:00.473176+00:00",
                "limit_dollars": null
            },
            "seven_day": {
                "utilization": 86.0,
                "resets_at": "2026-10-29T17:00:00+00:00"
            },
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": false}
        });
        let snapshot = anthropic_usage(&body, "2026-10-28T09:48:00Z").expect("snapshot");
        assert_eq!(snapshot.source, "usage-api");
        assert_eq!(snapshot.windows[WINDOW_5H].used_pct, 77.0);
        assert_eq!(snapshot.windows[WINDOW_5H].resets_at, Some(1_793_188_800));
        assert_eq!(snapshot.windows[WINDOW_WEEK].used_pct, 86.0);
        assert_eq!(snapshot.windows[WINDOW_WEEK].resets_at, Some(1_793_293_200));
    }

    #[test]
    fn opencode_usage_reads_the_three_windows_it_reports() {
        let body = json!({"usage": {
            "rolling": {"status": "ok", "percent": 11, "resetsAt": "2026-10-28T10:03:11.000Z"},
            "weekly": {"status": "ok", "percent": 24, "resetsAt": "2026-11-02T00:00:00.000Z"},
            "monthly": {"status": "ok", "percent": 93, "resetsAt": "2026-11-05T01:35:30.000Z"}
        }});
        let snapshot = opencode_usage(&body, "2026-10-28T09:48:02Z").expect("snapshot");
        assert_eq!(snapshot.windows[WINDOW_5H].used_pct, 11.0);
        assert_eq!(snapshot.windows[WINDOW_WEEK].used_pct, 24.0);
        assert_eq!(snapshot.windows[WINDOW_MONTH].used_pct, 93.0);
        assert_eq!(
            snapshot.windows[WINDOW_MONTH].resets_at,
            Some(1_793_842_530)
        );
        assert_eq!(snapshot.windows[WINDOW_5H].status.as_deref(), Some("ok"));
    }

    #[test]
    fn commandcode_usage_turns_spend_against_a_cap_into_a_percent() {
        let credits = json!({
            "credits": {"monthlyCredits": 45.58, "purchasedCredits": 0, "freeCredits": 0},
            "windowLimits": {
                "limited": true,
                "fiveHour": {"used": 0.54, "cap": 14, "exceeded": false, "resetAt": 1_793_182_065_277i64},
                "weekly": {"used": 4.44, "cap": 35, "exceeded": false, "resetAt": 1_793_546_903_002i64}
            }
        });
        let subscription = json!({
            "planId": "individual-goat",
            "currentPeriodEnd": "2026-11-11T15:10:29.000Z"
        });
        let snapshot =
            commandcode_usage(&credits, Some(&subscription), "2026-10-28T09:48:00Z").expect("snap");
        assert_eq!(snapshot.source, "billing");
        // 0.54 of 14 is 3.857%, 4.44 of 35 is 12.686%.
        assert!((snapshot.windows[WINDOW_5H].used_pct - 3.857).abs() < 0.01);
        assert_eq!(snapshot.windows[WINDOW_5H].resets_at, Some(1_793_182_065));
        assert!((snapshot.windows[WINDOW_WEEK].used_pct - 12.685_7).abs() < 0.01);
        // The month is the credit pool: 70 granted less 45.58 left is 34.9% used.
        assert!((snapshot.windows[WINDOW_MONTH].used_pct - 34.886).abs() < 0.01);
        assert_eq!(
            snapshot.windows[WINDOW_MONTH].resets_at,
            Some(1_794_409_829)
        );
    }

    #[test]
    fn commandcode_month_is_absent_when_the_pool_size_is_unknown() {
        // An unknown plan id has no published grant: the month window is
        // unmeasured rather than divided by a guessed denominator. The two
        // rolling windows still resolve, because their caps come from the API.
        let credits = json!({
            "credits": {"monthlyCredits": 12.5},
            "windowLimits": {
                "fiveHour": {"used": 1, "cap": 14, "resetAt": 1_793_182_065_277i64}
            }
        });
        let subscription = json!({"planId": "individual-mystery"});
        let snapshot =
            commandcode_usage(&credits, Some(&subscription), "2026-10-28T09:48:00Z").expect("snap");
        assert!(snapshot.windows.contains_key(WINDOW_5H));
        assert!(!snapshot.windows.contains_key(WINDOW_MONTH));
    }

    #[test]
    fn commandcode_exceeded_window_is_limited() {
        let credits = json!({
            "credits": {"monthlyCredits": 0},
            "windowLimits": {
                "fiveHour": {"used": 14, "cap": 14, "exceeded": true, "resetAt": 1_793_182_065_277i64}
            }
        });
        let snapshot = commandcode_usage(&credits, None, "2026-10-28T09:48:00Z").expect("snapshot");
        let window = &snapshot.windows[WINDOW_5H];
        assert!(limited(window));
        assert_eq!(window.status.as_deref(), Some("exceeded"));
        // No subscriptions answer: the month stays unmeasured.
        assert!(!snapshot.windows.contains_key(WINDOW_MONTH));
    }

    #[test]
    fn a_window_past_its_reset_has_rolled() {
        let window = QuotaWindow {
            used_pct: 40.0,
            resets_at: Some(1_000),
            status: None,
        };
        assert!(rolled(&window, 1_000));
        assert!(!rolled(&window, 999));
        let no_reset = QuotaWindow {
            used_pct: 40.0,
            resets_at: None,
            status: None,
        };
        assert!(!rolled(&no_reset, 10_000));
    }

    #[test]
    fn the_bar_fills_in_fifths_and_never_overruns() {
        assert_eq!(bar(0.0).chars().count(), BAR_WIDTH);
        assert_eq!(bar(0.0), "░".repeat(BAR_WIDTH));
        assert_eq!(bar(100.0), "█".repeat(BAR_WIDTH));
        assert_eq!(bar(72.0), format!("{}{}", "█".repeat(14), "░".repeat(6)));
        // 104% is still one bar: the panel cannot hold a cell it does not have.
        assert_eq!(bar(104.0), "█".repeat(BAR_WIDTH));
        assert_eq!(bar(-3.0), "░".repeat(BAR_WIDTH));
    }

    #[test]
    fn the_percent_is_right_aligned_into_a_four_cell_field() {
        assert_eq!(percent_text(72.4), " 72%");
        assert_eq!(percent_text(7.0), "  7%");
        assert_eq!(percent_text(100.0), "100%");
    }

    #[test]
    fn the_countdown_drops_empty_units() {
        assert_eq!(countdown(30), "30s");
        assert_eq!(countdown(18 * 60), "18m");
        assert_eq!(countdown(4 * 3_600 + 28 * 60), "4h28m");
        assert_eq!(countdown(4 * 86_400 + 14 * 3_600), "4d14h");
        assert_eq!(countdown(-5), "0s");
    }

    #[test]
    fn the_reset_text_names_the_day_only_when_it_is_not_today() {
        // 10:00 UTC on a fixed instant; a reset later the same local day
        // prints only the clock, one within the week names the weekday, and one
        // beyond it names the date.
        let now = 1_793_181_600; // 2026-10-28T10:00:00Z
        let same_day = now + 3 * 3_600;
        let text = reset_text(same_day, now);
        let (when, count) = text.split_once(" (").expect("countdown");
        assert!(!when.contains(' '), "{text}");
        assert_eq!(count, "3h0m)");
        let next_week = now + 4 * 86_400 + 14 * 3_600;
        let text = reset_text(next_week, now);
        let (when, count) = text.split_once(" (").expect("countdown");
        assert_eq!(when.matches(' ').count(), 1, "{text}");
        assert_eq!(count, "4d14h)");
        let next_month = now + 10 * 86_400;
        let text = reset_text(next_month, now);
        let (when, count) = text.split_once(" (").expect("countdown");
        assert_eq!(when.matches(' ').count(), 2, "{text}");
        assert_eq!(count, "10d0h)");
    }

    #[test]
    fn a_snapshot_round_trips_through_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = crate::types::ProviderId::anthropic();
        let snapshot = QuotaSnapshot {
            source: "headers".to_string(),
            observed_at: "2026-10-28T09:47:39Z".to_string(),
            representative: Some("seven_day".to_string()),
            windows: BTreeMap::from([
                (
                    WINDOW_5H.to_string(),
                    QuotaWindow {
                        used_pct: 77.0,
                        resets_at: Some(1_793_188_800),
                        status: Some("allowed".to_string()),
                    },
                ),
                (
                    WINDOW_WEEK.to_string(),
                    QuotaWindow {
                        used_pct: 85.0,
                        resets_at: None,
                        status: None,
                    },
                ),
            ]),
        };
        let quota = BTreeMap::from([("a@x.com".to_string(), snapshot.clone())]);
        save_quota(dir.path(), &provider, &quota).expect("save");
        assert_eq!(load_quota(dir.path(), &provider), quota);
        // The file is private: it names the accounts an operator has.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let path = super::quota_path(dir.path(), &provider);
            let mode = std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_corrupt_store_reads_as_no_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = crate::types::ProviderId::anthropic();
        let path = super::quota_path(dir.path(), &provider);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, "{not json").expect("write");
        assert!(load_quota(dir.path(), &provider).is_empty());
    }
}
