//! The dashboard's two report payloads beyond `/admin/accounts`: the relay's
//! own health, and a request/token window aggregated across every Pool.
//!
//! Pure over the admin snapshots and the date handed in, so both payloads are
//! tested without a running relay or a clock.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::tokens::Peak;
use crate::utils::now_iso;

/// One account's counters for one local day, as `/admin/accounts` reports them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counters {
    requests: i64,
    successes: i64,
    failures: i64,
    /// The prompt after the cache was taken out, which is how every Provider's
    /// counter lands in the store (ADR-0023).
    input: i64,
    cache_read: i64,
    cache_creation: i64,
    output: i64,
    reasoning: i64,
}

impl Counters {
    fn add(&mut self, other: Self) {
        self.requests += other.requests;
        self.successes += other.successes;
        self.failures += other.failures;
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_creation += other.cache_creation;
        self.output += other.output;
        self.reasoning += other.reasoning;
    }

    /// The whole prompt: what was read from cache, what was written to it, and
    /// what was sent raw. The CLI's `input` figure.
    fn prompt(&self) -> i64 {
        self.input + self.cache_read + self.cache_creation
    }

    /// The part of the prompt served from cache, and the part that was not.
    fn uncached(&self) -> i64 {
        self.input + self.cache_creation
    }

    /// What crossed the relay: prompt plus output. Reasoning is inside output
    /// and is never added again (ADR-0024).
    fn carried(&self) -> i64 {
        self.prompt() + self.output
    }

    fn from_day(day: &Value) -> Self {
        Self {
            requests: field(day, "requests"),
            successes: field(day, "successes"),
            failures: field(day, "failures"),
            input: field(day, "inputTokens"),
            cache_read: field(day, "cacheReadInputTokens"),
            cache_creation: field(day, "cacheCreationInputTokens"),
            output: field(day, "outputTokens"),
            reasoning: field(day, "reasoningOutputTokens"),
        }
    }

    fn from_account(account: &Value) -> Self {
        Self {
            requests: field(account, "totalRequests"),
            successes: field(account, "totalSuccesses"),
            failures: field(account, "totalFailures"),
            input: field(account, "totalInputTokens"),
            cache_read: field(account, "totalCacheReadInputTokens"),
            cache_creation: field(account, "totalCacheCreationInputTokens"),
            output: field(account, "totalOutputTokens"),
            reasoning: field(account, "totalReasoningOutputTokens"),
        }
    }

    fn json(&self) -> Value {
        json!({
            "requests": self.requests,
            "successes": self.successes,
            "failures": self.failures,
            "inputTokens": self.prompt(),
            "cachedTokens": self.cache_read,
            "cachedPct": percent(self.cache_read, self.prompt()),
            "uncachedInputTokens": self.uncached(),
            "outputTokens": self.output,
            "reasoningTokens": self.reasoning,
            "reasoningPct": percent(self.reasoning, self.output),
            "totalTokens": self.carried(),
        })
    }
}

fn field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// `part` as a rounded percentage of `whole`, one decimal place. `None` when
/// there is no whole to divide by: a zero percentage would claim a share.
fn percent(part: i64, whole: i64) -> Option<f64> {
    if whole <= 0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a percentage is a display figure; token counters near 2^53 are not a case"
    )]
    let value = (part as f64) / (whole as f64) * 100.0;
    Some((value * 10.0).round() / 10.0)
}

/// Every account's counters for the local day `today`.
fn requests_on(account: &Value, today: &str) -> i64 {
    account
        .get("days")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|day| day.get("date").and_then(Value::as_str) == Some(today))
        .map(|day| field(day, "requests"))
        .sum()
}

/// The relay's health: build, uptime, where it listens, and what each pool can
/// serve right now. `available` is the same rule the account rows use, so the
/// dashboard's counts and `pengepul status` cannot disagree.
#[must_use]
pub(crate) fn status_json(
    pools: &[(String, Vec<Value>)],
    version: &str,
    uptime_seconds: u64,
    address: &str,
    today: &str,
) -> Value {
    let mut providers = Map::new();
    let mut total = 0_usize;
    let mut available = 0_usize;
    let mut requests_today = 0_i64;
    for (provider, accounts) in pools {
        let mut pool_available = 0_usize;
        for account in accounts {
            if account.get("available").and_then(Value::as_bool) == Some(true) {
                pool_available += 1;
            }
            requests_today += requests_on(account, today);
        }
        total += accounts.len();
        available += pool_available;
        providers.insert(
            provider.clone(),
            json!({
                "available": pool_available,
                "total": accounts.len(),
            }),
        );
    }
    json!({
        "version": version,
        "uptimeSeconds": uptime_seconds,
        "address": address,
        "accounts": {"available": available, "total": total},
        "requestsToday": requests_today,
        "providers": providers,
        "generatedAt": now_iso(),
    })
}

