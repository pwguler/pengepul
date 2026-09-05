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
            // AC-7: the same per-model breakdown, plain.
            for row in model_rows(account) {
                output.line(&format!(
                    "    {} {} ok {}",
                    row.name,
                    format_exact(row.successes),
                    format_count(row.tokens())
                ));
                output.line(&format!(
                    "      in {} out {} cache {}{}",
                    format_count(row.input),
                    format_count(row.output),
                    format_count(row.cache),
                    if row.reasoning == 0 {
                        String::new()
                    } else {
                        format!(" reasoning {}", format_count(row.reasoning))
                    }
                ));
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
    /// The pool's carried load, through the one function that defines it.
    /// `account_tokens` reads the same four fields off an account; both
    /// call `carried_tokens`, so a change to what "carried" means moves
    /// every view at once rather than one of them silently.
    fn tokens(&self) -> i64 {
        carried_tokens(self.input, self.output, self.cache_read, self.cache_write)
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

/// One model's usage on one account, as the payload carries it.
pub(crate) struct ModelRow {
    name: String,
    successes: i64,
    input: i64,
    output: i64,
    cache: i64,
    reasoning: i64,
}

impl ModelRow {
    /// The model's carried load, on the same definition as every other
    /// total in the view: in + out + cache, reasoning excluded.
    fn tokens(&self) -> i64 {
        self.input + self.output + self.cache
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

    /// `in 300  out 400  cache 8.5K` — plus reasoning when non-zero.
    fn detail(&self) -> String {
        let mut detail = format!(
            "in {}  out {}  cache {}",
            format_count(self.input),
            format_count(self.output),
            format_count(self.cache)
        );
        if self.reasoning != 0 {
            write!(detail, "  reasoning {}", format_count(self.reasoning))
                .expect("write to String cannot fail");
        }
        detail
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
                        cache: i64_field(model, "cacheReadInputTokens")
                            + i64_field(model, "cacheCreationInputTokens"),
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
        panel_facts.extend(footer_facts(&totals));
        let column = label_column(&panel_facts);
        for account in accounts {
            output.line(&panel_row(&account_row(account, pool_total, now)));
            for fact in account_detail_facts(account) {
                output.line(&panel_row(&fact_row(&fact, column)));
            }
            // AC-5: the models this account served, heaviest first.
            for row in model_rows(account) {
                output.line(&panel_row(&format!("  {}", row.headline(width))));
                output.line(&panel_row(&paint(DIM, &format!("    {}", row.detail()))));
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
    let pool = &totals.totals;
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
    ];
    for line in &totals.lines {
        facts.push(Fact::new(&line.name, &line.render_value()));
    }
    facts.push(Fact::new(
        "requests",
        &format!(
            "{}  ({} ok, {} failed)",
            paint(BOLD, &format_exact(pool.requests)),
            format_exact(pool.successes),
            format_exact(pool.failures)
        ),
    ));
    facts.push(Fact::new(
        "tokens",
        &format!(
            "in {}  out {}  cache {}",
            paint(BOLD, &format_count(pool.input)),
            paint(BOLD, &format_count(pool.output)),
            format_count(pool.cache_read + pool.cache_write)
        ),
    ));
    if pool.reasoning != 0 {
        facts.push(Fact::new(
            "reasoning",
            &paint(BOLD, &format_count(pool.reasoning)),
        ));
    }
    facts.push(Fact::new(
        "total",
        &paint(BOLD, &format_count(totals.totals.tokens())),
    ));
    for line in fact_panel(&relay_header_rich(&totals), &facts) {
        output.line(&line);
    }
}

/// The relay-wide rollup, plain: requests, tokens, reasoning when non-zero,
/// and the carried-load total.
fn aggregate_lines(totals: &RelayTotals) -> Vec<String> {
    let pool = &totals.totals;
    let mut lines = vec![
        format!(
            "requests {}  ({} ok, {} failed)",
            format_exact(pool.requests),
            format_exact(pool.successes),
            format_exact(pool.failures)
        ),
        format!(
            "tokens in {}  out {}  cache {}",
            format_count(pool.input),
            format_count(pool.output),
            format_count(pool.cache_read + pool.cache_write)
        ),
    ];
    if pool.reasoning != 0 {
        lines.push(format!("reasoning {}", format_count(pool.reasoning)));
    }
    lines.push(format!("total {}", format_count(totals.totals.tokens())));
    lines
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

/// The per-account token facts shown under `accounts`: in/out/cache, plus
/// reasoning only when that total is non-zero. Labelled like every other
/// fact row so the panel has one column, not one per section.
pub(crate) fn account_detail_facts(account: &Value) -> Vec<Fact> {
    let mut facts = vec![Fact::new(
        "tokens",
        &paint(
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
        ),
    )];
    let reasoning = i64_field(account, "totalReasoningOutputTokens");
    if reasoning != 0 {
        facts.push(Fact::new(
            "reasoning",
            &paint(DIM, &format_count(reasoning)),
        ));
    }
    facts
}

/// Footer rollup facts: requests, tokens, and reasoning when non-zero.
pub(crate) fn footer_facts(totals: &PoolTotals) -> Vec<Fact> {
    let mut lines = vec![Fact::new(
        "requests",
        &format!(
            "{}  ({} ok, {} failed)",
            paint(BOLD, &format_exact(totals.requests)),
            format_exact(totals.successes),
            format_exact(totals.failures)
        ),
    )];
    lines.push(Fact::new(
        "tokens",
        &format!(
            "in {}  out {}  cache {}",
            paint(BOLD, &format_count(totals.input)),
            paint(BOLD, &format_count(totals.output)),
            format_count(totals.cache_read + totals.cache_write),
        ),
    ));
    // Reasoning totals get their own row only when non-zero; one row for
    // all five fields cannot fit the fixed width.
    if totals.reasoning != 0 {
        lines.push(Fact::new(
            "reasoning",
            &paint(BOLD, &format_count(totals.reasoning)),
        ));
    }
    // Named for its scope: `status` prints `total` for the whole relay,
    // and one word must not mean two spans (ARCHITECTURE, "One word, one
    // scope").
    lines.push(Fact::new(
        "pool",
        &paint(BOLD, &format_count(totals.tokens())),
    ));
    lines
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
/// total the CLI prints resolves through here, so `status`, the pool
/// footers, the share bars and `usage`'s all-time row cannot disagree
/// about what they are summing (AC-11).
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
    // Only days that actually carry traffic are history; the rest of the
    // window is drawn but was never recorded. Saying "across 30 days"
    // when one day exists invites the reader to compare the total against
    // `status` and conclude the trend is broken, when it is only new.
    let recorded: Vec<&TrendDay> = days.iter().filter(|day| day.recorded).collect();
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
        Fact::new(
            "peak",
            &format!(
                "{} on {}",
                paint(BOLD, &format_count(peak.tokens)),
                peak.date
            ),
        ),
        Fact::new(
            "window",
            &format!(
                "{} across {} {} recorded",
                paint(BOLD, &format_count(total)),
                recorded.len(),
                if recorded.len() == 1 { "day" } else { "days" }
            ),
        ),
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
