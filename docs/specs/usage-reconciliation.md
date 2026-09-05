# usage-reconciliation

> **Draft.** The destination is named and the frontier is sketched. No
> decision below is settled, and nothing is implemented from this file
> yet. Work it with `drill`, one open decision per session.

## Destination

Every number the CLI prints about requests is trustworthy enough to audit,
not merely useful enough to read: `requests == successes + failures` per
Account and per local day, each outcome attributed to the attempt that
produced it, and no counter written to disk that the relay cannot justify.

## Decisions so far

The reconciliation work that shipped with `usage-trend` — the settling
seam, Refusals, load-time repair, both-direction repair, the in-flight
day list. It closed a real gap the operator hit (`1,404 requests, 1,398
ok, 0 failed`) and survived five review rounds, but it was never drilled:
its invariants were written after its code and revised three times.
This draft is the drill it did not get.

## Open decisions

### 1. What identifies an attempt? — **HITL**

`settle` pops the *oldest* in-flight entry, so it settles "an attempt",
never "this attempt". Counts stay correct; day attribution can be wrong
when two attempts straddle local midnight.

The judge's proposal, and the shape `StreamAccounting` already uses for
one path: `record_attempt` returns an `Attempt` guard carrying its id and
opening day; `settle` consumes it **by value**, so a second settle is a
compile error and a dropped guard records a failure on `Drop`.

The fork is cost, not correctness: the guard must travel through 8
accounting call sites in `app.rs` and survive an async boundary into the
stream path. Roughly 200 lines, and it deletes `in_flight`, most of
`reconcile_loaded_counters`' reason to exist, and several paragraphs of
invariant prose.

- **A**: build the guard. Makes double-count and leaks unrepresentable.
- **B**: keep the list, accept wrong day attribution across midnight.
- **C**: something narrower — an id on the list entry without the guard.

### 2. What bounds `in_flight`? — **HITL**

`src/accounts.rs:99`. `record_attempt` pushes; only `settle` and a
restart pop. Any path recording an attempt without settling leaks an
entry permanently, and each leaked entry misattributes a *later*
outcome's day. There is no cap, no timeout, no reaper. Five review rounds
found leaking paths one at a time; nothing structurally prevents the
next.

Resolved by decision 1A (a dropped guard settles itself). Needs its own
answer only if 1 lands on B or C.

### 3. Should the relay repair persisted counters at all? — **HITL**

`reconcile_loaded_counters` rewrites `usage.json` at every load. It fixed
the operator's real 6-request gap, is idempotent, and is tested in both
directions — but it is the one piece that can *destroy* history rather
than only misreport it, and it runs before the process is known healthy.

- **A**: keep repairing at load.
- **B**: repair, but write a `repaired: {...}` record beside the counters
  so the operator can see what was changed.
- **C**: never rewrite; show the gap in the panel and leave the file
  alone.

### 4. Is a datastore the answer? — **AFK**

Repeatedly proposed and repeatedly the wrong tool for *this* bug: a
database changes where counters live, not whether an outcome knows its
attempt. But `docs/specs/keyed-usage.md`, `usage-console.md` and
`web-console.md` describe a settled-but-unbuilt Postgres design for
history beyond 90 days, cross-process reconciliation, and queries.
Research whether that design subsumes this one or is orthogonal to it.
Findings to `docs/research/usage-reconciliation-datastore.md`.

## Not yet specified

- Multi-process safety. `persist_usage` is whole-file last-writer-wins;
  two relays on one auth dir would clobber each other. No decision needs
  it today (one relay per auth dir), and nothing enforces that.
- Whether `requests` should count attempts or *completed* attempts. The
  in-flight window makes `status` read one low while a request streams,
  which is honest but confused the operator once already.

## Out of scope

- The `usage-trend` feature itself: `pengepul usage`, daily buckets,
  retention, the sparkline. Shipped and reviewed; this draft does not
  reopen it.
- Per-harness attribution, which the operator queued separately and which
  needs its own drill (one shared API key today, so nothing distinguishes
  callers).
