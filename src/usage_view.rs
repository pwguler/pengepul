//! The `status`/`accounts` view: the admin payload turned into pool
//! panels, account rows, footers and the relay total block, in both
//! styles. Pure over the payload and a `now` handed in by the caller.

use std::fmt::Write as _;

use serde_json::Value;

use crate::render::{
    AMBER, ActionGlyph, BOLD, DIM, Fact, GREEN, INNER_WIDTH, Output, RED, fact_panel, fact_row,
    format_count, format_exact, label_column, pad, paint, panel_row, share_bar, sparkline,
    status_glyph, top_rule,
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
            let detail = format!(
                "    requests {} ({} ok) {}",
                format_exact(i64_field(account, "totalRequests")),
                format_exact(i64_field(account, "totalSuccesses")),
                token_line(
                    i64_field(account, "totalInputTokens"),
                    i64_field(account, "totalOutputTokens"),
                    i64_field(account, "totalCacheReadInputTokens"),
                    i64_field(account, "totalCacheCreationInputTokens"),
                    i64_field(account, "totalReasoningOutputTokens"),
                ),
            );
            output.line(&detail);
            // AC-7: the same per-model breakdown, plain.
            for row in model_rows(account) {
                output.line(&format!(
                    "    {} {} ok {}",
                    row.name,
                    format_exact(row.successes),
                    format_count(row.tokens())
                ));
                output.line(&format!("      {}", row.token_line()));
            }
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

/// One model's usage on one account, as the payload carries it.
pub(crate) struct ModelRow {
    name: String,
    successes: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
}

impl ModelRow {
    /// The model's carried load, on the same definition as every other total
    /// in the view: `input + output + cached + written`, reasoning excluded.
    fn tokens(&self) -> i64 {
        carried_tokens(self.input, self.output, self.cache_read, self.cache_write)
    }

    /// The model's token block, on the same rows as every other scope
    /// (ADR-0024).
    fn facts(&self) -> Vec<Fact> {
        token_block_facts(
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.reasoning,
            DIM,
        )
    }

    /// The model's block as one line, for the plain contract.
    fn token_line(&self) -> String {
        token_line(
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.reasoning,
        )
    }

    /// `claude-fable-5-1   612 ok     9.2K` — `width` is the name column
    /// the caller fitted to the rows it is about to print, so short names
    /// keep their numbers close and one long name widens every row.
    fn headline(&self, width: usize) -> String {
        format!(
            "{} {} {:>tokens$}",
            pad(&self.name, width),
            ok_cell(self.successes),
            format_count(self.tokens()),
            tokens = POOL_TOKENS_WIDTH
        )
    }
}
/// The widest the model name cell may grow. The indented account row
/// spends its 60 inner columns as 2 indent + name + 1 + ok 9 + 1 +
/// tokens 9, so 38 is what is left; the widest id in the shipped catalog
/// is 28. `name_column` fits the cell to the rows actually present and
/// clamps here, so a longer id clips rather than breaking the box — the
/// plain branch prints names un-clipped.
const MODEL_NAME_WIDTH: usize = 38;

/// The name column for one account's rows: the longest name present plus
/// a space of air before the ok cell, capped at `MODEL_NAME_WIDTH`.
fn name_column(rows: &[ModelRow]) -> usize {
    rows.iter()
        .map(|row| row.name.chars().count() + 1)
        .max()
        .unwrap_or(0)
        .min(MODEL_NAME_WIDTH)
}

/// The account's `models` array, heaviest first, ties broken by name so the
/// order never wobbles between calls.
pub(crate) fn model_rows(account: &Value) -> Vec<ModelRow> {
    let mut rows: Vec<ModelRow> =
        account
            .get("models")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |models| {
                models
                    .iter()
                    .map(|model| ModelRow {
                        name: model
                            .get("model")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        successes: i64_field(model, "successes"),
                        input: i64_field(model, "inputTokens"),
                        output: i64_field(model, "outputTokens"),
                        cache_read: i64_field(model, "cacheReadInputTokens"),
                        cache_write: i64_field(model, "cacheCreationInputTokens"),
                        reasoning: i64_field(model, "reasoningOutputTokens"),
                    })
                    .collect()
            });
    rows.sort_by(|left, right| {
        right
            .tokens()
            .cmp(&left.tokens())
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

/// The rich pool view behind `accounts`: one panel per provider with rows,
/// per-account token lines and a footer rollup. Pure over the payload and
/// the clock value handed to it: the renderer reads no clock.
pub(crate) fn print_pool_rich(payload: &Value, output: &mut Output, now: f64) {
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
            "pool {provider_id} ─ {count} {suffix}, {available} available"
        )));

        let pool_total: i64 = accounts.iter().map(account_tokens).sum();
        let mut totals = PoolTotals::default();
        for account in accounts {
            totals.add(account);
        }
        // One name column for the whole panel: the same model's ok cell
        // must land in the same place in every row of a box.
        let width = name_column(
            &accounts
                .iter()
                .flat_map(model_rows)
                .collect::<Vec<ModelRow>>(),
        );
        // One label column for every fact in the panel -- the per-account
        // token rows and the footer rollup align down the whole box.
        let mut panel_facts: Vec<Fact> = accounts.iter().flat_map(account_detail_facts).collect();
        panel_facts.extend(
            accounts
                .iter()
                .flat_map(model_rows)
                .flat_map(|row| row.facts()),
        );
        panel_facts.extend(footer_facts(&totals));
        let column = label_column(&panel_facts);
        for account in accounts {
            output.line(&panel_row(&account_row(account, pool_total, now)));
            for fact in account_detail_facts(account) {
                output.line(&panel_row(&format!("  {}", fact_row(&fact, column))));
            }
            // AC-5: the models this account served, heaviest first.
            for row in model_rows(account) {
                output.line(&panel_row(&format!("  {}", row.headline(width))));
                for fact in row.facts() {
                    output.line(&panel_row(&format!("    {}", fact_row(&fact, column))));
                }
            }
        }

        output.line(&format!("├{}┤", "─".repeat(INNER_WIDTH + 2)));
        for fact in footer_facts(&totals) {
            output.line(&panel_row(&fact_row(&fact, column)));
        }
        output.line(&format!("└{}┘", "─".repeat(INNER_WIDTH + 2)));
    }
}

