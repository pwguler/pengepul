# Research: does the unbuilt datastore design subsume usage-reconciliation?

Input for decision 4 of `docs/specs/usage-reconciliation.md:72`. Sources: the six
specs named there, `src/accounts.rs`, `src/tokens.rs`, `src/app.rs`.

**State of the art:** none of it is built. No `web/` directory exists, and
`database:` appears nowhere in `src/`. Everything below is spec text, not code.

**Correction to the draft's premise:** `usage-reconciliation.md:76` calls it "a
settled-but-unbuilt Postgres design". The specs say SQLite: `workspace-split.md:43`
gitignores "the SQLite database and its `-wal`/`-shm` sidecars", and
`keyed-usage.md:163-165` verifies with `sqlite3 pengepul.db`. Prisma and Next.js are
right; Postgres is not in any spec. This does not change the verdict but the draft
should be corrected.

## 1. What it stores, and at what granularity

Per-request rows, plus one aggregate table.

- `UsageEvent` — one row per client request, `workspace-split.md:73-95`. Carries
  `keyId, provider, model`, five token counts, `costUsd`, four rate snapshots,
  `startedAt`, `durationMs`, `statusCode`, `ok`, `streamed`.
- `keyed-usage.md:50` — "Every client request to a generation route writes exactly one
  `UsageEvent`, and one means one: a request that fails over across several accounts
  records the outcome the client was handed, not one row per rejected attempt".
- `KeySpend` — month-granular running total, `workspace-split.md:65-71`; `usage-console.md:21`
  calls it "a month-granular running total that exists so budget enforcement stays cheap".
- `RequestLog` — truncated prompt/completion, `workspace-split.md:97-103`, pruned on a
  retention window (`keyed-usage.md:36`: "Usage rows are never pruned. Only Request logs age out.").

**The decisive detail: `UsageEvent` has no account column.** Its identity axis is
`keyId` — the inbound Local API key — not the upstream Account email that
`AccountManager` counts by (`src/accounts.rs:212`, keyed by email). The row records
which *customer* spent, not which *account* served.

## 2. Does it inherently solve attempt/outcome identity?

**No. It sidesteps the question by never modelling an attempt.**

The row is written at outcome, not at attempt. Every field that decides when it can be
written is outcome-only: `durationMs`, `statusCode`, `ok`, `costUsd`
(`workspace-split.md:88-92`). No spec describes an attempt-time insert followed by an
outcome-time update. `keyed-usage.md:50` is explicit that failover attempts produce *no*
rows — "not one row per rejected attempt". So the design does not have attempt identity;
it has outcome identity, and deliberately discards attempts.

Two consequences:

- For its own dataset the day-attribution bug cannot occur, because `startedAt` is a
  column on the row rather than a bucket chosen by popping a list. Attribution is by
  construction, not by ordering. That is a real property — but it is a property of
  *rows*, obtained by not having attempts, not by linking them.
- `KeySpend` is the same shape as the current defect in miniature: a counter incremented
  beside a row. `keyed-usage.md:63` upserts it "on every priced write". It is protected
  by `keyed-usage.md:139` (AC-21): event and spend "are written in one transaction".
  That is the correct fix for *that* pairing, and it is available only because a database
  is present. Nothing analogous is proposed for the per-Account counters.

## 3. Superset, orthogonal, or subset?

**Orthogonal, and the specs say so themselves.**

`keyed-usage.md:37` — "The per-Account totals behind `/admin/accounts` keep their shape
and their meaning: which account is healthy, not who spent what." The database is the
"who spent what" axis; the JSON file is the "which account is healthy" axis. They answer
different questions on different keys.

`usage-console.md:13` — "**No core change at all.** ... `core/` is not opened." The
console work item never touches the accounting seam.

Therefore: build the entire database design, and `record_attempt` /
`settle` / `in_flight` in `src/accounts.rs:99,149,445` are untouched. `settle` still
pops the oldest entry; a leaked entry still misattributes a later outcome's day; the
guard is still unwritten. **The identity bug survives the migration intact.**

