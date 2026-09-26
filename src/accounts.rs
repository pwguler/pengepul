use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;
use serde_json::{Value, json};

use crate::tokens::{
    DayUsage, ModelUsage, PersistedUsage, RETENTION_DAYS, load_all_tokens, load_disabled_tokens,
    load_usage, save_token, save_usage, set_credential_disabled, trim_days, usage_file_unreadable,
    usage_path,
};
use crate::types::{
    AvailableAccount, ProviderId, ProviderKind, RefreshTokenExhaustedError, TokenData, UsageData,
};
use crate::utils::{local_today, now_iso, sha256_hex};

pub type RefreshFuture = Pin<Box<dyn Future<Output = Result<TokenData>> + Send>>;

pub type RefreshFn = Box<dyn Fn(String) -> RefreshFuture + Send + Sync>;

/// Every failure kind cools down the same way at the base: 1s, 2s, 4s, 8s, … per
/// consecutive failure, capped at 5 minutes and reset on the next success. Short first
/// retries keep a single static key (or a lone account) from being locked out by one
/// transient error. The *ceiling* is what varies — billing, reauth, and a credential
/// that has never succeeded each sit out longer; see [`AccountState::failure_cooldown`].
///
/// A Cooldown needs a sibling to be worth anything: it exists to send the next request
/// somewhere else, so a Pool that holds one account earns none at all (ADR-0027).
const FAILURE_BACKOFF: (f64, f64) = (1.0, 5.0 * 60.0);

/// Billing failures (an account out of credits or quota) do not recover mid-session the
/// way a transient error does, so the account sits out far longer before being retried.
const BILLING_COOLDOWN_SECONDS: f64 = 10.0 * 60.0;

/// The cooldown ceiling for a credential that has never once succeeded.
///
/// [`FAILURE_BACKOFF`]'s five-minute ceiling assumes the account is good and the
/// error was transient, which is the wrong assumption for an account with no
/// success to its name: a static key the upstream rejects will never recover on
/// its own, and retrying it every five minutes forever is pure waste (one such key
/// drew 91 requests and served none). Raising only the *ceiling* and keeping the
/// 1s, 2s, 4s opening means a fresh account's first blips are still retried fast,
/// an upstream outage that hits every account cannot park the whole pool (the
/// cooldown still has to be earned by a long streak), and the first success resets
/// `failure_count` and puts the account straight back into Rotation.
const NEVER_SUCCEEDED_COOLDOWN_SECONDS: f64 = 60.0 * 60.0;

const REAUTH_COOLDOWN_SECONDS: f64 = 24.0 * 60.0 * 60.0;

/// How many conversations keep an account preference before the whole map
/// is dropped. A relay serves a handful of harnesses at once; this is far
/// above that and still bounded, so a long-lived process cannot grow one
/// entry per conversation forever.
const AFFINITY_CAPACITY: usize = 1024;

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

/// Where the account a request was handed to came from. Logged where Rotation
/// decides, so a
/// cache miss can be correlated with the account switch that caused it: a
/// conversation re-reads its prefix cold every time its recorded account
/// stops being selectable, and nothing else in the logs says so (ADR-0017).
#[derive(Debug, Clone, Copy)]
pub enum AffinityOutcome {
    /// The conversation's recorded account was still selectable and was reused.
    Honored,
    /// No record, or the recorded account was on Cooldown: Rotation chose.
    Rotation,
}

impl AffinityOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Honored => "honored",
            Self::Rotation => "rotation",
        }
    }
}

/// What `disable` or `enable` did to an account (disable-an-account).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleOutcome {
    /// Taken out of the Pool; `enabled_left` accounts still serve it.
    Disabled {
        enabled_left: usize,
    },
    AlreadyDisabled,
    /// A Disabled account put back in the Pool.
    Enabled,
    /// An account on Cooldown made selectable now, its failure streak reset.
    CooldownCleared,
    /// The Cooldown is cleared, but the account is in Reauth: only a login
    /// makes it serve again.
    NeedsLogin,
    AlreadyAvailable,
}