/// Sums across every pool of the relay: the aggregates behind the block.
#[derive(Default)]
pub(crate) struct RelayTotals {
    pools: usize,
    accounts: usize,
    /// Every account of every pool, summed. The relay's request and token
    /// figures both read from here: one accumulation, one source of truth.
    totals: PoolTotals,
    lines: Vec<PoolLine>,
}

/// One pool's summary row in the relay block: what `status` shows now that
/// the per-pool panels are gone.
pub(crate) struct PoolLine {
    name: String,
    accounts: i64,
    requests: i64,
    tokens: i64,
}

/// The pool line's column budget inside the 60 inner columns: name 18 +
/// accounts 12 + requests 11 + tokens 9, single spaces between (53), the
/// rest is slack. The name cell is a *minimum*, not a clamp: plain output
/// is the surface a script parses and the one that can be trusted for a
/// full provider key (usage-by-model AC-7), so a long name widens its row
/// rather than losing characters to an ellipsis a parser would read as
/// part of the id. Short names still align down the block. The rich
/// branch carries the name as a fact label, where it does clip.
const POOL_NAME_WIDTH: usize = 18;
const POOL_ACCOUNTS_WIDTH: usize = 12;
const POOL_REQUESTS_WIDTH: usize = 11;
const POOL_TOKENS_WIDTH: usize = 9;

impl PoolLine {
    /// `anthropic     3 accounts    1,204 req    143.4M` — the plain
    /// branch's row, where the pool name leads the line.
    fn render(&self) -> String {
        format!(
            "{:<width$} {}",
            self.name,
            self.render_value(),
            width = POOL_NAME_WIDTH
        )
    }

