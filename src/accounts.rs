use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;
use serde_json::{Value, json};

use crate::tokens::{
    DayUsage, ModelUsage, PersistedUsage, RETENTION_DAYS, load_all_tokens, load_usage, save_token,
    save_usage, trim_days,
};
use crate::types::{
    AvailableAccount, ProviderId, ProviderKind, RefreshTokenExhaustedError, TokenData, UsageData,
};
use crate::utils::{local_today, now_iso, sha256_hex};

pub type RefreshFuture = Pin<Box<dyn Future<Output = Result<TokenData>> + Send>>;

pub type RefreshFn = Box<dyn Fn(String) -> RefreshFuture + Send + Sync>;

/// Every failure kind backs off the same: 1s, 2s, 4s, 8s, … per consecutive failure, capped
/// at 5 minutes, reset on the next success. Short first retries keep a single static key (or a
/// lone account) from being locked out by one transient error.
const FAILURE_BACKOFF: (f64, f64) = (1.0, 5.0 * 60.0);

/// Billing failures (an account out of credits or quota) do not recover mid-session the
/// way a transient error does, so the account sits out far longer before being retried.
const BILLING_COOLDOWN_SECONDS: f64 = 10.0 * 60.0;

const REAUTH_COOLDOWN_SECONDS: f64 = 24.0 * 60.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshPolicyKind {
    ExpiresLead,
    SinceLastRefresh,
    /// Static credentials: refresh is never due. The callback must never run.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshPolicy {
    pub kind: RefreshPolicyKind,
    pub seconds: i64,
}

#[derive(Debug, Clone)]
pub struct AccountResult {
    pub account: Option<AvailableAccount>,
    pub failure_kind: Option<String>,
    pub retry_after_seconds: Option<f64>,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            kind: RefreshPolicyKind::ExpiresLead,
            seconds: 4 * 60 * 60,
        }
    }
}

#[derive(Debug, Clone)]
struct AccountState {
    token: TokenData,
    cooldown_until: f64,
    failure_count: i64,
    last_failure_kind: Option<String>,
    last_error: Option<String>,
    last_failure_at: Option<String>,
    last_success_at: Option<String>,
    last_refresh_at: Option<String>,
    total_requests: i64,
    total_successes: i64,
    total_failures: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_creation_input_tokens: i64,
    total_cache_read_input_tokens: i64,
    total_reasoning_output_tokens: i64,
    /// Per-model successes and their tokens, keyed by upstream model name.
    /// Sorted by construction (`BTreeMap`) so the payload order is stable.
    /// Attempts and failures are not attributed: a model is only known once
    /// the upstream served it.
    models: BTreeMap<String, ModelUsage>,
    /// Per-local-day traffic, keyed `YYYY-MM-DD`. Sorted by construction,
    /// which is also chronological for that format (usage-trend AC-1).
    days: BTreeMap<String, DayUsage>,
    /// The local day each in-flight attempt opened on, oldest first. A
    /// list rather than a count because an outcome belongs to the bucket
    /// its attempt opened: a request spanning local midnight would
    /// otherwise leave one day short an outcome and the next short an
    /// attempt, which load-time repair then turns into an invented
    /// failure — nightly, for an operator working across midnight. A list
    /// rather than a flag because Rotation is in-flight-blind and the
    /// manager lock is released before the upstream call, so one Account
    /// serves several requests at once. In-memory only: a restart begins
    /// with nothing in flight.
    in_flight: Vec<String>,
}

impl AccountState {
    fn new(token: TokenData) -> Self {
        let last_refresh_at = token.last_refresh_at.clone();
        Self {
            token,
            cooldown_until: 0.0,
            failure_count: 0,
            last_failure_kind: None,
            last_error: None,
            last_failure_at: None,
            last_success_at: None,
            last_refresh_at,
            total_requests: 0,
            total_successes: 0,
            total_failures: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_creation_input_tokens: 0,
            total_cache_read_input_tokens: 0,
            total_reasoning_output_tokens: 0,
            models: BTreeMap::new(),
            days: BTreeMap::new(),
            in_flight: Vec::new(),
        }
    }