impl ToggleOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled { .. } => "disabled",
            Self::AlreadyDisabled => "already_disabled",
            Self::Enabled => "enabled",
            Self::CooldownCleared => "cooldown_cleared",
            Self::NeedsLogin => "needs_login",
            Self::AlreadyAvailable => "already_available",
        }
    }
}

/// Why `disable` or `enable` could not act.
#[derive(Debug)]
pub enum ToggleError {
    /// No account of this Pool, with or without a credential, has the id.
    NotFound,
    /// The account's record is kept but its credential is gone (ADR-0026):
    /// there is no file to rename, and only a login brings one back.
    NoCredential,
    /// The rename itself failed.
    Io(anyhow::Error),
}

impl std::fmt::Display for ToggleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such account"),
            Self::NoCredential => write!(f, "the account's credential is gone"),
            Self::Io(error) => write!(f, "{error:#}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountResult {
    pub account: Option<AvailableAccount>,
    pub failure_kind: Option<String>,
    pub retry_after_seconds: Option<f64>,
    pub affinity: AffinityOutcome,
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
    /// Held out of the Pool by the operator; the credential is on disk as
    /// `<id>.json.disabled`. A Disabled account is in `accounts`, so an
    /// outcome in flight is still booked, but never in `order`.
    disabled: bool,
    /// The refresh token was rejected: nothing but a login restores it.
    reauth: bool,
    total_requests: i64,
    total_successes: i64,
    total_failures: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_creation_input_tokens: i64,
    /// The 1h-retention share of the line above (ADR-0018).
    total_cache_creation_1h_input_tokens: i64,
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
}

