# A counter counts outcomes, never attempts

`pengepul` counts requests per Account. Until now `record_attempt`
incremented `requests` when a request was dispatched and a separate
recorder incremented `successes` or `failures` when it finished, so the
two sides were written by different calls at different times. Every gap
between them — a lost success under concurrency, a leaked attempt from a
client hanging up, a billing rejection counted twice — was a defect in
the space between those two writes. Five review rounds found eight P1s
and one P0 there, each round's fix causing the next round's defect.

`AccountState::settle` now increments `requests` in the same call that
increments `successes` or `failures`, cumulatively and in the daily
bucket, so `requests == successes + failures` holds by construction
rather than by repair. An outcome books to the day it arrived on, which
is what removes the need for attempt identity: the seam never books to
another day, so it never needs to know which attempt it is settling.

## Considered options

A guard handed out by `record_attempt` and consumed by `settle` was
proposed twice, and would have made a double-settle a compile error. Its
headline property does not hold in this codebase: `AccountManager` sits
behind a `tokio::sync::Mutex` and `Drop` is sync, so a dropped guard
cannot settle directly — it must `tokio::spawn`, exactly as
`StreamAccounting` does, and cannot settle at all during runtime
shutdown. It also had to thread through 24 accounting call sites. An id
on the in-flight entry was the cheaper version of the same idea. Both
solve attribution; counting outcomes removes the question.

## Consequences

A request in flight is counted nowhere. `status` no longer reads one
low while a request streams — the gap that showed in-flight work was
also the gap that hid real defects, and the operator read it as damage
three times in one session.

An attempt lost to a crash or `SIGKILL` between dispatch and outcome is
never counted. The relay counts what it observed: a number it cannot
justify is worse than a number it does not have.

`reconcile_loaded_counters` becomes a migration. A gap can no longer be
created, so it only ever meets files written by the old scheme: it fires
once, writes back, and finds nothing on every later load. It can be
deleted in a later release.