    /// Count one outcome, cumulative and in today's bucket, keeping
    /// `requests == successes + failures` true by construction.
    ///
    /// Every recorder goes through here. Two properties hold whatever the
    /// call sites do, so no future one has to remember them:
    ///
    /// - **An outcome implies an attempt.** If the outcomes would exceed
    ///   the attempts (a recorder fired without `record_attempt`), the
    ///   attempt is counted too rather than leaving `ok + failed > requests`
    ///   — the direction load-time repair cannot fix.
    /// - **One attempt, one outcome.** A second outcome for the same
    ///   attempt is refused, so a path that records twice (as the billing
    ///   rejection did) cannot double-count.
    ///
    fn settle(&mut self, success: bool) -> String {
        // The oldest attempt in flight reaches its outcome, booked to the
        // day it opened on rather than the day it finished: a request
        // spanning local midnight would otherwise leave one bucket short
        // an outcome and the next short an attempt.
        let day = if self.in_flight.is_empty() {
            // Nothing in flight: an outcome the relay recorded without an
            // attempt, or a second outcome for one already settled. Both
            // are answered the same way — count the attempt this outcome
            // implies, so `ok + failed` can never exceed `requests`, the
            // direction load-time repair cannot fix. A caller that must
            // not record twice applies its health without an outcome
            // instead (`record_billing_cooldown`).
            let today = local_today();
            self.total_requests += 1;
            self.day(&today).requests += 1;
            today
        } else {
            self.in_flight.remove(0)
        };
        if success {
            self.total_successes += 1;
            self.day(&day).successes += 1;
        } else {
            self.total_failures += 1;
            self.day(&day).failures += 1;
        }
        day
    }

    /// One day's bucket, opened on first touch. Trims the window first,
    /// so a long-lived process cannot serve an admin payload holding more
    /// history than its own file (usage-trend AC-4).
    fn day(&mut self, date: &str) -> &mut DayUsage {
        let cutoff = retention_cutoff();
        if self
            .days
            .keys()
            .next()
            .is_some_and(|oldest| *oldest < cutoff)
        {
            self.days = trim_days(&self.days, &cutoff);
        }
        self.days.entry(date.to_string()).or_default()
    }
}

impl From<&AccountState> for PersistedUsage {
    fn from(state: &AccountState) -> Self {
        Self {
            requests: state.total_requests,
            successes: state.total_successes,
            failures: state.total_failures,
            input_tokens: state.total_input_tokens,
            output_tokens: state.total_output_tokens,
            cache_creation_input_tokens: state.total_cache_creation_input_tokens,
            cache_read_input_tokens: state.total_cache_read_input_tokens,
            reasoning_output_tokens: state.total_reasoning_output_tokens,
            models: state.models.clone(),
            days: state.days.clone(),
        }
    }
}

pub struct AccountManager {
    auth_dir: PathBuf,
    provider: ProviderId,
    refresh: RefreshFn,
    refresh_policy: RefreshPolicy,
    accounts: BTreeMap<String, AccountState>,
    order: Vec<String>,
    last_used_index: Option<usize>,
    /// Usage counters read from disk at `load()` and merged into fresh
    /// account states; never updated afterwards. Writes rebuild the file
    /// from the live accounts, dropping unknown entries.
    persisted_usage: BTreeMap<String, PersistedUsage>,
}

