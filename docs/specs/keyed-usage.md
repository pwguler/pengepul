# keyed-usage

## Goal

Teach core to resolve a Local API key to its Owner and identity, record a Usage event and a
Request log for every completed request, and refuse requests from a key past its Budget —
all of it active only when a database is configured, and absent when one is not.

## Non-goals

- **No metering behaviour when no database is configured.** With `database:` unset no row
  is written, no connection is opened, no budget is checked, and keys come from
  `config.yaml`. Fenced and tested, not assumed.
  Three fixes deliberately apply on both paths, because they correct counts that were
  simply wrong before and the per-Account totals behind `/admin/accounts` are shared:
  streaming requests to a configured provider now ask for usage, the `Generic` arm now
  reads it, and cached tokens are no longer counted twice. An unmetered relay's account
  totals therefore change — from wrong to right. Fencing them would have meant keeping a
  known-wrong number for the sake of a literal reading of this line.
- Core never creates, migrates, or alters a table. Prisma owns the schema; core issues
  hand-written queries against tables that already exist.
- No OAuth change. `pengepul login` for anthropic and codex is untouched, and no upstream
  credential — OAuth token or static key — is written to the database.
- Rate limiting stays keyed by IP. Per-key limiting is a separate work item.
- Operator settings stay in `config.yaml`. The new file keys are the database path, the log
  retention window and the admin token, and nothing else.
  **One exception, and it was a decision, not a slip:** the console registers providers, so
  with a database configured the `Provider` table is the routing authority and the file's
  `providers:` section is imported into it once. Reading the file as well would mean an
  endpoint deleted in the console kept routing, which is the opposite of registering it
  there. Nothing else moves: host, port, auth-dir, timeouts, cloaking, body limit, debug
  and the three new keys are read from the file and only the file.
- No UI, no console pages, no HTML **in `core/`**. The console under `web/` belongs to
  `docs/specs/console-shell.md` and rides the same branch; it is out of scope for this
  spec's criteria and is gated by that one.
- Usage rows are never pruned. Only Request logs age out.
- The per-Account totals behind `/admin/accounts` keep their shape and their meaning: which
  account is healthy, not who spent what. Their token counts do change, because the three
  counting fixes above apply on both paths.

## Acceptance criteria

- AC-1: With no `database:` configured, an inbound key is accepted iff it is in
  `config.yaml`'s `api-keys`, no database connection is opened, and no row is written.
  - Verify: a test asserting the relay serves a request with an `AppState` whose pool is
    `None`, and that the response and status match the pre-change behaviour.
- AC-2: With a database configured, a key is accepted iff a `Key` row matches
  `sha256(presented)` and `revokedAt` is null. Setting `revokedAt` refuses the very next
  request with no restart and no reload call.
- AC-3: Every client request to a generation route writes exactly one `UsageEvent`, and one
  means one: a request that fails over across several accounts records the outcome the
  client was handed, not one row per rejected attempt, and a request whose upstream never
  answers still records a row rather than vanishing. The count is asserted with `COUNT(*)`,
  because `fetch_one` takes the first of many rows without complaining.
  The row carries `keyId`, `provider`, `model`, all five token counts, `statusCode`, `ok`,
  `streamed`, `durationMs`, `startedAt`, and the four rates copied from the matching
  `Model` row at write time. A refused upstream response still records a row, with zero
  tokens and its real status.
  `count_tokens` is deliberately excluded: it is free upstream and agents call it before
  nearly every request, so recording it would bury real usage under costless rows.
- AC-4: A model with no `Model` row still yields a `UsageEvent` with tokens and a null
  `costUsd`. An unpriced model is never an error and never blocks a request.
- AC-5: `KeySpend` is upserted on every priced write, so month-to-date Spend is a single
  primary-key read rather than an aggregate over `UsageEvent`.
- AC-6: A key whose Spend has reached its Budget is refused with 429 and a message naming
  the key and the budget; a key below it is served. The check costs one query, taken
  together with the key lookup.
  - Verify: a test seeding `KeySpend` at and just under the budget, asserting 429 and 200.
- AC-7: A streaming request to a Generic provider records non-zero output tokens, closing
  the `ProviderKind::Generic => {}` hole at `app.rs:1760`.
  - Verify: a test driving a fake SSE stream carrying an OpenAI `usage` block through a
    configured provider and asserting the persisted `UsageEvent` token counts.
- AC-8: A `RequestLog` row holds the last user message and the completion text, each
  truncated to at most 8192 bytes on a character boundary, with `truncated` set when either
  was cut. The full resent context is never stored.
  - Verify: a test with a request whose context far exceeds the cap, asserting the stored
    length, the flag, and that an interior context message does not appear.
- AC-9: `RequestLog` rows older than the retention window are removed by a background task,
  and no `UsageEvent` row is ever removed by it.
  - Verify: a test seeding rows on both sides of the boundary and running the prune directly.
- AC-10: `/admin/*` requires the admin credential and refuses a valid relay key with 403.
  `pengepul accounts` and `pengepul accounts --reload` use that credential rather than
  `first_api_key`.
- AC-11: On first start with a database configured, keys already present in `config.yaml`
  are imported once as `Key` rows so an existing install keeps working, and the import is
  idempotent across restarts.
- AC-12: `config.yaml` learns `database:` without breaking `deny_unknown_fields`, and an
  existing config with no such key still loads.
