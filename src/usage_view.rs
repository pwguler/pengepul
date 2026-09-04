//! The `status`/`accounts` view: the admin payload turned into pool
//! panels, account rows, footers and the relay total block, in both
//! styles. Pure over the payload and a `now` handed in by the caller.

use std::fmt::Write as _;

use serde_json::Value;

use crate::render::{
    AMBER, BOLD, DIM, GREEN, INNER_WIDTH, Output, RED, format_count, format_exact, pad, paint,
    panel_row, share_bar, top_rule,
};

/// `"on cooldown 4m12s"` while a cooldown lasts, `""` once it has cleared or
/// was never set (`cooldownUntil` is an absolute unix timestamp).
pub(crate) fn cooldown_label(now: f64, cooldown_until: f64) -> String {
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
pub(crate) fn print_pool_inner(payload: &Value, output: &mut Output, now: f64) {
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
        // An empty pool says nothing the relay total block doesn't; skip it.
        if accounts.is_empty() {
            continue;
        }
        if first && !output.is_empty() {
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
            "  tokens in {}  out {}  cache {}  reasoning {}",
            format_count(totals.input),
            format_count(totals.output),
            format_count(totals.cache_read + totals.cache_write),
            format_count(totals.reasoning)
        ));
    }
}

pub(crate) fn print_accounts(payload: &Value, output: &mut Output, now: f64) {
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
                "    requests {} ({} ok) in {} out {} cache {}",
                format_exact(i64_field(account, "totalRequests")),
                format_exact(i64_field(account, "totalSuccesses")),
                format_count(i64_field(account, "totalInputTokens")),
                format_count(i64_field(account, "totalOutputTokens")),
                format_count(
                    i64_field(account, "totalCacheReadInputTokens")
                        + i64_field(account, "totalCacheCreationInputTokens")
                ),
            );
            if reasoning != 0 {
                write!(detail, " reasoning {}", format_count(reasoning))
                    .expect("write to String cannot fail");
            }
            output.line(&detail);
        }
    }
}

pub(crate) fn providers(payload: &Value) -> Vec<(&str, &Value)> {
    let Some(providers) = payload.get("providers").and_then(Value::as_object) else {
        return Vec::new();
    };
    providers
        .iter()
        .map(|(provider_id, provider)| (provider_id.as_str(), provider))
        .collect()
}