impl AccountManager {
    #[must_use]
    pub fn new(
        auth_dir: PathBuf,
        provider: ProviderId,
        refresh: impl Fn(String) -> RefreshFuture + Send + Sync + 'static,
        refresh_policy: RefreshPolicy,
    ) -> Self {
        Self {
            auth_dir,
            provider,
            refresh: Box::new(refresh),
            refresh_policy,
            accounts: BTreeMap::new(),
            order: Vec::new(),
            last_used_index: None,
            persisted_usage: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Load provider token files from disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the auth directory exists but cannot be read.
    /// Read every token from the auth dir, merge persisted usage, and
    /// repair any attempt/outcome gap the files carry. The repair is
    /// written back immediately: an account that serves nothing after a
    /// restart would otherwise keep a stale file behind a correct panel.
    pub fn load(&mut self) -> Result<()> {
        self.persisted_usage = load_usage(&self.auth_dir, &self.provider);
        for token in load_all_tokens(&self.auth_dir, Some(&self.provider))? {
            self.upsert_loaded_token(token);
        }
        // Write the repair back now. An account that serves nothing after
        // a restart would otherwise leave a stale file behind a correct
        // panel, and the two would disagree until its next request.
        if !self.accounts.is_empty() {
            self.persist_usage();
        }
        Ok(())
    }

    /// Reload provider token files from disk and report changed account emails.
    ///
    /// # Errors
    ///
    /// Returns an error when the auth directory exists but cannot be read.
    pub fn reload(&mut self) -> Result<Value> {
        let mut added = Vec::new();
        let mut updated = Vec::new();
        let mut unchanged = Vec::new();
        for token in load_all_tokens(&self.auth_dir, Some(&self.provider))? {
            let Some(existing) = self.accounts.get_mut(&token.email) else {
                added.push(token.email.clone());
                self.upsert_loaded_token(token);
                continue;
            };
            if existing.token.access_token == token.access_token
                && existing.token.refresh_token == token.refresh_token
            {
                unchanged.push(token.email.clone());
                continue;
            }
            updated.push(token.email.clone());
            existing.token = token;
            existing.cooldown_until = 0.0;
            existing.failure_count = 0;
            existing.last_failure_kind = None;
            existing.last_error = None;
            existing.last_failure_at = None;
        }
        Ok(json!({
            "added": added,
            "updated": updated,
            "unchanged": unchanged
        }))
    }

    /// Refresh an account when its configured refresh policy says it is due.
    ///
    /// # Errors
    ///
    /// Returns an error when the refresh callback fails or refreshed token persistence fails.
    pub async fn refresh_if_due(&mut self, email: &str) -> Result<bool> {
        if !self
            .accounts
            .get(email)
            .is_some_and(|state| self.should_refresh(state))
        {
            return Ok(true);
        }
        self.refresh_account(email).await
    }

    /// Force-refresh one account.
    ///
    /// # Errors
    ///
    /// Returns an error when the refresh callback fails or refreshed token persistence fails.
    pub async fn refresh_account(&mut self, email: &str) -> Result<bool> {
        let Some(state) = self.accounts.get(email) else {
            return Ok(false);
        };
        let old_token = state.token.clone();
        let refreshed = match (self.refresh)(old_token.refresh_token.clone()).await {
            Ok(token) => token,
            Err(error) => {
                if let Some(exhausted) = error.downcast_ref::<RefreshTokenExhaustedError>() {
                    self.record_refresh_exhausted(email, &exhausted.reason);
                    return Ok(false);
                }
                return Err(error);
            }
        };
        let refresh_at = now_iso();
        let new_token = TokenData {
            access_token: refreshed.access_token,
            refresh_token: refreshed.refresh_token,
            email: if refreshed.email.is_empty() {
                old_token.email.clone()
            } else {
                refreshed.email
            },
            expires_at: refreshed.expires_at,
            account_uuid: if refreshed.account_uuid.is_empty() {
                old_token.account_uuid.clone()
            } else {
                refreshed.account_uuid
            },
            provider: self.provider.clone(),
            id_token: refreshed.id_token.or(old_token.id_token),
            last_refresh_at: Some(refresh_at.clone()),
            plan_type: refreshed.plan_type.or(old_token.plan_type),
        };
        save_token(&self.auth_dir, &new_token)?;
        if let Some(state) = self.accounts.get_mut(email) {
            state.token = new_token;
            state.cooldown_until = 0.0;
            state.failure_count = 0;
            state.last_failure_kind = None;
            state.last_error = None;
            state.last_failure_at = None;
            state.last_success_at = Some(refresh_at.clone());
            state.last_refresh_at = Some(refresh_at);
        }
        Ok(true)
    }

    pub fn record_success(&mut self, email: &str, usage: Option<&UsageData>, model: &str) {
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.cooldown_until = 0.0;
        state.failure_count = 0;
        state.last_failure_kind = None;
        state.last_error = None;
        state.last_failure_at = None;
        state.last_success_at = Some(now_iso());
        let day = state.settle(true);
        // Keyed by the upstream model name: what the provider billed, not
        // what the client happened to ask for. A success with no usage
        // block (count-tokens, or a 2xx whose usage will not parse) still
        // belongs to its model, so the counter opens either way.
        let counters = state.models.entry(model.to_string()).or_default();
        counters.successes += 1;
        if let Some(usage) = usage {
            counters.add_tokens(usage);
            state.total_input_tokens += usage.input_tokens;
            state.total_output_tokens += usage.output_tokens;
            state.total_cache_creation_input_tokens += usage.cache_creation_input_tokens;
            state.total_cache_read_input_tokens += usage.cache_read_input_tokens;
            state.total_reasoning_output_tokens += usage.reasoning_output_tokens;
            state.day(&day).add_tokens(usage);
        }
        self.persist_usage();
    }

    /// An attempt the relay refused before reaching upstream — a dialect
    /// this Provider cannot serve. It counts as a failed request so the
    /// panels reconcile, but it is not the Account's fault: no cooldown,
    /// no failure streak, nothing that would take it out of Rotation.
    pub fn record_refusal(&mut self, email: &str) {
        if let Some(state) = self.accounts.get_mut(email) {
            let _ = state.settle(false);
        }
        // Like every other outcome: the attempt was already persisted, so
        // a refusal held only in memory would resurface as a permanent
        // gap after a restart.
        self.persist_usage();
    }

    /// Apply the billing cooldown and its failure streak to an account
    /// whose request already reached an outcome. Counts no new outcome:
    /// `requests` must keep equalling `successes + failures`.
    pub fn record_billing_cooldown(&mut self, email: &str, detail: &str) {
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.failure_count += 1;
        state.last_failure_kind = Some("billing".to_string());
        state.last_failure_at = Some(now_iso());
        state.last_error = Some(format!("billing: {detail}"));
        state.cooldown_until = unix_now() + BILLING_COOLDOWN_SECONDS;
        self.persist_usage();
    }

    pub fn record_attempt(&mut self, email: &str) {
        if let Some(state) = self.accounts.get_mut(email) {
            let today = local_today();
            state.total_requests += 1;
            state.day(&today).requests += 1;
            state.in_flight.push(today);
            self.persist_usage();
        }
    }

    pub fn record_failure(&mut self, email: &str, kind: &str, detail: Option<&str>) {
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.failure_count += 1;
        let _ = state.settle(false);
        state.last_failure_at = Some(now_iso());
        let (base, maximum) = if kind == "billing" {
            (BILLING_COOLDOWN_SECONDS, BILLING_COOLDOWN_SECONDS)
        } else {
            FAILURE_BACKOFF
        };
        let multiplier = 2_f64.powi(i32::try_from(state.failure_count - 1).unwrap_or(0));
        let cooldown = unix_now() + (base * multiplier).min(maximum);
        // A cooldown only ever grows. A reauth lockout is 24 hours; a
        // failure recorded after it must not collapse the account back to
        // seconds and re-select it into a failure loop.
        if cooldown > state.cooldown_until {
            state.cooldown_until = cooldown;
            state.last_failure_kind = Some(kind.to_string());
            state.last_error =
                Some(detail.map_or_else(|| kind.to_string(), |detail| format!("{kind}: {detail}")));
        }
        self.persist_usage();
    }

    pub fn record_refresh_exhausted(&mut self, email: &str, reason: &str) {
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.failure_count += 1;
        let _ = state.settle(false);
        state.last_failure_kind = Some("auth".to_string());
        state.last_failure_at = Some(now_iso());
        state.last_error = Some(format!(
            "refresh token {reason}; re-run login for {}",
            self.provider
        ));
        state.cooldown_until = unix_now() + REAUTH_COOLDOWN_SECONDS;
        self.persist_usage();
    }

    #[must_use]
    pub fn snapshots(&self) -> Vec<Value> {
        // The window bounds what is served, not only what is written: a
        // process idle since the window moved would otherwise carry
        // buckets its own file no longer holds.
        let cutoff = retention_cutoff();
        let now = unix_now();
        self.accounts
            .values()
            .map(|state| {
                let cooldown_remaining = (state.cooldown_until - now).max(0.0);
                json!({
                    "email": state.token.email,
                    "available": cooldown_remaining == 0.0,
                    "cooldownUntil": if cooldown_remaining == 0.0 { 0.0 } else { state.cooldown_until },
                    "failureCount": state.failure_count,
                    "lastError": state.last_error,
                    "lastFailureAt": state.last_failure_at,
                    "lastSuccessAt": state.last_success_at,
                    "lastRefreshAt": state.last_refresh_at,
                    "totalRequests": state.total_requests,
                    "totalSuccesses": state.total_successes,
                    "totalFailures": state.total_failures,
                    "totalInputTokens": state.total_input_tokens,
                    "totalOutputTokens": state.total_output_tokens,
                    "totalCacheCreationInputTokens": state.total_cache_creation_input_tokens,
                    "totalCacheReadInputTokens": state.total_cache_read_input_tokens,
                    "totalReasoningOutputTokens": state.total_reasoning_output_tokens,
                    "models": state.models.iter().map(|(model, usage)| json!({
                        "model": model,
                        "successes": usage.successes,
                        "inputTokens": usage.input_tokens,
                        "outputTokens": usage.output_tokens,
                        "cacheCreationInputTokens": usage.cache_creation_input_tokens,
                        "cacheReadInputTokens": usage.cache_read_input_tokens,
                        "reasoningOutputTokens": usage.reasoning_output_tokens
                    })).collect::<Vec<_>>(),
                    "days": state.days.iter().filter(|(date, _)| **date >= cutoff).map(|(date, day)| json!({
                        "date": date,
                        "requests": day.requests,
                        "successes": day.successes,
                        "failures": day.failures,
                        "inputTokens": day.input_tokens,
                        "outputTokens": day.output_tokens,
                        "cacheCreationInputTokens": day.cache_creation_input_tokens,
                        "cacheReadInputTokens": day.cache_read_input_tokens,
                        "reasoningOutputTokens": day.reasoning_output_tokens
                    })).collect::<Vec<_>>(),
                    "expiresAt": state.token.expires_at,
                    "refreshing": false,
                    "planType": state.token.plan_type
                })
            })
            .collect()
    }

    #[must_use]
    pub fn next_account(&mut self) -> Option<AvailableAccount> {
        self.next_account_result().account
    }

    #[must_use]
    pub fn next_account_result(&mut self) -> AccountResult {
        if self.order.is_empty() {
            return AccountResult {
                account: None,
                failure_kind: None,
                retry_after_seconds: None,
            };
        }
        let now = unix_now();
        let start = self.last_used_index.map_or(0, |index| index + 1);
        for offset in 0..self.order.len() {
            let index = (start + offset) % self.order.len();
            let email = &self.order[index];
            let state = &self.accounts[email];
            if state.cooldown_until <= now {
                self.last_used_index = Some(index);
                return AccountResult {
                    account: Some(self.available_account(state)),
                    failure_kind: None,
                    retry_after_seconds: None,
                };
            }
        }
        let best = self
            .order
            .iter()
            .filter_map(|email| self.accounts.get(email))
            .min_by(|left, right| {
                let left_remaining = left.cooldown_until - now;
                let right_remaining = right.cooldown_until - now;
                left_remaining.total_cmp(&right_remaining)
            });
        AccountResult {
            account: None,
            failure_kind: best.and_then(|state| state.last_failure_kind.clone()),
            retry_after_seconds: best.map(|state| (state.cooldown_until - now).max(0.0)),
        }
    }

    #[must_use]
    pub fn account(&self, email: &str) -> Option<AvailableAccount> {
        self.accounts
            .get(email)
            .map(|state| self.available_account(state))
    }

    fn should_refresh(&self, state: &AccountState) -> bool {
        match self.refresh_policy.kind {
            RefreshPolicyKind::Never => false,
            RefreshPolicyKind::SinceLastRefresh => {
                let Some(last_refresh_at) = &state.last_refresh_at else {
                    return true;
                };
                let Ok(last_refresh_at) = chrono::DateTime::parse_from_rfc3339(last_refresh_at)
                else {
                    return true;
                };
                chrono::Utc::now()
                    .signed_duration_since(last_refresh_at.with_timezone(&chrono::Utc))
                    .num_seconds()
                    >= self.refresh_policy.seconds
            }
            RefreshPolicyKind::ExpiresLead => {
                let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(&state.token.expires_at)
                else {
                    return true;
                };
                expires_at
                    .with_timezone(&chrono::Utc)
                    .signed_duration_since(chrono::Utc::now())
                    .num_seconds()
                    <= self.refresh_policy.seconds
            }
        }
    }

    /// Snapshot every account's counters to `usage.json`. Write failures
    /// are swallowed: losing an increment of observability must never
    /// fail the request that produced it.
    fn persist_usage(&self) {
        // Only loaded accounts are written: entries in the file for
        // unknown emails are dropped rather than carried forever.
        // One cutoff for the whole write, so every account's window ends on
        // the same day (usage-trend AC-4).
        let cutoff = retention_cutoff();
        let usage: BTreeMap<String, PersistedUsage> = self
            .accounts
            .iter()
            .map(|(email, state)| {
                let mut persisted = PersistedUsage::from(state);
                persisted.days = trim_days(&persisted.days, &cutoff);
                (email.clone(), persisted)
            })
            .collect();
        // Write failures are swallowed: losing an increment of
        // observability must never fail the request that produced it.
        let _ = save_usage(&self.auth_dir, &self.provider, &usage);
    }

    fn upsert_loaded_token(&mut self, token: TokenData) {
        let email = token.email.clone();
        if self.accounts.contains_key(&email) {
            self.accounts
                .get_mut(&email)
                .expect("account exists after contains_key")
                .token = token;
        } else {
            let mut state = AccountState::new(token);
            if let Some(usage) = self.persisted_usage.get(&email) {
                state.total_requests = usage.requests;
                state.total_successes = usage.successes;
                state.total_failures = usage.failures;
                state.total_input_tokens = usage.input_tokens;
                state.total_output_tokens = usage.output_tokens;
                state.total_cache_creation_input_tokens = usage.cache_creation_input_tokens;
                state.total_cache_read_input_tokens = usage.cache_read_input_tokens;
                state.total_reasoning_output_tokens = usage.reasoning_output_tokens;
                state.models = usage.models.clone();
                state.days = usage.days.clone();
                reconcile_loaded_counters(&mut state);
            }
            self.order.push(email.clone());
            self.accounts.insert(email, state);
        }
    }

    fn available_account(&self, state: &AccountState) -> AvailableAccount {
        AvailableAccount {
            token: state.token.clone(),
            device_id: sha256_hex(&format!(
                "{}:{}",
                self.auth_dir.display(),
                state.token.email
            ))[..32]
                .to_string(),
            account_uuid: state.token.account_uuid.clone(),
            provider: self.provider.clone(),
            chatgpt_account_id: (self.provider.kind == ProviderKind::Codex)
                .then(|| state.token.account_uuid.clone()),
        }
    }
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

/// Close any attempt/outcome gap a file carries, in either direction. At
/// load nothing is in flight, so both gaps are knowable: an attempt with
/// no outcome did not succeed, so it failed; an outcome with no attempt
/// implies the attempt that produced it. Successes are never rewritten
/// and no recorded outcome is discarded (ARCHITECTURE, "Every attempt
/// reaches exactly one outcome").
fn reconcile_loaded_counters(state: &mut AccountState) {
    let outcomes = state.total_successes + state.total_failures;
    if state.total_requests > outcomes {
        // Attempts with no outcome: they did not succeed, so they failed.
        state.total_failures += state.total_requests - outcomes;
    } else if outcomes > state.total_requests {
        // Outcomes with no attempt, written before the seam existed. An
        // outcome implies an attempt, so the attempts rise to meet them —
        // never the reverse, which would discard a recorded outcome.
        state.total_requests = outcomes;
    }
    for day in state.days.values_mut() {
        let outcomes = day.successes + day.failures;
        if day.requests > outcomes {
            day.failures += day.requests - outcomes;
        } else if outcomes > day.requests {
            day.requests = outcomes;
        }
    }
    // A loaded account has nothing in flight: whatever it was doing when
    // the process died is over.
    state.in_flight.clear();
}

/// The oldest local day a write keeps: today minus the retention window.
/// The clock reach lives here, at the edge that writes the file; the trim
/// itself is pure over this value (usage-trend AC-4).
fn retention_cutoff() -> String {
    (chrono::Local::now() - chrono::Duration::days(RETENTION_DAYS - 1))
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod settle_tests {
    use super::{AccountState, DayUsage};
    use crate::types::{ProviderId, TokenData};

    fn state() -> AccountState {
        AccountState::new(TokenData {
            access_token: "a".to_string(),
            refresh_token: String::new(),
            email: "k@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct".to_string(),
            provider: ProviderId::generic("commandcode"),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        })
    }

    fn bucket(state: &AccountState, date: &str) -> DayUsage {
        state.days.get(date).cloned().unwrap_or_default()
    }

    /// An outcome is booked to the day its attempt opened on, not the day
    /// it finished. A request spanning local midnight would otherwise
    /// leave one bucket short an outcome and the next short an attempt,
    /// which load-time repair turns into an invented failure — nightly,
    /// for an operator working across midnight.
    #[test]
    fn an_outcome_settles_in_the_day_its_attempt_opened() {
        let mut state = state();
        // An attempt that opened yesterday, still in flight at midnight.
        state.total_requests += 1;
        state.day("2026-09-05").requests += 1;
        state.in_flight.push("2026-09-05".to_string());

        // Its outcome arrives after the date has rolled over.
        let settled_on = state.settle(true);

        assert_eq!(settled_on, "2026-09-05", "the outcome left its attempt");
        let opened = bucket(&state, "2026-09-05");
        assert_eq!(opened.requests, 1);
        assert_eq!(opened.successes, 1, "the bucket is unbalanced");
        // And nothing was booked to the new day.
        assert_eq!(bucket(&state, "2026-09-06").successes, 0);
    }

    /// Attempts settle oldest-first, so two in flight across a midnight
    /// each keep their own day.
    #[test]
    fn two_attempts_across_midnight_keep_their_own_days() {
        let mut state = state();
        for date in ["2026-09-05", "2026-09-06"] {
            state.total_requests += 1;
            state.day(date).requests += 1;
            state.in_flight.push((*date).to_string());
        }

        assert_eq!(state.settle(true), "2026-09-05");
        assert_eq!(state.settle(false), "2026-09-06");

        assert_eq!(bucket(&state, "2026-09-05").successes, 1);
        assert_eq!(bucket(&state, "2026-09-05").failures, 0);
        assert_eq!(bucket(&state, "2026-09-06").successes, 0);
        assert_eq!(bucket(&state, "2026-09-06").failures, 1);
        for date in ["2026-09-05", "2026-09-06"] {
            let day = bucket(&state, date);
            assert_eq!(
                day.requests,
                day.successes + day.failures,
                "bucket {date} is unbalanced"
            );
        }
    }
}