- AC-13: A database that is unreachable at request time fails closed: the request is
  refused, never served as if unmetered.
- AC-14: An existing v0.4.0 `config.yaml` loads unchanged. `database:`,
  `log-retention-days:` and `admin-token:` are the new keys, all optional, and
  `deny_unknown_fields` still rejects a genuine typo. The third is forced by AC-10: admin
  access needs a credential, and the Non-goals keep settings in the file.
- AC-15: The relay runs standalone. With no `database:` set, core builds, starts and serves
  with the `web/` directory absent from the tree, and has no build-time or runtime dependency
  on it.
  - Verify: `cargo build --locked` and a live request, in a checkout with `web/` removed.
- AC-16: A database-backed install is manageable without the console:
  `pengepul keys add --name <n> --owner <email> [--budget <usd>]` prints the new key exactly
  once, `pengepul keys list` shows id, name, owner, prefix, budget and month-to-date spend,
  and `pengepul keys revoke <id>` takes the id positionally and takes effect on the next
  request.
- AC-17: With a database configured, an endpoint that exists only as a `Provider` row —
  never named in `config.yaml` — routes: `serve` builds an account manager for it, its
  `/v1/models` is fetched and advertised prefixed, and a Chat Completions request against
  `<id>/<model>` reaches it.
  - Verify: seed a `Provider` row against a config whose `providers:` is empty, resolve it
    through the same function `serve` uses, and drive a request through the resulting app;
    assert the resolved id, a 200, and that the upstream was really called.
- AC-17a: Nothing on the serving path writes a `Provider` row. `serve` and `login` read
  the table and never copy the file into it, so an endpoint deleted in the console stays
  deleted across every restart. The file's one turn is `pengepul providers import`, which
  the operator runs; `serve` warns by name for any `providers:` entry the table does not
  carry, so a prefix that stopped resolving says why rather than going quiet.
  `ON CONFLICT DO NOTHING` is not a substitute for this: a per-row upsert is idempotent
  only while the row exists, so an automatic import would resurrect a deleted endpoint on
  the next start, and every start after that.
  - Verify: import, delete the row directly, then resolve repeatedly — the endpoint must
    not come back. And a `providers:` entry never imported must not appear.
- AC-18: `login` and `serve` agree on what a provider is. `pengepul login --provider <id>
  --key <k>` saves a key for an endpoint that exists only in the database, and refuses one
  the database does not carry — the file naming it is not enough.
- AC-19: A `Provider` row whose `dialect` is not `openai_chat` is refused, not guessed at.
  ADR-0011 scopes generic providers to Chat Completions; core drops such a row with a
  warning naming the endpoint and the dialect, does not advertise its models, and `login`
  refuses to save it a key with a message naming the dialect rather than claiming the
  endpoint does not exist.
  - Verify: a seeded `anthropic_messages` row alongside a servable one; assert it is absent
    from the resolved providers, that its prefix answers 400, that nothing was sent
    upstream for it, and that `login` exits non-zero naming the dialect.
- AC-20: The one-event rule holds on the streaming path too. A streamed request that fails
  over records one `UsageEvent`, not one per rejected attempt. This is its own criterion
  because streaming reaches the decision through `stream_accounting` rather than
  `record_json_result`, and a stream is the common case: a double-count here inflates every
  report and every budget check on the path a busy relay hits most.
  - Verify: a streaming failover test asserting `COUNT(*) = 1` — `fetch_one` cannot tell one
    row from three, which is how this hid in the first place.
- AC-21: A key's `UsageEvent` and the `KeySpend` it adds to are written in one transaction.
  Two statements would let a failure between them leave an event nobody was charged for,
  reported to the caller as a write that failed entirely.
- AC-22: The admin token is compared without leaking how much of it matched, and its test
  sets `admin-token` to something other than the file's key — otherwise the assertion
  passes whichever credential the CLI picked and proves nothing.

## Verification

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Exercised end to end against a running relay, both modes:

```sh
# no database: v0.4.0 behaviour
pengepul serve &
curl -sS localhost:8317/v1/messages -H "Authorization: Bearer $CONFIG_KEY" ...

# database configured: metered
curl -sS localhost:8317/v1/messages -H "Authorization: Bearer $NAMED_KEY" ...
sqlite3 pengepul.db 'select keyId, model, inputTokens, outputTokens, costUsd from UsageEvent order by startedAt desc limit 1;'
sqlite3 pengepul.db 'select length(prompt), truncated from RequestLog order by createdAt desc limit 1;'
sqlite3 pengepul.db 'select * from KeySpend;'
```

An endpoint the file never named, against a stub upstream:

```sh
sqlite3 pengepul.db "insert into Provider (id,baseUrl,dialect,createdAt) \
  values ('dbonly','http://127.0.0.1:8791/v1','openai_chat',datetime('now'));"
pengepul login --provider dbonly --key $STUB_KEY   # the id comes from the table
pengepul serve &                                   # startup logs providers=1
curl -sS localhost:8317/v1/models -H "Authorization: Bearer $NAMED_KEY"      # dbonly/...
curl -sS localhost:8317/v1/chat/completions -H "Authorization: Bearer $NAMED_KEY" \
  -d '{"model":"dbonly/llama-3","messages":[{"role":"user","content":"ping"}]}'
```
