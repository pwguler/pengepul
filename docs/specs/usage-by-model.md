# usage-by-model

## Goal

Per-account usage counters gain a **per-model breakdown**. `pengepul
accounts` shows, under each account, which models that account served and
what they cost — and repeats the breakdown aggregated per pool. The
counters persist next to the existing ones in
`~/.pengepul/<provider>/usage.json`.

Today every token is attributed to an account only; the relay knows the
model at accounting time but throws it away.

## Non-goals

- **No change to `status`.** Under `status-total-only` it prints one
  aggregate block; models are an `accounts` concern.
- **No per-model failure attribution.** Failures and attempts are
  recorded before a model is confirmed served, and `record_failure`'s
  call sites do not all carry a model. Per-model counters count
  **successes and their tokens** only; the account-level
  `requests/ok/failed` lines stay the source of truth for attempts.
- **No pool-level model aggregate.** Models are shown under the account
  that served them, once.
- **No backfill.** Counters accumulated before this change have no model
  attribution and stay only in the account totals (user's choice:
  "diamkan saja"). Per-model lines therefore may sum to less than the
  account total until old history is outgrown. No `untracked` line.
- **No per-model cooldown, routing, or limits.** Display and accounting
  only; account selection is untouched.
- **No new endpoint.** The existing `GET /admin/accounts` payload carries
  the breakdown.
- **Key growth is bounded by upstream, not by config.** Model names come
  from `upstream_model()` after `resolve_id`, which accepts any
  `<configured-provider>/<anything>` by prefix and any bare id matching a
  provider heuristic — so the key space is "whatever upstream answered 2xx
  for", not the configured model list. `AccountState.models` is neither
  capped nor pruned, and `persist_usage` rewrites the whole map on every
  outcome. Acceptable at the observed scale (a handful of models per
  account); revisit if a caller ever sprays generated model names.

## Acceptance criteria

- AC-1: `AccountManager::record_success` takes the model name and adds
  the usage to a per-model counter for that account, keyed by the
  **upstream** model name (what the provider bills), in addition to the
  existing account totals. Both stream and JSON paths pass it. A success
  carrying no usage block still counts against its model.
- AC-2: A per-model counter holds: successes, input, output,
  cache-creation, cache-read, and reasoning tokens. Two successes for the
  same model on the same account accumulate; two different models stay
  separate; the same model on two accounts stays per-account.
- AC-3: `GET /admin/accounts` includes, per account, a `models` array of
  `{ "model", "successes", "inputTokens", "outputTokens",
  "cacheCreationInputTokens", "cacheReadInputTokens",
  "reasoningOutputTokens" }`, in a deterministic (name-sorted) order.
  Accounts with no per-model history emit an empty array.
- AC-4: The counters persist: after `record_success`, `usage.json` holds
  a `models` map per email; a manager built over that file restores the
  per-model counters. A file written before this change (no `models`
  key) loads with empty per-model history and intact account totals.
- AC-5: `pengepul accounts` (rich) prints under each account row, one
  model per two lines: `<model>  <n> ok  <total>` then indented
  `in X  out Y  cache Z`, sorted by total tokens descending, ties broken
  by name. Accounts with no per-model history print no model lines.
- AC-6: **Withdrawn.** The pool footer carries no `by model` aggregate —
  see Revisions. The per-account lines are the only breakdown.
- AC-7: `Style::Plain` `accounts` carries the same information in plain
  form, indented under the account. Plain does not clip the model name at
  all (no fixed columns to keep aligned), so it is the surface that can
  always be trusted for the full id.
- AC-8: Model names render whole at their catalog widths: the name cell
  is fitted to the longest name actually present (capped so the box never
  breaks — the widest shipped id is 28 chars, the cap is 38), and two
  names sharing a long prefix stay distinguishable. Panel lines are
  exactly 64 visible columns.
- AC-9: `total` on a model line = in + out + cache-read +
  cache-creation, matching the share-bar and relay-total definition;
  reasoning is excluded from the total and shown only in the detail line
  when non-zero.

## Revisions

- The mockup labelled the model line `612 req`. Since per-model counters
  only ever count successes (see non-goals), the rendered label is
  **`ok`**, matching the account row's existing `679 ok` vocabulary.
  `req` would have implied attempts, which are not attributed per model.
- Placement: the user chose **both** — nested under each account *and*
  aggregated per pool — but the pool half was later withdrawn; see the
  AC-6 entry below.
- **Name column fitted, not fixed.** The first cut clipped names at 22
  columns; the reviewer showed the shipped catalog reaches 28, so names
  lost characters (its collision example used prefixed ids, which
  `upstream_model` strips — no real collision existed). A fixed 38 fixed
  the clipping but left a 25-column gap after short names, so the column
  is now fitted to the rows present and capped at 38.
- **Width assertions strengthened.** `<= 64` held for any renderer at all
  (`panel_row` clips to 64), so it could not catch the clipping bug it
  was meant to catch. The panel tests now assert `== 64`, and the lib
  width test's fixture carries a `models` array so model rows are
  actually exercised.
- **AC-6 narrowed, then withdrawn.** First attempt: aggregate in every
  pool footer. Seen live, with one contributing account it repeated that
  account's own lines verbatim (`claude-opus-5  2 ok  330.4K` twice in
  one panel), so it was gated on two or more contributors. The user then
  asked for it to go entirely ("by modelnya ga usah"): a pool aggregate
  is a sum of lines already on screen, and `status` already carries the
  pool-level totals. Models now appear once, under the account that
  served them. `pool_model_rows` is gone with it.

## Verification

```sh
cargo test --test accounts
cargo test --test cli
cargo test --lib
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual: drive a real request through the relay for two different models,
then `pengepul accounts` and `cat ~/.pengepul/anthropic/usage.json`.
