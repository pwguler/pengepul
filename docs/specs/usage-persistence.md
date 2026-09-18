# usage-persistence

## Goal

Per-account usage counters (**total requests, successes, failures,
tokens in/out/cache/reasoning**, and the cooldown state) survive:

- relay restarts (`systemctl restart pengepul`, crash, reboot)
- binary upgrades (new install replaces the process, data stays)
- config edits and `login` of other accounts

The counters live today only in memory on `AccountState`; every restart
zeroes them. They must be persisted to disk and reloaded on startup.

## Shape

- One file per provider: `~/.pengepul/<provider>/usage.json`, same
  directory as the token files, mode 0600, dir 0700.
- Content: a JSON object keyed by account email; each entry carries the
  persisted fields: `total_requests`, `total_successes`,
  `total_failures`, `total_input_tokens`, `total_output_tokens`,
  `total_cache_creation_input_tokens`, `total_cache_read_input_tokens`,
  `total_reasoning_output_tokens`.
- Written after every `record_success` / `record_failure` /
  `record_attempt` / `record_refresh_exhausted` that changed a counter.
  Writes are atomic (temp file + rename) so a crash never leaves a
  truncated file.
- Loaded in `AccountManager::load`: entries merge into the in-memory
  state by email; emails without a usage entry start at zero.
- Token rotation (refresh) must NOT reset the counters; only failure-
  derived transient state (cooldown, failure_count, last_error) may
  reset, as today.
- Deleting `usage.json` resets that provider's counters (explicit user
  action; documented).

## Acceptance criteria

- AC-1: after `record_success` with usage, a manager re-created from
  disk (same auth dir) reports the same `totalRequests`/token totals
  through `snapshots()`.
- AC-2: `record_failure` persists; a re-created manager shows the same
  `totalFailures` (cooldown itself is not persisted: a fresh process
  must not stay blocked).
- AC-3: `record_attempt` (requests) persists.
- AC-4: ~~unknown emails in `usage.json` are ignored; accounts added
  later start at zero; `load` with no file succeeds and yields zeros.~~
  **The second half is reversed by `usage-after-removal`**: an email with no token
  file is still not an account — it never joins rotation — but its record is no
  longer dropped by the next write, and it is listed as an account that cannot
  serve. The load half stands: `load` with no file succeeds and yields zeros, and
  an entry is only ever merged into an account that has a credential.
- AC-5: a corrupted `usage.json` does not break startup — the manager
  loads with zeros (permissive, like token loading) and overwrites the
  file on the next write.
- AC-6: admin payload (`/admin/accounts`) after restart shows the
  persisted totals, so `pengepul status` shows them.

## Non-goals

- No per-request history/timeline: only the running totals persist.
- No generic server change to `/admin/*` payload shape: same fields as
  today, just surviving restarts.
- No persistence of cooldown *walls* across restarts (a restart is an
  operator action; the account retries immediately).

## Revisions

- **Deleting a credential no longer deletes its counters.** This spec's shape said
  writes rebuild the file from the loaded accounts, and its AC-4 said a stranger's
  entry is dropped "instead of keeping it". `usage-after-removal` reverses that
  half, at the user's ask: the traffic behind a removed key happened, and the
  operator asked to keep seeing it in `usage` and `status`. What survives is
  unchanged — one file per Provider under the auth dir, `0600`, atomic
  temp-plus-rename, per email — and so is everything else in this spec.