The overlap is narrow: both count requests and tokens. The unbuilt design covers
"history beyond 90 days" and "queries" as the draft says (`usage-reconciliation.md:74`).
It does not cover cross-process reconciliation of *account* counters — no spec mentions it.

## 4. What the JSON file answers that the design does not

Every one of these is answered by the JSON file today, and the specs confirm the
database *does not replace* it — it sits beside it:

- **Works with no external service.** `keyed-usage.md:11` — "With `database:` unset no row
  is written, no connection is opened, no budget is checked". Unmetered mode is a fenced
  first-class mode, so account counters must keep working without a database. The JSON
  file is not optional infrastructure; it is the only path in the default deployment.
- **Single static binary.** `keyed-usage.md:95` and `web-console.md:54-56` — "core builds,
  starts and serves with the `web/` directory absent from the tree", verified by
  `cargo build --locked` in such a checkout. But: the SQLite file, `sqlite3`, and Prisma's
  generated client are runtime/tooling surface for anyone who *does* enable it.
- **No migration step.** The design explicitly requires one and refuses to run it itself:
  `keyed-usage.md:20` — "Core never creates, migrates, or alters a table"; `workspace-split.md:139-140`
  — "`pnpm prisma migrate dev --name init` is the operator's to run ... The relay itself
  never migrates." An operator upgrading the binary must run a Node toolchain to get a schema.
- **Readable with `cat`.** Not addressed by any spec. The nearest thing is the `sqlite3`
  shell in `keyed-usage.md:163-165`, which is a different capability: it needs a tool
  installed and a query written. A stalled relay's counters are readable today with
  `cat ~/.pengepul/<pool>/usage.json`; under SQLite with a live writer they are not,
  without a client and possibly not during a WAL checkpoint.

**Not answered by any spec:** whether account-level counters should ever move into the
database at all. No spec proposes it; `keyed-usage.md:37` implies the opposite.

## 5. Verdict on decision 1 (the attempt guard)

**BEFORE, and independent of — not instead of — any datastore work.**

The two do not compete. The guard changes *how an outcome finds its attempt* inside
`AccountManager`; the datastore changes *where per-key history lives*. Building the
datastore leaves `settle`'s `self.in_flight.remove(0)` (`src/accounts.rs:171`) exactly
as it is, so the bug ships either way. And the guard is a precondition for a clean
metered path, not an obstacle to it: `keyed-usage.md:50` (AC-3) and `:132` (AC-20)
both demand "exactly one" row per request across failover and streaming — the same
double-count/leak class the guard makes unrepresentable, in the same call sites in
`app.rs`. Writing the guard first means the database path inherits an accounting seam
that already knows which attempt it is settling.

What would have to be true for each answer:

- **BEFORE (recommended).** True if the identity defect is real in the unmetered default
  mode — it is; that is the only mode that exists today — and if the guard is confined to
  `AccountManager` and its 8 call sites. Both hold. Cost is the ~200 lines the draft
  estimates, and it deletes `in_flight` and most of `reconcile_loaded_counters`, so it
  also shrinks decisions 2 and 3 rather than deferring them.
- **AFTER.** Would require the datastore to become the sole source of account counters,
  making guard work throwaway. Nothing in the specs proposes this, and `keyed-usage.md:37`
  and `:11` both point the other way. Also requires the datastore to be closer to shipping
  than 200 lines of Rust — it is six unbuilt specs and a Node toolchain.
- **INSTEAD OF.** Would require `UsageEvent` to carry the account identity and the relay
  to read account health from SQL. That is not the schema (`workspace-split.md:73-95` has
  no account column) and is contradicted by `usage-console.md:13`'s "no core change at
  all". This is the option the draft's working position rejects, and the specs support the
  rejection.

**Contradictions flagged:** (a) "Postgres" in `usage-reconciliation.md:76` vs SQLite in
`workspace-split.md:43` / `keyed-usage.md:163`. (b) `usage-reconciliation.md:74` credits
the design with "cross-process reconciliation"; no spec claims this, and the multi-process
hazard named under "Not yet specified" (whole-file last-writer-wins `persist_usage`,
`src/accounts.rs`) is untouched by it, since account counters stay in the file.