#[derive(Default)]
pub(crate) struct PoolTotals {
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
    /// The pool's carried load: the same sum `account_tokens` computes per
    /// account, so footer totals match the share bars exactly.
    fn tokens(&self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

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

pub(crate) fn i64_field(account: &Value, field: &str) -> i64 {
    account.get(field).and_then(Value::as_i64).unwrap_or(0)
}

pub(crate) fn email(account: &Value) -> &str {
    account
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

/// The account row's column budget, summing to the 60 inner columns:
/// email 16 + state 17 (glyph, space, text) + ok 9 + bar 10 + pct 4,
/// single spaces between.
pub(crate) const EMAIL_WIDTH: usize = 16;
pub(crate) const STATE_SPAN_WIDTH: usize = 17; // "● " + "cooldown 23h59m"

pub(crate) const OK_WIDTH: usize = 9; // "999,999 ok" — bigger counts compact via format_count

/// The ok-count cell: exact while it fits the column, compact beyond —
/// a silently truncated number would be a lie, so large pools degrade to
/// the humanized form ("1.2M ok") the same way the footer does.
pub(crate) fn ok_cell(ok: i64) -> String {
    let exact = format!("{} ok", format_exact(ok));
    let text = if exact.chars().count() <= OK_WIDTH {
        exact
    } else {
        format!("{} ok", format_count(ok))
    };
    pad(&text, OK_WIDTH)
}

pub(crate) fn glyph(available: bool, on_cooldown: bool) -> String {
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
/// adds the per-account token line the `accounts` command shows. `now` is
/// handed in by the command layer: the renderer reads no clock (AC-7).
pub(crate) fn print_pool_rich(payload: &Value, output: &mut Output, with_detail: bool, now: f64) {
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
        // An empty pool says nothing the relay total block doesn't; skip it.
        if accounts.is_empty() {
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

        output.line(&top_rule(&format!(
            "pool: {provider_id} ─ {count} {suffix}, {available} available"
        )));

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

/// Sums across every pool of the relay: the aggregates behind the block.
#[derive(Default)]
pub(crate) struct RelayTotals {
    pools: usize,
    accounts: usize,
    requests: i64,
    tokens: i64,
}

impl RelayTotals {
    fn from_payload(payload: &Value) -> Self {
        let mut totals = Self::default();
        for (_provider_id, provider) in providers(payload) {
            // Pools with no loaded accounts are hidden from the status
            // output; the header must not count what it does not show.
            let has_accounts = provider
                .get("accounts")
                .and_then(Value::as_array)
                .is_some_and(|accounts| !accounts.is_empty());
            if has_accounts {
                totals.pools += 1;
            }
            totals.accounts += usize::try_from(
                provider
                    .get("account_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .max(0),
            )
            .unwrap_or(0);
            let accounts = provider
                .get("accounts")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            for account in accounts {
                totals.requests += i64_field(account, "totalRequests");
                totals.tokens += account_tokens(account);
            }
        }
        totals
    }
}

/// `relay total: P pools, A accounts` with singular forms where due.
pub(crate) fn relay_header(totals: &RelayTotals) -> String {
    format!(
        "relay total: {} {}, {} {}",
        totals.pools,
        if totals.pools == 1 { "pool" } else { "pools" },
        totals.accounts,
        if totals.accounts == 1 {
            "account"
        } else {
            "accounts"
        }
    )
}

/// The `Style::Plain` relay-total block: header, two totals, nothing else.
pub(crate) fn print_relay_total_plain(payload: &Value, output: &mut Output, connection: &[String]) {
    let totals = RelayTotals::from_payload(payload);
    output.line("");
    output.line(&relay_header(&totals));
    for line in connection {
        output.line(line);
    }
    output.line(&format!("total requests {}", format_exact(totals.requests)));
    output.line(&format!("total tokens {}", format_count(totals.tokens)));
}

/// The `Style::Rich` relay-total block: a 64-wide rule with the header
/// inside, then the same two totals.
pub(crate) fn print_relay_total_rich(payload: &Value, output: &mut Output, connection: &[String]) {
    let totals = RelayTotals::from_payload(payload);
    let header = relay_header(&totals);
    let fill = INNER_WIDTH.saturating_sub(header.chars().count() + 2);
    let mut rule = format!("──── {header} ");
    rule.extend(std::iter::repeat_n('─', fill));
    output.line(&rule);
    // Bare lines like the totals: the relay block is a rule, not a box.
    for line in connection {
        output.line(&paint(DIM, line));
    }
    output.line(&format!("total requests {}", format_exact(totals.requests)));
    output.line(&format!("total tokens {}", format_count(totals.tokens)));
}

/// One colored account row: email, glyph + state, ok count, share bar.
pub(crate) fn account_row(account: &Value, pool_total: i64, now: f64) -> String {
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
        // Glossary-safe: "unavailable" is on the Cooldown avoid-list; the
        // leftover case (no future cooldownUntil) reads "unresponsive".
        "unresponsive".to_string()
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
    let ok_text = ok_cell(ok);
    let (bar, share) = share_bar(account_tokens(account), pool_total);
    // AC-4: the percentage is right-aligned into a fixed 4-cell field
    // (" 45%", "100%", four spaces when the pool total is 0).
    let share_text = share.map_or_else(|| "    ".to_string(), |value| format!("{value:>3}%"));
    [
        pad(email(account), EMAIL_WIDTH),
        state_span,
        paint(BOLD, &ok_text),
        bar,
        paint(DIM, &share_text),
    ]
    .join(" ")
}

/// The dim per-account token lines shown under `accounts`: in/out/cache,
/// plus a reasoning line only when that total is non-zero.
pub(crate) fn account_detail_lines(account: &Value) -> Vec<String> {
    let mut lines = vec![paint(
        DIM,
        &format!(
            "in {}  out {}  cache {}",
            format_count(i64_field(account, "totalInputTokens")),
            format_count(i64_field(account, "totalOutputTokens")),
            format_count(
                i64_field(account, "totalCacheReadInputTokens")
                    + i64_field(account, "totalCacheCreationInputTokens")
            ),
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
pub(crate) fn footer_lines(totals: &PoolTotals) -> Vec<String> {
    let mut lines = vec![format!(
        "requests {}  ({} ok, {} failed)",
        paint(BOLD, &format_exact(totals.requests)),
        format_exact(totals.successes),
        format_exact(totals.failures)
    )];
    lines.push(format!(
        "tokens in {}  out {}  cache {}",
        paint(BOLD, &format_count(totals.input)),
        paint(BOLD, &format_count(totals.output)),
        format_count(totals.cache_read + totals.cache_write),
    ));
    // Reasoning totals get their own line only when non-zero; one row
    // for all five fields cannot fit the fixed width.
    if totals.reasoning != 0 {
        lines.push(format!(
            "reasoning {}",
            paint(BOLD, &format_count(totals.reasoning))
        ));
    }
    // The pool total stands alone, separated like the relay block's.
    lines.push(format!(
        "total {}",
        paint(BOLD, &format_count(totals.tokens()))
    ));
    lines
}

/// The account's carried load: every token that crossed it. This is what the
/// share bar divides; there is no quota in the domain to fill one with.
pub(crate) fn account_tokens(account: &Value) -> i64 {
    i64_field(account, "totalInputTokens")
        + i64_field(account, "totalOutputTokens")
        + i64_field(account, "totalCacheReadInputTokens")
        + i64_field(account, "totalCacheCreationInputTokens")
}

#[cfg(test)]
mod tests {
    use super::cooldown_label;
    use crate::render::strip_ansi;
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
    fn rich_renderer_panels_are_exactly_the_fixed_width() {
        use super::{Output, print_pool_rich};
        use serde_json::json;

        let payload = json!({
            "providers": {
                "anthropic": {
                    "account_count": 1,
                    "accounts": [{
                        "email": "a@x.com",
                        "available": true,
                        "failureCount": 0,
                        "totalRequests": 640,
                        "totalSuccesses": 638,
                        "totalInputTokens": 22_100_000,
                        "totalOutputTokens": 401_200,
                        "totalCacheCreationInputTokens": 6_000_000,
                        "totalCacheReadInputTokens": 155_000_000,
                        "totalReasoningOutputTokens": 64_000
                    }]
                }
            }
        });
        let mut output = Output::default();
        print_pool_rich(&payload, &mut output, false, 1_000_000.0);
        for line in output.stdout.lines() {
            assert_eq!(
                strip_ansi(line).chars().count(),
                64,
                "panel line off the fixed width: {line}"
            );
        }
    }

    #[test]
    fn rich_renderer_survives_an_oversized_provider_id() {
        use super::{Output, print_pool_rich};
        use serde_json::json;

        let long_id = "p".repeat(80);
        let payload = json!({
            "providers": {
                long_id: {
                    "account_count": 1,
                    "accounts": [{
                        "email": "a@x.com",
                        "available": true,
                        "failureCount": 0
                    }]
                }
            }
        });
        let mut output = Output::default();
        print_pool_rich(&payload, &mut output, false, 1_000_000.0);
        for line in output.stdout.lines() {
            assert_eq!(
                strip_ansi(line).chars().count(),
                64,
                "oversized header broke the box: {line}"
            );
        }
    }

    #[test]
    fn ok_cell_compacts_counts_that_cannot_fit_the_column() {
        use super::ok_cell;

        assert_eq!(ok_cell(638), "638 ok   ");
        assert_eq!(ok_cell(999_999), "999.9K ok");
        // A million successes cannot render exactly in 9 columns; the cell
        // degrades to the humanized form instead of truncating digits.
        assert_eq!(ok_cell(1_000_000), "1.0M ok  ");
    }
}