/// The last `days` local dates ending on `today`, oldest first. Date math on the
/// `YYYY-MM-DD` string keeps the helper clock-free.
fn window_dates(today: &str, days: usize) -> Vec<String> {
    let Ok(end) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
        return Vec::new();
    };
    let days = i64::try_from(days).unwrap_or(1).max(1);
    (0..days)
        .rev()
        .map(|offset| {
            (end - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect()
}

/// A request/token window over every Pool, plus the all-time figures beside it.
/// Days with no traffic are present with zeros: the chart has one column per
/// calendar day, not one per recorded day.
///
/// `peak` is the stored best day, and it wins over a better retained day only
/// when it is at least as good: the store is written whenever a retained day
/// beats it, so the two agree unless a hand-edited file says otherwise.
#[must_use]
pub(crate) fn usage_json(
    snapshots: &[Value],
    peak: Option<&Peak>,
    days: usize,
    today: &str,
) -> Value {
    let mut by_day: BTreeMap<String, Counters> = BTreeMap::new();
    let mut all_time = Counters::default();
    for account in snapshots {
        all_time.add(Counters::from_account(account));
        for day in account
            .get("days")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            let Some(date) = day.get("date").and_then(Value::as_str) else {
                continue;
            };
            // The entry exists because the day was recorded; its value may be
            // zero.
            by_day
                .entry(date.to_string())
                .or_default()
                .add(Counters::from_day(day));
        }
    }

    let window = window_dates(today, days);
    let mut window_totals = Counters::default();
    let daily: Vec<Value> = window
        .iter()
        .map(|date| {
            let counters = by_day.get(date).copied().unwrap_or_default();
            window_totals.add(counters);
            let mut entry = counters.json();
            let object = entry
                .as_object_mut()
                .expect("counters serialize to an object");
            object.insert("date".to_string(), json!(date));
            object.insert("recorded".to_string(), json!(by_day.contains_key(date)));
            entry
        })
        .collect();

    let stored_peak = peak.map(|peak| {
        json!({
            "date": peak.date,
            "tokens": peak.tokens,
        })
    });
    json!({
        "days": window.len(),
        "from": window.first(),
        "to": window.last(),
        "daily": daily,
        "window": window_totals.json(),
        "allTime": all_time.json(),
        "peak": stored_peak,
        "failedPct": percent(window_totals.failures, window_totals.requests),
        "generatedAt": now_iso(),
    })
}

#[cfg(test)]
mod tests {
    use super::{status_json, usage_json, window_dates};
    use crate::tokens::Peak;
    use serde_json::{Value, json};

    fn account(email: &str, available: bool, days: &Value) -> Value {
        json!({
            "email": email,
            "available": available,
            "totalRequests": 10,
            "totalSuccesses": 9,
            "totalFailures": 1,
            "totalInputTokens": 100,
            "totalCacheReadInputTokens": 400,
            "totalCacheCreationInputTokens": 50,
            "totalOutputTokens": 25,
            "totalReasoningOutputTokens": 5,
            "days": days,
        })
    }

    fn day(date: &str, requests: i64, input: i64, read: i64, output: i64) -> Value {
        json!({
            "date": date,
            "requests": requests,
            "successes": requests - 1,
            "failures": 1,
            "inputTokens": input,
            "cacheReadInputTokens": read,
            "cacheCreationInputTokens": 10,
            "outputTokens": output,
            "reasoningOutputTokens": 2,
        })
    }

    #[test]
    fn status_counts_what_each_pool_can_serve_and_todays_requests() {
        let pools = vec![
            (
                "anthropic".to_string(),
                vec![
                    account(
                        "a@x.com",
                        true,
                        &json!([day("2026-10-28", 7, 100, 400, 25)]),
                    ),
                    account("b@x.com", false, &json!([day("2026-10-28", 3, 1, 1, 1)])),
                ],
            ),
            (
                "opencode".to_string(),
                vec![account(
                    "c@x.com",
                    true,
                    &json!([day("2026-10-27", 99, 1, 1, 1)]),
                )],
            ),
        ];
        let status = status_json(&pools, "0.112.0", 540, "0.0.0.0:8317", "2026-10-28");
        assert_eq!(status["version"], "0.112.0");
        assert_eq!(status["uptimeSeconds"], 540);
        assert_eq!(status["address"], "0.0.0.0:8317");
        assert_eq!(status["accounts"]["available"], 2);
        assert_eq!(status["accounts"]["total"], 3);
        // Only today's buckets count, whichever pool holds them.
        assert_eq!(status["requestsToday"], 10);
        assert_eq!(status["providers"]["anthropic"]["available"], 1);
        assert_eq!(status["providers"]["anthropic"]["total"], 2);
        assert_eq!(status["providers"]["opencode"]["available"], 1);
        assert!(status["generatedAt"].is_string());
    }

    #[test]
    fn usage_windows_days_oldest_first_and_keeps_idle_days() {
        let snapshots = vec![
            account(
                "a@x.com",
                true,
                &json!([
                    day("2026-10-28", 7, 100, 400, 25),
                    day("2026-10-27", 3, 50, 100, 10),
                ]),
            ),
            account("b@x.com", false, &json!([day("2026-10-28", 5, 10, 20, 5)])),
        ];
        let usage = usage_json(&snapshots, None, 30, "2026-10-28");
        assert_eq!(usage["days"], 30);
        assert_eq!(usage["from"], "2026-09-29");
        assert_eq!(usage["to"], "2026-10-28");
        let daily = usage["daily"].as_array().expect("daily");
        assert_eq!(daily.len(), 30);
        assert_eq!(daily[0]["date"], "2026-09-29");
        assert_eq!(daily[0]["recorded"], false);
        assert_eq!(daily[0]["requests"], 0);
        // The last two days fold every account that served them.
        assert_eq!(daily[29]["date"], "2026-10-28");
        assert_eq!(daily[29]["requests"], 12);
        assert_eq!(daily[28]["requests"], 3);
        // Prompt = input + cache read + cache write; carried = prompt + output.
        assert_eq!(daily[29]["inputTokens"], 110 + 420 + 20);
        assert_eq!(daily[29]["cachedTokens"], 420);
        assert_eq!(daily[29]["uncachedInputTokens"], 130);
        assert_eq!(daily[29]["totalTokens"], 550 + 30);
        // The window totals only what the window holds.
        assert_eq!(usage["window"]["requests"], 15);
        assert_eq!(usage["window"]["failures"], 3);
    }

    #[test]
    fn usage_reports_all_time_once_and_the_stored_peak() {
        let snapshots = vec![account("a@x.com", true, &json!([]))];
        let peak = Peak {
            date: "2026-10-13".to_string(),
            tokens: 1_575_864_399,
        };
        let usage = usage_json(&snapshots, Some(&peak), 30, "2026-10-28");
        // Every all-time figure is the sum of the accounts' totals, not a
        // recomputation from the retained days.
        assert_eq!(usage["allTime"]["requests"], 10);
        assert_eq!(usage["allTime"]["inputTokens"], 550);
        assert_eq!(usage["allTime"]["cachedTokens"], 400);
        assert_eq!(usage["allTime"]["cachedPct"], 72.7);
        assert_eq!(usage["allTime"]["uncachedInputTokens"], 150);
        assert_eq!(usage["allTime"]["outputTokens"], 25);
        assert_eq!(usage["allTime"]["reasoningTokens"], 5);
        assert_eq!(usage["allTime"]["reasoningPct"], 20.0);
        assert_eq!(usage["allTime"]["totalTokens"], 575);
        assert_eq!(usage["peak"]["date"], "2026-10-13");
        assert_eq!(usage["peak"]["tokens"], 1_575_864_399_i64);
        // A window with no buckets has no failure rate to state.
        assert!(usage["failedPct"].is_null());
        assert_eq!(usage["window"]["requests"], 0);
    }

    #[test]
    fn window_dates_end_on_today_and_count_back() {
        assert_eq!(
            window_dates("2026-10-28", 3),
            [
                "2026-10-26".to_string(),
                "2026-10-27".to_string(),
                "2026-10-28".to_string()
            ]
        );
        // Crossing a month boundary walks the calendar, not a 30-day constant.
        assert_eq!(window_dates("2026-03-01", 2), ["2026-02-28", "2026-03-01"]);
        assert!(window_dates("not a date", 3).is_empty());
    }
}