impl AccountState {
    /// The (base, maximum) cooldown this account's next failure earns.
    ///
    /// A billing failure sits out longer than a transient one because it does not
    /// clear mid-session, and an account with no success to its name sits out longer
    /// still — see ADR-0020 for why the ceiling, and not the base, is what moves.
    fn failure_cooldown(&self, kind: &str) -> (f64, f64) {
        let billing = kind == "billing";
        if self.total_successes == 0 {
            let base = if billing {
                BILLING_COOLDOWN_SECONDS
            } else {
                FAILURE_BACKOFF.0
            };
            (base, NEVER_SUCCEEDED_COOLDOWN_SECONDS)
        } else if billing {
            (BILLING_COOLDOWN_SECONDS, BILLING_COOLDOWN_SECONDS)
        } else {
            FAILURE_BACKOFF
        }
    }

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
            disabled: false,
            reauth: false,
            total_requests: 0,
            total_successes: 0,
            total_failures: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_creation_input_tokens: 0,
            total_cache_creation_1h_input_tokens: 0,
            total_cache_read_input_tokens: 0,
            total_reasoning_output_tokens: 0,
            models: BTreeMap::new(),
            days: BTreeMap::new(),
        }
    }

    /// Count one request and its outcome, cumulatively and in the day the
    /// outcome arrived, keeping `requests == successes + failures` true by
    /// construction.
    ///
    /// Every recorder goes through here, and the property that holds
    /// whatever the call sites do is that both sides are written by the
    /// same call. Nothing is counted at dispatch, so there is no window
    /// between the two writes for an interleaving, a concurrent outcome
    /// or a leaked attempt to open (ADR-0015).
    ///
    /// Two consequences worth stating, because both were bugs before:
    ///
    /// - **It does not refuse a second outcome.** There is nothing to
    ///   refuse against: a path that records twice counts two requests,
    ///   inflating `requests` rather than corrupting the balance. A caller
    ///   that must not record twice applies its health without an outcome
    ///   instead (`record_billing_cooldown`).
    /// - **A request in flight is counted nowhere**, and an attempt lost
    ///   to a crash is never counted at all. The relay counts what it
    ///   observed: a number it cannot justify is worse than one it does
    ///   not have.
    ///
    fn settle(&mut self, success: bool) -> String {
        // A counter counts outcomes, never attempts (ADR-0015). The same
        // call writes both sides, in the day the outcome arrived, so
        // `requests == successes + failures` holds cumulatively and per
        // day by construction: no interleaving, no concurrent outcome and
        // no future call site can put them out of step. A request still in
        // flight is counted nowhere.
        let today = local_today();
        self.total_requests += 1;
        let day = self.day(&today);
        day.requests += 1;
        if success {
            day.successes += 1;
            self.total_successes += 1;
        } else {
            day.failures += 1;
            self.total_failures += 1;
        }
        today
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
            cache_creation_1h_input_tokens: state.total_cache_creation_1h_input_tokens,
            cache_read_input_tokens: state.total_cache_read_input_tokens,
            reasoning_output_tokens: state.total_reasoning_output_tokens,
            models: state.models.clone(),
            days: state.days.clone(),
            last_success_at: state.last_success_at.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Usage snapshots
// ---------------------------------------------------------------------------

/// One record's usage counters, under the field names an account entry has
/// always used.
///
/// Shared by an account with a credential and one without, rather than written
/// twice: the CLI sums and lists both through these names, and two builders would
/// be two vocabularies to keep in step (usage-after-removal).
fn usage_json(email: &str, usage: &PersistedUsage, cutoff: &str) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("email".to_string(), json!(email));
    // A served success only, and persisted, so a record without a credential still
    // says when it last served (last-ok).
    fields.insert("lastSuccessAt".to_string(), json!(usage.last_success_at));
    fields.insert("totalRequests".to_string(), json!(usage.requests));
    fields.insert("totalSuccesses".to_string(), json!(usage.successes));
    fields.insert("totalFailures".to_string(), json!(usage.failures));
    fields.insert("totalInputTokens".to_string(), json!(usage.input_tokens));
    fields.insert("totalOutputTokens".to_string(), json!(usage.output_tokens));
    fields.insert(
        "totalCacheCreationInputTokens".to_string(),
        json!(usage.cache_creation_input_tokens),
    );
    fields.insert(
        "totalCacheCreation1hInputTokens".to_string(),
        json!(usage.cache_creation_1h_input_tokens),
    );
    fields.insert(
        "totalCacheReadInputTokens".to_string(),
        json!(usage.cache_read_input_tokens),
    );
    fields.insert(
        "totalReasoningOutputTokens".to_string(),
        json!(usage.reasoning_output_tokens),
    );
    fields.insert(
        "models".to_string(),
        Value::Array(
            usage
                .models
                .iter()
                .map(|(model, usage)| {
                    json!({
                        "model": model,
                        "successes": usage.successes,
                        "inputTokens": usage.input_tokens,
                        "outputTokens": usage.output_tokens,
                        "cacheCreationInputTokens": usage.cache_creation_input_tokens,
                        "cacheCreation1hInputTokens": usage.cache_creation_1h_input_tokens,
                        "cacheReadInputTokens": usage.cache_read_input_tokens,
                        "reasoningOutputTokens": usage.reasoning_output_tokens
                    })
                })
                .collect(),
        ),
    );
    // The window bounds what is served, not only what is written: a record
    // carrying a bucket its own file no longer holds is not served either.
    fields.insert(
        "days".to_string(),
        Value::Array(
            usage
                .days
                .iter()
                .filter(|(date, _)| date.as_str() >= cutoff)
                .map(|(date, day)| {
                    json!({
                        "date": date,
                        "requests": day.requests,
                        "successes": day.successes,
                        "failures": day.failures,
                        "inputTokens": day.input_tokens,
                        "outputTokens": day.output_tokens,
                        "cacheCreationInputTokens": day.cache_creation_input_tokens,
                        "cacheCreation1hInputTokens": day.cache_creation_1h_input_tokens,
                        "cacheReadInputTokens": day.cache_read_input_tokens,
                        "reasoningOutputTokens": day.reasoning_output_tokens
                    })
                })
                .collect(),
        ),
    );
    fields
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
    /// account states. Never updated afterwards: writes rebuild the accounts that
    /// loaded from memory and carry the rest of the file through unchanged
    /// (usage-after-removal).
    persisted_usage: BTreeMap<String, PersistedUsage>,
    /// Conversation key -> the account that served it last.
    affinity: BTreeMap<String, String>,
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
            affinity: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn account_count(&self) -> usize {
        // The serving count: a Disabled account is listed, never handed a request.
        self.order.len()
    }

    /// Whether a Cooldown has a sibling to send the next request to.
    ///
    /// It does not, for a Pool that holds one account: withholding that account is
    /// withholding the only way this Provider has to serve, which turns the vendor's
    /// own error into a local one the client can do nothing with (ADR-0027).
    fn has_sibling(&self) -> bool {
        self.order.len() > 1
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
        for token in load_disabled_tokens(&self.auth_dir, &self.provider)? {
            // An account with both files is enabled: the `.json` is the newer act.
            if !self.accounts.contains_key(&token.email) {
                self.upsert_disabled_token(token);
            }
        }
        // `total_successes` is a policy input, not only a counter: it decides whether an
        // account earns the never-succeeded cooldown ceiling (ADR-0020). So a usage file
        // that exists and yielded nothing is not merely missing observability — writing the
        // repair back would overwrite recoverable history with zeros and reclassify every
        // proven account as one that has never succeeded. Leave the file alone and say so:
        // counters regenerate, an hour-long cooldown earned by a lost file does not undo
        // itself.
        if !self.accounts.is_empty() && usage_file_unreadable(&self.auth_dir, &self.provider) {
            tracing::warn!(
                path = %usage_path(&self.auth_dir, &self.provider).display(),
                provider = %self.provider,
                "usage file is unreadable; leaving it in place rather than overwriting it with \
                 zeros, which would reclassify these accounts as never-succeeded"
            );
            return Ok(());
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
        let enabled = load_all_tokens(&self.auth_dir, Some(&self.provider))?;
        let disabled = load_disabled_tokens(&self.auth_dir, &self.provider)?;
        // Reload mirrors the directory: an account whose credential is on disk under
        // neither name leaves the Pool as the removed-credential record ADR-0026
        // describes, exactly as a restart would leave it (disable-an-account AC-10).
        let gone: Vec<String> = self
            .accounts
            .keys()
            .filter(|email| {
                !enabled.iter().any(|token| &token.email == *email)
                    && !disabled.iter().any(|token| &token.email == *email)
            })
            .cloned()
            .collect();
        for email in gone {
            self.leave_rotation(&email);
            if let Some(state) = self.accounts.remove(&email) {
                self.persisted_usage
                    .insert(email, PersistedUsage::from(&state));
            }
        }
        for token in disabled {
            if enabled.iter().any(|held| held.email == token.email) {
                continue;
            }
            match self.accounts.get_mut(&token.email) {
                Some(state) if !state.disabled => {
                    state.disabled = true;
                    let email = token.email.clone();
                    self.leave_rotation(&email);
                }
                Some(_) => {}
                None => self.upsert_disabled_token(token),
            }
        }
        for token in enabled {
            if self
                .accounts
                .get(&token.email)
                .is_some_and(|state| state.disabled)
            {
                self.join_rotation(&token.email);
            }
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
            existing.reauth = false;
        }
        Ok(json!({
            "added": added,
            "updated": updated,
            "unchanged": unchanged
        }))
    }

    /// Whether this Pool has a record for the account, with or without a
    /// credential.
    #[must_use]
    pub fn holds(&self, email: &str) -> bool {
        self.accounts.contains_key(email) || self.persisted_usage.contains_key(email)
    }

    /// Take an account out of the Pool: its credential is renamed
    /// `<id>.json.disabled`, and Rotation, affinity and Failover pass it over
    /// until [`Self::enable`] (disable-an-account).
    ///
    /// # Errors
    ///
    /// [`ToggleError::NotFound`] for an id the Pool has no record of,
    /// [`ToggleError::NoCredential`] for a record whose credential is gone, and
    /// [`ToggleError::Io`] when the rename fails.
    pub fn disable(&mut self, email: &str) -> Result<ToggleOutcome, ToggleError> {
        let Some(state) = self.accounts.get(email) else {
            return Err(self.missing(email));
        };
        if state.disabled {
            return Ok(ToggleOutcome::AlreadyDisabled);
        }
        if !set_credential_disabled(&self.auth_dir, &self.provider, email, true)
            .map_err(ToggleError::Io)?
        {
            return Err(ToggleError::NoCredential);
        }
        if let Some(state) = self.accounts.get_mut(email) {
            state.disabled = true;
        }
        self.leave_rotation(email);
        Ok(ToggleOutcome::Disabled {
            enabled_left: self.order.len(),
        })
    }

    /// Make an account serve on the next request: a Disabled one is renamed
    /// back into the Pool, and one on Cooldown has it cleared with its failure
    /// streak (disable-an-account).
    ///
    /// # Errors
    ///
    /// As [`Self::disable`].
    pub fn enable(&mut self, email: &str) -> Result<ToggleOutcome, ToggleError> {
        let Some(state) = self.accounts.get(email) else {
            return Err(self.missing(email));
        };
        if state.disabled {
            if !set_credential_disabled(&self.auth_dir, &self.provider, email, false)
                .map_err(ToggleError::Io)?
            {
                return Err(ToggleError::NoCredential);
            }
            self.join_rotation(email);
            if let Some(state) = self.accounts.get_mut(email) {
                state.cooldown_until = 0.0;
                state.failure_count = 0;
            }
            return Ok(ToggleOutcome::Enabled);
        }
        let on_cooldown = state.cooldown_until > unix_now();
        let reauth = state.reauth;
        if !on_cooldown && !reauth {
            return Ok(ToggleOutcome::AlreadyAvailable);
        }
        if let Some(state) = self.accounts.get_mut(email) {
            state.cooldown_until = 0.0;
            state.failure_count = 0;
        }
        Ok(if reauth {
            ToggleOutcome::NeedsLogin
        } else {
            ToggleOutcome::CooldownCleared
        })
    }

    fn missing(&self, email: &str) -> ToggleError {
        if self.persisted_usage.contains_key(email) {
            ToggleError::NoCredential
        } else {
            ToggleError::NotFound
        }
    }

    /// Drop an account from the rotation order, keeping Rotation's place: the
    /// account after the one used last is still next.
    fn leave_rotation(&mut self, email: &str) {
        let Some(index) = self.order.iter().position(|held| held == email) else {
            return;
        };
        self.order.remove(index);
        self.last_used_index = match self.last_used_index {
            Some(last) if last >= index => last.checked_sub(1),
            other => other,
        };
        if self.order.is_empty() {
            self.last_used_index = None;
        }
    }

    fn join_rotation(&mut self, email: &str) {
        if let Some(state) = self.accounts.get_mut(email) {
            state.disabled = false;
        }
        if !self.order.iter().any(|held| held == email) {
            self.order.push(email.to_string());
        }
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
        // A refresh saves `<id>.json`, which would enable a Disabled account behind
        // the operator's back. A request already in flight holds its token and
        // needs nothing written (disable-an-account AC-13).
        if state.disabled {
            return Ok(true);
        }
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
            state.reauth = false;
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
        state.reauth = false;
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
            state.total_cache_creation_1h_input_tokens += usage.cache_creation_1h_input_tokens;
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
        let has_sibling = self.has_sibling();
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.failure_count += 1;
        state.last_failure_at = Some(now_iso());
        // A lone account keeps serving (see `has_sibling`); what a sibling's cooldown
        // would do is the guard below.
        if !has_sibling {
            state.last_failure_kind = Some("billing".to_string());
            state.last_error = Some(format!("billing: {detail}"));
            self.persist_usage();
            return;
        }
        // An account with successes behind it gets the flat billing cooldown it
        // always has. One that has never succeeded escalates instead: it is not
        // going to clear mid-session either, and a flat ten minutes forever is the
        // cycle that let a depleted key be drawn six times an hour indefinitely.
        let (base, maximum) = state.failure_cooldown("billing");
        let multiplier = 2_f64.powi(i32::try_from(state.failure_count - 1).unwrap_or(0));
        let cooldown = unix_now() + (base * multiplier).min(maximum);
        // Same rule as `record_failure`: a cooldown only ever grows, so a billing
        // rejection cannot collapse a 24-hour reauth cooldown back to minutes — and the
        // recorded reason belongs inside that guard for the same reason its sibling keeps
        // it there. Writing "billing" outside would leave an account parked on the reauth
        // cooldown with `lastError` claiming exhausted credits, hiding the one field
        // CONTEXT.md tells the operator to read for a Reauth.
        if cooldown > state.cooldown_until {
            state.cooldown_until = cooldown;
            state.last_failure_kind = Some("billing".to_string());
            state.last_error = Some(format!("billing: {detail}"));
        }
        self.persist_usage();
    }

    pub fn record_failure(&mut self, email: &str, kind: &str, detail: Option<&str>) {
        let has_sibling = self.has_sibling();
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.failure_count += 1;
        let _ = state.settle(false);
        state.last_failure_at = Some(now_iso());
        // A lone account records the failure — streak, counters, `lastError`, the
        // fields the panels read — and keeps serving, so the client sees the
        // upstream's own error instead of "no available account". Nothing is
        // inherited either: a Cooldown is never persisted, so a Pool that lost a
        // credential between restarts cannot come up holding one (ADR-0027).
        if !has_sibling {
            state.last_failure_kind = Some(kind.to_string());
            state.last_error =
                Some(detail.map_or_else(|| kind.to_string(), |detail| format!("{kind}: {detail}")));
            self.persist_usage();
            return;
        }
        let (base, maximum) = state.failure_cooldown(kind);
        let multiplier = 2_f64.powi(i32::try_from(state.failure_count - 1).unwrap_or(0));
        let cooldown = unix_now() + (base * multiplier).min(maximum);
        // A cooldown only ever grows. A reauth cooldown is 24 hours; a
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
        let has_sibling = self.has_sibling();
        let Some(state) = self.accounts.get_mut(email) else {
            return;
        };
        state.failure_count += 1;
        let _ = state.settle(false);
        state.last_failure_kind = Some("auth".to_string());
        state.last_failure_at = Some(now_iso());
        state.reauth = true;
        state.last_error = Some(format!(
            "refresh token {reason}; re-run login for {}",
            self.provider
        ));
        // A Reauth is the one failure a Cooldown cannot help with at all: the
        // credential will not serve again without a human. For a lone account the
        // relay asks for one refresh per request instead, and every request answers
        // with the message that says what to do, which is more use than a 24-hour
        // local refusal (ADR-0027).
        if has_sibling {
            state.cooldown_until = unix_now() + REAUTH_COOLDOWN_SECONDS;
        }
        self.persist_usage();
    }

    /// Every account the relay has a record for, in account-key order.
    ///
    /// That includes a record whose credential the auth directory no longer holds:
    /// removing a credential removes a way to serve, not the traffic it carried.
    /// The entry keeps the account's field names and carries the liveness defaults
    /// of an account that cannot serve, so no reader needs a second shape and no
    /// view needs a second rule (usage-after-removal).
    ///
    /// Serving is not this list. Rotation walks `accounts`, which holds only
    /// records with a credential, so such a record is never handed a request.
    #[must_use]
    pub fn snapshots(&self) -> Vec<Value> {
        let cutoff = retention_cutoff();
        let now = unix_now();
        let mut records: BTreeMap<String, Value> = self
            .accounts
            .iter()
            .map(|(email, state)| {
                let cooldown_remaining = (state.cooldown_until - now).max(0.0);
                let mut fields =
                    usage_json(&state.token.email, &PersistedUsage::from(state), &cutoff);
                fields.insert(
                    "available".to_string(),
                    json!(cooldown_remaining == 0.0 && !state.disabled),
                );
                fields.insert("disabled".to_string(), json!(state.disabled));
                fields.insert(
                    "cooldownUntil".to_string(),
                    json!(if cooldown_remaining == 0.0 {
                        0.0
                    } else {
                        state.cooldown_until
                    }),
                );
                fields.insert("failureCount".to_string(), json!(state.failure_count));
                fields.insert("lastError".to_string(), json!(state.last_error));
                fields.insert("lastFailureAt".to_string(), json!(state.last_failure_at));
                fields.insert("lastRefreshAt".to_string(), json!(state.last_refresh_at));
                fields.insert("expiresAt".to_string(), json!(state.token.expires_at));
                fields.insert("refreshing".to_string(), json!(false));
                fields.insert("planType".to_string(), json!(state.token.plan_type));
                (email.clone(), Value::Object(fields))
            })
            .collect();
        for (email, usage) in &self.persisted_usage {
            if self.accounts.contains_key(email) {
                continue;
            }
            let mut fields = usage_json(email, usage, &cutoff);
            // An account that cannot serve: no Cooldown to name (there is nothing
            // to wait out) and no failure to report (nothing has been attempted since
            // its credential went away).
            fields.insert("available".to_string(), json!(false));
            fields.insert("disabled".to_string(), json!(false));
            fields.insert("cooldownUntil".to_string(), json!(0.0));
            fields.insert("failureCount".to_string(), json!(0));
            fields.insert("lastError".to_string(), Value::Null);
            fields.insert("lastFailureAt".to_string(), Value::Null);
            fields.insert("lastRefreshAt".to_string(), Value::Null);
            fields.insert("expiresAt".to_string(), json!(""));
            fields.insert("refreshing".to_string(), json!(false));
            fields.insert("planType".to_string(), Value::Null);
            records.insert(email.clone(), Value::Object(fields));
        }
        records.into_values().collect()
    }

    #[must_use]
    pub fn next_account(&mut self) -> Option<AvailableAccount> {
        self.next_account_result().account
    }

    #[must_use]
    /// The account a conversation should keep using, when it still can.
    ///
    /// Every upstream holds its own prompt cache, so moving a conversation
    /// between accounts throws away the prefix it just paid to cache.
    /// Affinity is a preference and never a pin: an account on Cooldown
    /// falls through to Rotation, which is what keeps availability ahead of
    /// cache efficiency.
    pub fn account_for(&mut self, affinity: &str) -> AccountResult {
        let now = unix_now();
        if let Some(email) = self.affinity.get(affinity)
            && let Some(state) = self.accounts.get(email)
            && state.cooldown_until <= now
            && !state.disabled
        {
            let account = self.available_account(state);
            if let Some(index) = self.order.iter().position(|held| held == email) {
                self.last_used_index = Some(index);
            }
            return AccountResult {
                account: Some(account),
                failure_kind: None,
                retry_after_seconds: None,
                affinity: AffinityOutcome::Honored,
            };
        }
        let result = self.next_account_result();
        if let Some(account) = &result.account {
            // Bounded by the same rule the pool is: one entry per live
            // conversation, dropped once it outnumbers the accounts by more
            // than a working set. Nothing here is persisted — a restart
            // re-learns it on the next turn, at the price of one cold read.
            if self.affinity.len() >= AFFINITY_CAPACITY {
                self.affinity.clear();
            }
            self.affinity
                .insert(affinity.to_string(), account.token.email.clone());
        }
        result
    }

    pub fn next_account_result(&mut self) -> AccountResult {
        if self.order.is_empty() {
            return AccountResult {
                account: None,
                failure_kind: None,
                retry_after_seconds: None,
                affinity: AffinityOutcome::Rotation,
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
                    affinity: AffinityOutcome::Rotation,
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
            affinity: AffinityOutcome::Rotation,
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
        // One cutoff for the whole write, so every record's window ends on the
        // same day (usage-trend AC-4).
        let cutoff = retention_cutoff();
        // Records whose credential is gone are carried through, not dropped: the
        // file is the record of what the relay carried, not only a snapshot of what
        // it can serve, and `status` and `usage` count the difference
        // (usage-after-removal).
        let mut usage: BTreeMap<String, PersistedUsage> = self
            .persisted_usage
            .iter()
            .filter(|(email, _)| !self.accounts.contains_key(*email))
            .map(|(email, persisted)| {
                let mut persisted = persisted.clone();
                persisted.days = trim_days(&persisted.days, &cutoff);
                (email.clone(), persisted)
            })
            .collect();
        // A loaded account overwrites its own record: its in-memory counters are the
        // running ones.
        for (email, state) in &self.accounts {
            let mut persisted = PersistedUsage::from(state);
            persisted.days = trim_days(&persisted.days, &cutoff);
            usage.insert(email.clone(), persisted);
        }
        // Write failures are swallowed: losing an increment of
        // observability must never fail the request that produced it.
        let _ = save_usage(&self.auth_dir, &self.provider, &usage);
    }

    /// Load a Disabled account: listed and counted like any other, never in
    /// the rotation order.
    fn upsert_disabled_token(&mut self, token: TokenData) {
        let email = token.email.clone();
        self.upsert_loaded_token(token);
        if let Some(state) = self.accounts.get_mut(&email) {
            state.disabled = true;
        }
        self.leave_rotation(&email);
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
                state.total_cache_creation_1h_input_tokens = usage.cache_creation_1h_input_tokens;
                state.total_cache_read_input_tokens = usage.cache_read_input_tokens;
                state.total_reasoning_output_tokens = usage.reasoning_output_tokens;
                state.models = usage.models.clone();
                state.days = usage.days.clone();
                state.last_success_at.clone_from(&usage.last_success_at);
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
/// reaches its outcome").
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

    /// AC-1/AC-2: the counting rule. A request that has been dispatched
    /// but not finished is counted nowhere; the call that records its
    /// outcome is the same call that counts the request (ADR-0015).
    #[test]
    fn a_dispatched_request_counts_nothing_until_it_settles() {
        let mut state = state();

        // Dispatch is not a recorded event: nothing to call, and nothing
        // counted. The relay counts what it observed (ADR-0015).
        assert_eq!(state.total_requests, 0);
        assert!(state.days.is_empty(), "a bucket exists before any outcome");

        state.settle(true);

        assert_eq!(state.total_requests, 1);
        assert_eq!(state.total_successes, 1);
        let today = bucket(&state, &crate::utils::local_today());
        assert_eq!(today.requests, 1, "the outcome did not count its request");
        assert_eq!(today.successes, 1);
    }

    /// A local date `offset` days from today, as the store keys them.
    fn day_offset(offset: i64) -> String {
        (chrono::Local::now() + chrono::Duration::days(offset))
            .format("%Y-%m-%d")
            .to_string()
    }

    /// AC-1/AC-4: an outcome books to the day it arrived, so a request
    /// spanning local midnight counts once, on the day it finished. This
    /// is what removes the need for attempt identity: the seam never
    /// books to another day, so it never needs to know which attempt it
    /// is settling (ADR-0015).
    #[test]
    fn an_outcome_books_to_the_day_it_arrived() {
        let yesterday = day_offset(-1);
        let today = crate::utils::local_today();
        let mut state = state();
        // Yesterday saw traffic of its own.
        state.day(&yesterday).requests += 1;
        state.day(&yesterday).successes += 1;

        // A request dispatched yesterday, finishing today.
        let settled_on = state.settle(true);

        assert_eq!(settled_on, today, "booked to a day it did not arrive in");
        let arrived = bucket(&state, &today);
        assert_eq!(arrived.requests, 1);
        assert_eq!(arrived.successes, 1);
        // Yesterday is untouched: the outcome did not reach back.
        let earlier = bucket(&state, &yesterday);
        assert_eq!(earlier.requests, 1, "an earlier day was rewritten");
        assert_eq!(earlier.successes, 1);
    }

    /// AC-5: every bucket balances, whatever the order of outcomes.
    #[test]
    fn concurrent_outcomes_each_count_their_own_request() {
        let mut state = state();
        state.settle(true);
        state.settle(false);
        state.settle(true);

        assert_eq!(state.total_requests, 3);
        assert_eq!(state.total_successes, 2);
        assert_eq!(state.total_failures, 1);
        let today = bucket(&state, &crate::utils::local_today());
        assert_eq!(today.requests, 3);
        assert_eq!(today.requests, today.successes + today.failures);
    }
}
