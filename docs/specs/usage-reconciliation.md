# usage-reconciliation

## Goal

`requests == successes + failures` holds for every Account, cumulatively
and per local day, **by construction rather than by repair**: the same
call writes both sides, so no interleaving, no concurrent attempt, no
crash and no future call site can put them out of step.

This is the drill the counter work never got. It shipped alongside
`usage-trend` after five judge rounds found eight P1s and one P0, each
round's fix causing the next round's defect — the signature of a design
whose invariants were written after its code.

## The rule

**A counter counts outcomes.** One word, one scope: `requests` means the
same thing in `status`, in `accounts`, in `usage`, cumulatively and in a
daily bucket. A request that has started but not finished is not counted
anywhere, and lands in its bucket the moment it settles.

```rust
// record_attempt: no longer counts anything

fn settle(&mut self, success: bool) {
    self.total_requests += 1;
    let day = self.day(&local_today());
    day.requests += 1;
    if success { day.successes += 1 } else { day.failures += 1 }
}
```

The day an outcome books to is the day it **arrived**, not the day its
attempt opened. This is what makes the identity problem disappear rather
than get solved: `settle` no longer needs to know which attempt it is
settling, because it no longer books anything to another day.

## Decisions

- **An outcome books to its own day.** The alternative — attributing it
  to the attempt that produced it — requires attempt identity: either a
  guard consumed by `settle`, or an id on the in-flight entry. Both were
  costed and both were declined. The guard's headline property does not
  even hold here: `AccountManager` sits behind a `tokio::sync::Mutex` and
  `Drop` is sync, so a dropped guard cannot settle directly; it must
  `tokio::spawn`, exactly as `StreamAccounting` does, and cannot settle
  at all during runtime shutdown. Cost accepted: a request spanning local
  midnight counts on the day it finished.
- **Counters count outcomes everywhere**, not attempts cumulatively and
  outcomes per day. Two meanings for one word is the defect the operator
  cut from the panels twice; it does not belong in the data either.
- **Load-time repair stays, as a migration.** A gap can no longer be
  created, so repair now only ever meets files written by the old
  scheme. It fires once, writes back, and finds nothing on every later
  load. Deletable in a later release.
- **A datastore is orthogonal, not a superset.** The unbuilt SQLite +
  Prisma + Next.js design (`keyed-usage`, `usage-console`, `web-console`,
  `workspace-split`) stores one row per *client request* keyed by inbound
  API key. `UsageEvent` has **no account column**: account counters keep
  their shape and their meaning through the whole migration, so building
  the console leaves this bug exactly as it is. Findings:
  `docs/research/usage-reconciliation-datastore.md`.

## Non-goals

- **Attempt identity.** No guard, no id, no per-attempt token. The design
  removes the need for one instead of building it.
- **Counting an attempt lost to a crash.** `SIGKILL` between attempt and
  outcome loses the request from the counters entirely. Accepted: the
  relay counts what it observed, and a number it cannot justify is worse
  than a number it does not have.
- **In-flight visibility.** `status` no longer reads one low while a
  request streams. The gap that showed it was also the gap that hid real
  defects for months; the operator read it as damage three times in one
  session.
- **Multi-process safety.** `persist_usage` stays whole-file
  last-writer-wins. One relay per auth directory, still unenforced.
- **Per-harness attribution**, queued separately.
- **No change to the panels.** `status`, `accounts` and `usage` keep
  their current shape and their plain bytes.

## Acceptance criteria

- AC-1: `AccountState::settle` increments `total_requests` and the daily
  bucket's `requests` in the same call that increments `successes` or
  `failures`. No other code path *on the serving path* writes
  `total_requests` or a bucket's `requests`; the load path writes both,
  as AC-8 requires.
- AC-2: `record_attempt` no longer writes any counter. It is removed, or
  reduced to what remains of it, and `src/app.rs` has no call site that
  counts a request before its outcome.
- AC-3: `AccountState::in_flight` is deleted, with the pop-oldest branch,
  the implied-attempt branch, and the retention clamp that existed only
  to keep a stale in-flight day inside the window.
- AC-4: `settle` takes no attempt and returns the day it wrote, so
  `record_success` books its tokens to the same bucket as its outcome.
- AC-5: For every sequence of recorder calls, cumulative
  `requests == successes + failures` and, for every bucket,
  `requests == successes + failures`. The table test asserts exact
  counts per sequence, not balance alone: balance cannot distinguish
  "counted correctly" from "attempt and outcome both dropped".
- AC-6: Two concurrent outcomes on one account both count, with their
  tokens and their per-model rows. The round-4 P0 stays closed.
- AC-7: A recorder firing twice for one request counts two requests, and
  the balance holds. The seam does not refuse: with nothing to refuse
  against, a double-recording path inflates `requests` rather than
  corrupting the invariant. A caller that must not record twice applies
  its health without an outcome (`record_billing_cooldown`).
- AC-8: `reconcile_loaded_counters` repairs a file written by the old
  scheme, writes the repair back at load, and finds nothing to repair on
  the next load of the file it just wrote.
- AC-9: A Refusal counts a failed request and never touches account
  health: no cooldown, no failure streak, no effect on Rotation.
- AC-10: A cooldown only ever widens. A 24-hour reauth lockout survives a
  paired failure carrying a 2-second backoff.
- AC-11: No test hardcodes a calendar date. Dates come from the clock, so
  no test can go red on a fixed future day.
- AC-12: `ARCHITECTURE.md` states the counting rule, and no comment,
  doc or spec claims the seam refuses a second outcome.

## Verification

```bash
source ~/.cargo/env
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
TMPDIR=/home/kognos/tmp/a/rather/long/temp/prefix cargo test   # long-path safety

# AC-5, by hand, against the live relay:
pengepul accounts        # every account: requests == ok + failed
pengepul usage | cat     # every day:     requests == ok + failed
```

Each acceptance criterion needs a test that fails when its fix is
reverted. Round 5 caught two claims that no test could falsify.