    /// The same row without its name: rich carries the name as the fact
    /// label, so the value starts at the accounts count.
    fn render_value(&self) -> String {
        let accounts = format!(
            "{} {}",
            self.accounts,
            if self.accounts == 1 {
                "account"
            } else {
                "accounts"
            }
        );
        let requests = format!("{} req", format_exact(self.requests));
        let requests = if requests.chars().count() <= POOL_REQUESTS_WIDTH {
            requests
        } else {
            format!("{} req", format_count(self.requests))
        };
        format!(
            "{} {} {:>width$}",
            pad(&accounts, POOL_ACCOUNTS_WIDTH),
            pad(&requests, POOL_REQUESTS_WIDTH),
            format_count(self.tokens),
            width = POOL_TOKENS_WIDTH
        )
    }
}

impl RelayTotals {
    fn from_payload(payload: &Value) -> Self {
        let mut totals = Self::default();
        for (provider_id, provider) in providers(payload) {
            let accounts = provider
                .get("accounts")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            // Pools with no loaded accounts are hidden from the status
            // output; the header must not count what it does not show.
            if !accounts.is_empty() {
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
            let mut pool = PoolLine {
                name: provider_id.to_string(),
                accounts: provider
                    .get("account_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                requests: 0,
                tokens: 0,
            };
            for account in accounts {
                let requests = i64_field(account, "totalRequests");
                let tokens = account_tokens(account);
                totals.totals.add(account);
                pool.requests += requests;
                pool.tokens += tokens;
            }
            if !accounts.is_empty() {
                totals.lines.push(pool);
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

/// The same facts as the box panel's header, where the separator is the
/// panel's own rule rather than a colon.
fn relay_header_rich(totals: &RelayTotals) -> String {
    relay_header(totals).replacen(": ", " ─ ", 1)
}

/// Where the relay is and whether it answered: the facts `status` shows
/// above its numbers. Kept structured rather than pre-formatted so plain
/// can join `url` and `server` on one line (its bytes are load-bearing
/// for scripts) while rich gives each its own labelled row.
pub(crate) struct Connection {
    pub(crate) config: String,
    pub(crate) url: String,
    pub(crate) server: String,
    /// The build of the binary making the call. Local by necessity — a client cannot learn
    /// the serving process's version — so `stale` is what says when the two differ.
    pub(crate) version: String,
    /// How long the service has been up, when the service manager could say. `None` is a
    /// fact too: the row is then absent rather than guessed.
    pub(crate) uptime: Option<String>,
    /// The binary making the call is newer than the service that is running: an install
    /// that has not been restarted into, or simply a newer local build.
    pub(crate) stale: bool,
}

/// The `Style::Plain` relay block: header, connection, one line per pool,
/// then the relay-wide aggregate. This is the whole of `status` now.
pub(crate) fn print_relay_total_plain(
    payload: &Value,
    output: &mut Output,
    connection: &Connection,
) {
    let totals = RelayTotals::from_payload(payload);
    output.line(&relay_header(&totals));
    output.line(&format!("config {}", connection.config));
    output.line(&format!(
        "url {} \u{2014} server {}",
        connection.url, connection.server
    ));
    output.line(&version_line(connection));
    if let Some(uptime) = &connection.uptime {
        output.line(&format!("uptime {uptime}"));
    }
    for pool in &totals.lines {
        output.line(pool.render().trim_end());
    }
    for line in aggregate_lines(&totals) {
        output.line(&line);
    }
}

/// The `Style::Rich` relay block: the same content as one 64-column box
/// of labelled facts (`consistent-panels`).
pub(crate) fn print_relay_total_rich(
    payload: &Value,
    output: &mut Output,
    connection: &Connection,
) {
    let totals = RelayTotals::from_payload(payload);
    // The glyph marks a state, never a plain fact: `server` earns one,
    // `config` and `url` do not.
    let health = if connection.server == "ok" {
        ActionGlyph::Ok
    } else {
        ActionGlyph::Attention
    };
    let mut facts = vec![
        Fact::new("config", &paint(DIM, &connection.config)),
        Fact::new("url", &paint(DIM, &connection.url)),
        Fact::new(
            "server",
            &format!("{} {}", status_glyph(health), connection.server),
        ),
        Fact::new("version", &version_value(connection)),
    ];
    if let Some(uptime) = &connection.uptime {
        facts.push(Fact::new("uptime", uptime));
    }
    for line in &totals.lines {
        facts.push(Fact::new(&line.name, &line.render_value()));
    }
    facts.extend(aggregate_facts(&totals));
    for line in fact_panel(&relay_header_rich(&totals), &facts) {
        output.line(&line);
    }
}

/// The plain `version` line: the build, and that it is not the one serving when the binary
/// making the call is newer than the service. The note names the condition rather than the
/// action, because the mark fires for a freshly built binary too, and "restart to apply"
/// would be false advice there.
fn version_line(connection: &Connection) -> String {
    if connection.stale {
        format!("version {}  (not the running build)", connection.version)
    } else {
        format!("version {}", connection.version)
    }
}

/// The rich `version` row: the same two facts, with the attention glyph carrying the state
/// the way `server` carries its own.
fn version_value(connection: &Connection) -> String {
    if connection.stale {
        format!(
            "{} {}  not the running build",
            status_glyph(ActionGlyph::Attention),
            connection.version
        )
    } else {
        connection.version.clone()
    }
}

/// The relay-wide rollup, rich: the requests breakdown, then the token block in the
/// vocabulary ADR-0024 settled. One function because two verbs print it — `status` inside
/// its connection panel, `usage` under the trend — and the whole point of the second copy is
/// that the figures cannot drift from the first.
fn aggregate_facts(totals: &RelayTotals) -> Vec<Fact> {
    let pool = &totals.totals;
    let mut facts = vec![Fact::new(
        "requests",
        &format!(
            "{}  ({} ok, {} failed)",
            paint(BOLD, &format_exact(pool.requests)),
            format_exact(pool.successes),
            format_exact(pool.failures)
        ),
    )];
    facts.extend(token_block_facts(
        pool.input,
        pool.output,
        pool.cache_read,
        pool.cache_write,
        pool.reasoning,
        BOLD,
    ));
    facts
}

/// `usage`'s relay total, plain: the header and the aggregate lines, with no blank line
/// before them — the views stack panels without a gap, and the header is what marks the
/// change of scope. The trend above covers 30 days; these figures do not, which is why it
/// keeps the wording `status` uses.
pub(crate) fn print_usage_total_plain(payload: &Value, output: &mut Output) {
    let totals = RelayTotals::from_payload(payload);
    output.line(&relay_header(&totals));
    for line in aggregate_lines(&totals) {
        output.line(&line);
    }
}

/// `usage`'s relay total, rich: the same content as one panel below the trend panel, with
/// the aggregate rows only — no `config`, no `url`, no per-pool line, because where-and-which
/// facts are `status`'s and per-pool detail is `accounts`'.
pub(crate) fn print_usage_total_rich(payload: &Value, output: &mut Output) {
    let totals = RelayTotals::from_payload(payload);
    for line in fact_panel(&relay_header_rich(&totals), &aggregate_facts(&totals)) {
        output.line(&line);
    }
}

/// The relay-wide rollup, plain: requests, tokens, reasoning when non-zero,
/// and the carried-load total.
fn aggregate_lines(totals: &RelayTotals) -> Vec<String> {
    let pool = &totals.totals;
    vec![
        format!(
            "requests {}  ({} ok, {} failed)",
            format_exact(pool.requests),
            format_exact(pool.successes),
            format_exact(pool.failures)
        ),
        token_line(
            pool.input,
            pool.output,
            pool.cache_read,
            pool.cache_write,
            pool.reasoning,
        ),
    ]
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

/// `part` as a rounded percentage of `whole`, or `None` when there is no whole to divide by.
///
/// `i128` so a counter near `i64::MAX` cannot overflow the multiply, and `None` rather than
/// 0 so a caller can tell "no fraction worth printing" from "nothing to divide by".
fn share(part: i64, whole: i64) -> Option<i64> {
    if whole <= 0 {
        return None;
    }
    let percent = (i128::from(part) * 100 + i128::from(whole) / 2) / i128::from(whole);
    i64::try_from(percent).ok()
}

/// `1.3B (94.8%)` — the cache read and its share of the prompt the upstream saw.
///
/// This is the hit rate the counter model exists to make comparable. `read` is the token a
/// cache served and `rest` is every prompt token it did not — the never-cached input plus the
/// cache write — so the ratio means the same thing on every Provider (ADR-0023). A write is
/// not a hit, and counting one in the numerator is what printed 100% for a pool whose read
/// share was 94.8% (ADR-0024).
///
/// Bare `(94.8%)` rather than `(94.8% of prompt)` because the fact row has ~49 columns for its
/// value and the longer label clipped the row with an ellipsis.
fn cache_with_share(read: i64, rest: i64) -> String {
    let count = format_count(read);
    match share(read, rest.saturating_add(read)) {
        Some(percent) => format!("{count} ({percent}%)"),
        None => count,
    }
}

/// The token block: the rows every scope prints, in order — the prompt the upstream saw, how
/// much of it a cache served, how much it did not, and what the model generated — with
/// `reasoning` last, and only when there was any (ADR-0024).
///
/// `input` is `cached + uncached` by construction and `cached` carries the read share of it, so
/// a reader can check the block without knowing which dialect the upstream spoke.
fn token_block_facts(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    colour: &str,
) -> Vec<Fact> {
    let uncached = input + cache_write;
    let mut facts = vec![
        Fact::new(
            "input",
            &paint(colour, &format_count(input + cache_read + cache_write)),
        ),
        Fact::new(
            "cached",
            &paint(colour, &cache_with_share(cache_read, uncached)),
        ),
        Fact::new("uncached", &paint(colour, &format_count(uncached))),
        Fact::new("output", &paint(colour, &format_count(output))),
    ];
    if reasoning != 0 {
        facts.push(reasoning_fact(reasoning, output, colour));
    }
    facts
}

/// The block as one line, for the plain contract: the panel's words in the panel's order
/// (ADR-0024).
fn token_line(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
) -> String {
    let uncached = input + cache_write;
    let mut line = format!(
        "input {} cached {} uncached {} output {}",
        format_count(input + cache_read + cache_write),
        cache_with_share(cache_read, uncached),
        format_count(uncached),
        format_count(output),
    );
    if reasoning != 0 {
        write!(line, " reasoning {}", reasoning_text(reasoning, output))
            .expect("write to String cannot fail");
    }
    line
}

/// The reasoning figure and, when the two counters agree, the share of `output` it is part of.
///
/// Reasoning is a breakdown of `output`, not a term beside it. Every vendor's
/// `output_tokens`/`completion_tokens` already includes it, and the two that report it apart
/// are folded in before the counters see them (ADR-0023), so a bare figure under a token row
/// reads as a fifth term that does not add up. The share is also the operator's only view of
/// how much of the bill was thinking, which on some Providers is nearly all of it.
fn reasoning_text(reasoning: i64, output: i64) -> String {
    let count = format_count(reasoning);
    match share(reasoning, output) {
        // Reasoning above output means the two counters cannot both be right, and
        // a percentage would dress that up as a share of something. Two things
        // produce it, and neither is this function's to guess at: counters recorded before
        // the fold existed (the grok pool's history holds `out 93` beside `reasoning 1,236`),
        // and an upstream reporting reasoning outside the output count where no fold applied
        // — the Gemini OpenAI-compatible shape the research file marks unverified. Printing
        // both figures says which without inventing a reason.
        Some(_) if reasoning > output => format!("{count} (out {})", format_count(output)),
        Some(percent) => format!("{count} ({percent}%)"),
        None => count,
    }
}

fn reasoning_fact(reasoning: i64, output: i64, colour: &str) -> Fact {
    Fact::new(
        "reasoning",
        &paint(colour, &reasoning_text(reasoning, output)),
    )
}

/// The account's token block, on the same rows as every other scope (ADR-0024).
pub(crate) fn account_detail_facts(account: &Value) -> Vec<Fact> {
    token_block_facts(
        i64_field(account, "totalInputTokens"),
        i64_field(account, "totalOutputTokens"),
        i64_field(account, "totalCacheReadInputTokens"),
        i64_field(account, "totalCacheCreationInputTokens"),
        i64_field(account, "totalReasoningOutputTokens"),
        DIM,
    )
}

/// Footer rollup facts: requests and the pool's token block. The pool's carried load is not a
/// row — it is `input + output`, two rows the block already prints (ADR-0024).
pub(crate) fn footer_facts(totals: &PoolTotals) -> Vec<Fact> {
    let mut facts = vec![Fact::new(
        "requests",
        &format!(
            "{}  ({} ok, {} failed)",
            paint(BOLD, &format_exact(totals.requests)),
            format_exact(totals.successes),
            format_exact(totals.failures)
        ),
    )];
    facts.extend(token_block_facts(
        totals.input,
        totals.output,
        totals.cache_read,
        totals.cache_write,
        totals.reasoning,
        BOLD,
    ));
    facts
}

/// The account's carried load: every token that crossed it. This is what the
/// share bar divides; there is no quota in the domain to fill one with.
pub(crate) fn account_tokens(account: &Value) -> i64 {
    carried_tokens(
        i64_field(account, "totalInputTokens"),
        i64_field(account, "totalOutputTokens"),
        i64_field(account, "totalCacheReadInputTokens"),
        i64_field(account, "totalCacheCreationInputTokens"),
    )
}

/// What "carried load" means, in one place: input, output and both cache
/// directions. Reasoning is excluded — it is already inside output. Every
/// figure derived from it resolves through here — the share bars, the model
/// headlines and `usage`'s all-time row — so they cannot disagree about what
/// they are summing (AC-11). No panel prints it as a row: it is `input +
/// output`, two rows the token block carries (ADR-0024).
pub(crate) fn carried_tokens(input: i64, output: i64, cache_read: i64, cache_write: i64) -> i64 {
    input + output + cache_read + cache_write
}

/// How many local days the trend shows. The file keeps more (the store's
/// retention); the panel shows this many (usage-trend AC-5).
const TREND_DAYS: usize = 30;

/// One day of relay-wide traffic: every account of every pool, summed.
pub(crate) struct TrendDay {
    date: String,
    tokens: i64,
    /// Whether the relay held a bucket for this day. Carried rather than
    /// re-derived from `tokens > 0`: a day whose every request failed is
    /// history with no tokens, and two rules for one word drift.
    recorded: bool,
}

/// The payload's per-account `days` arrays folded into one relay-wide
/// series, oldest first, covering the `TREND_DAYS` window that ends on
/// `today`. Days with no traffic are present with zero, so the sparkline
/// has one column per calendar day rather than one per recorded day.
/// Pure over the payload and the date handed in (AC-9, AC-10).
pub(crate) fn trend_days(payload: &Value, today: &str) -> Vec<TrendDay> {
    let mut totals: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for (_provider_id, provider) in providers(payload) {
        let accounts = provider
            .get("accounts")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for account in accounts {
            let days = account
                .get("days")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            for day in days {
                let Some(date) = day.get("date").and_then(Value::as_str) else {
                    continue;
                };
                let tokens = i64_field(day, "inputTokens")
                    + i64_field(day, "outputTokens")
                    + i64_field(day, "cacheReadInputTokens")
                    + i64_field(day, "cacheCreationInputTokens");
                // The entry exists because the day was recorded; its value
                // may be zero.
                *totals.entry(date.to_string()).or_default() += tokens;
            }
        }
    }
    let window: Vec<TrendDay> = window_dates(today, TREND_DAYS)
        .into_iter()
        .map(|date| {
            let tokens = totals.get(&date).copied().unwrap_or(0);
            let recorded = totals.contains_key(&date);
            TrendDay {
                date,
                tokens,
                recorded,
            }
        })
        .collect();
    // Emptiness is judged over the *window*, not over every bucket the
    // file holds: a relay whose only traffic predates the window would
    // otherwise render 30 flat bars, the shape AC-8 exists to prevent.
    // A day is history because it was *recorded*, not because it spent
    // tokens — a day of failures is history, and plain prints it, so rich
    // must not call it empty.
    if !window.iter().any(|day| day.recorded) {
        return Vec::new();
    }
    window
}

/// The `count` calendar dates ending at `today`, oldest first. Date math
/// on the `YYYY-MM-DD` string so the renderer stays clock-free.
fn window_dates(today: &str, count: usize) -> Vec<String> {
    let Ok(end) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
        return Vec::new();
    };
    let span = i64::try_from(count).unwrap_or(0);
    (0..span)
        .rev()
        .map(|back| {
            (end - chrono::Duration::days(back))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect()
}

/// The `Style::Rich` trend: one panel, four rows (AC-5).
pub(crate) fn print_trend_rich(payload: &Value, output: &mut Output, today: &str) {
    let days = trend_days(payload, today);
    if days.is_empty() {
        // AC-8: thirty flat bars would read as thirty idle days.
        for line in fact_panel(
            "usage ─ last 30 days",
            &[Fact::new("tokens", &paint(DIM, "no usage recorded yet"))],
        ) {
            output.line(&line);
        }
        return;
    }
    let values: Vec<i64> = days.iter().map(|day| day.tokens).collect();
    let peak = days
        .iter()
        .max_by_key(|day| day.tokens)
        .expect("non-empty window");
    let total: i64 = values.iter().sum();
    // The all-time figure comes from the same sum `status` prints, over the
    // same payload, so the two verbs cannot drift and the window is
    // visibly a subset rather than a competing total.
    let all_time: i64 = providers(payload)
        .into_iter()
        .flat_map(|(_provider_id, provider)| {
            provider
                .get("accounts")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
        })
        .map(account_tokens)
        .sum();
    // The window is a subset of all time by construction. A payload whose
    // cumulative counters are missing or lag its buckets would otherwise
    // print a total smaller than the subset inside it.
    let all_time = all_time.max(total);
    let facts = vec![
        Fact::new("tokens", &sparkline(&values)),
        // Figures, not sentences: the peak row carries its count and its day, and the
        // window row carries its total. The panel header already says the window is the
        // last 30 days, so words explaining either would be the only prose in the box.
        Fact::new(
            "peak",
            &format!("{}  {}", paint(BOLD, &format_count(peak.tokens)), peak.date),
        ),
        Fact::new("window", &paint(BOLD, &format_count(total))),
        Fact::new("all time", &paint(BOLD, &format_count(all_time))),
    ];
    for line in fact_panel("usage ─ last 30 days", &facts) {
        output.line(&line);
    }
}

/// The `Style::Plain` trend: one parseable row per recorded day, oldest
/// first, no block characters (AC-7).
pub(crate) fn print_trend_plain(payload: &Value, output: &mut Output, today: &str) {
    let mut rows: std::collections::BTreeMap<String, (i64, i64, i64, i64, i64)> =
        std::collections::BTreeMap::new();
    for (_provider_id, provider) in providers(payload) {
        let accounts = provider
            .get("accounts")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for account in accounts {
            let days = account
                .get("days")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            for day in days {
                let Some(date) = day.get("date").and_then(Value::as_str) else {
                    continue;
                };
                let row = rows.entry(date.to_string()).or_default();
                row.0 += i64_field(day, "requests");
                row.1 += i64_field(day, "inputTokens");
                row.2 += i64_field(day, "outputTokens");
                row.3 += i64_field(day, "cacheReadInputTokens")
                    + i64_field(day, "cacheCreationInputTokens");
                row.4 += i64_field(day, "reasoningOutputTokens");
            }
        }
    }
    let window: std::collections::BTreeSet<String> =
        window_dates(today, TREND_DAYS).into_iter().collect();
    for (date, row) in rows.iter().filter(|(date, _)| window.contains(*date)) {
        output.line(&format!(
            "{date} {} {} {} {} {}",
            row.0, row.1, row.2, row.3, row.4
        ));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::cooldown_label;
    use crate::render::strip_ansi;
    #[test]
    fn the_relay_block_prints_no_cache_write() {
        // A 1h write bills at 2x base input against 1.25x for 5m (ADR-0018), and the CLI
        // does not print it: the figure stays in the admin payload, where a reader who wants
        // what the write cost can still find it (ADR-0024).
        let payload = json!({"providers": {"anthropic": {
            "account_count": 1,
            "accounts": [{
                "email": "k@example.com",
                "totalRequests": 2,
                "totalSuccesses": 2,
                "totalInputTokens": 100,
                "totalCacheCreationInputTokens": 10416,
                "totalCacheCreation1hInputTokens": 7216,
                "totalCacheReadInputTokens": 0
            }]
        }}});

        let lines = super::aggregate_lines(&super::RelayTotals::from_payload(&payload));
        for line in &lines {
            assert!(!line.contains("1h write"), "{lines:?}");
            assert!(!line.contains("7216"), "{lines:?}");
            assert!(!line.contains("7.2K"), "{lines:?}");
        }
        // The write is still inside the prompt the block prints: 100 uncached + 10,416
        // written is 10.5K, and that is the only figure the CLI gives it.
        assert!(
            lines.iter().any(|line| line.contains("input 10.5K")),
            "{lines:?}"
        );
    }

    #[test]
    fn shares_and_facts_state_the_ratios_the_counters_imply() {
        use super::{Fact, cache_with_share, reasoning_fact};
        use crate::render::DIM;

        let value = |fact: &Fact| strip_ansi(&fact.value);

        // The grok pool's own history is the disagreement: counters written before the fold
        // existed hold `out 93` beside `reasoning 1,236`, and `of out (1329%)` would be a
        // percentage of nothing. The two figures are printed instead.
        let fact = reasoning_fact(64_000, 401_200, DIM);
        assert_eq!(value(&fact), "64.0K (16%)");
        let fact = reasoning_fact(1_236, 93, DIM);
        assert_eq!(value(&fact), "1.2K (out 93)");
        let fact = reasoning_fact(1_236, 1_329, DIM);
        assert_eq!(value(&fact), "1.2K (93%)");
        // No output to be a share of: the count stands alone rather than dividing by zero.
        let fact = reasoning_fact(7, 0, DIM);
        assert_eq!(value(&fact), "7");

        // The cache figure carries its share of the prompt the upstream saw, which is the hit
        // rate: an Anthropic pool reading 1.3B against 3.8M uncached is 100% of its prompt.
        assert_eq!(cache_with_share(1_266_782_110, 3_822_778), "1.2B (100%)");
        assert_eq!(cache_with_share(8_500, 300), "8.5K (97%)");
        // Nothing cached and nothing uncached: no ratio to state, so just the count.
        assert_eq!(cache_with_share(0, 0), "0");
    }

    /// AC-2: the share is the read's, and a cache write is not a hit.
    #[test]
    fn the_cache_share_is_the_read_share_of_the_prompt() {
        use super::{DIM, Fact, token_block_facts};

        let value = |fact: &Fact| strip_ansi(&fact.value);
        // 155.0M read of a 183.1M prompt, 6.0M of it written. The pre-change code put the
        // write in the numerator and printed 161.0M (88%).
        let facts = token_block_facts(22_100_000, 401_200, 155_000_000, 6_000_000, 0, DIM);
        assert_eq!(value(&facts[1]), "155.0M (85%)");
        assert_ne!(value(&facts[1]), "161.0M (88%)");
    }

    /// AC-4: the block's rows partition the prompt, so a reader never has to add two of them
    /// up to know the third.
    #[test]
    fn the_token_block_partitions_the_prompt() {
        use super::{DIM, Fact, token_block_facts};

        let facts = token_block_facts(300, 400, 500, 100, 0, DIM);
        let value = |fact: &Fact| strip_ansi(&fact.value);
        let count = |index: usize| {
            value(&facts[index])
                .split_whitespace()
                .next()
                .expect("a count")
                .parse::<i64>()
                .expect("a number")
        };
        // 300 never cached + 500 read + 100 written.
        assert_eq!(count(0), 900);
        assert_eq!(count(1), 500);
        // Neither a read nor a write served the 300: the write folds in beside it.
        assert_eq!(count(2), 400);
        assert_eq!(count(3), 400);
        assert_eq!(count(0), count(1) + count(2));
    }

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
                        "totalReasoningOutputTokens": 64_000,
                        "models": [{
                            "model": "claude-fable-5-1",
                            "successes": 612,
                            "inputTokens": 300,
                            "outputTokens": 400,
                            "cacheCreationInputTokens": 0,
                            "cacheReadInputTokens": 8_000,
                            "reasoningOutputTokens": 42
                        }]
                    }]
                }
            }
        });
        let mut output = Output::default();
        print_pool_rich(&payload, &mut output, 1_000_000.0);
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
        print_pool_rich(&payload, &mut output, 1_000_000.0);
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
