# cache-miss-attribution

## Goal

Say **which writer re-billed a prefix**, from the relay's own log, without a
capture proxy. A prompt-cache miss is billed by the upstream and visible only as a
`cached_tokens` that fell while the prompt grew, so the relay prints the two values
that turn that into an attribution:

- `"cacheable prefix"` — one line per client request: the request's cacheable
  material as a hash, beside the turn count it carried.
- `"upstream usage"` — one line per recorded success: what the upstream billed that
  request, including its cache read.

An unchanged fingerprint beside a collapsed cache read puts the writer upstream. A
fingerprint that moved names the relay or the harness. The `cached_tokens` a client
sees cannot tell those apart, and on a pool of one there is no account switch to blame
in the first place: measured on the live relay, a 97K prefix was billed 85K cached and
re-billed whole 37 seconds later, through the one grok account, with `affinity="honored"`
on both requests.

## Shape

- `cacheable_prefix` is the material both lines and Rotation key on: `system`,
  `tools`, `model`, and the opening window. `conversation_key`'s fallback and
  `cacheable_prefix_fingerprint` call it, so the log cannot print one hash while
  Rotation keys on another.
- The fingerprint is sha256 in hex. It holds across a conversation's appended turns
  and moves when the opening, `system`, `tools` or `model` moves.
- **The window is a fixed opening** — the first `AFFINITY_OPENING_MESSAGES` messages,
  `AFFINITY_MESSAGE_BYTES` each. A rewritten *tail* is invisible to it. That is the
  stated limit, and the direction of the claim follows from it: this line can rule
  the relay and the harness out, and can never rule them in.
- `messages` is how many turns the dialect's list carried — `messages` for Chat and
  Messages, `input` with `messages` as the fallback for Responses, matching the field
  the route itself reads. It separates a repeated fingerprint from a grown request
  apart from a shortened one.
- Both lines are DEBUG, and `tracing` evaluates field expressions lazily, so
  `debug: off` (the default) builds neither the hash nor the count.
- `"upstream usage"` prints the **normalized** counts the panels use: `input`
  excludes the cache read for every Provider (ADR-0023). It is silent when a success
  carries no usage at all — count-tokens, or a 2xx whose usage would not parse —
  because a line of zeroes would claim a measurement nobody made.
- The lines are separate events: the prefix line names the conversation, the usage line
  names the account, and neither carries the other's field. They pair by provider and
  time, and — a conversation has one request in flight at a time — by the account that
  served it. Putting the conversation on the usage line would mean threading it through
  every route to serve a log line.
- `"account selected"` stays byte-identical. These are new messages beside it.

## Acceptance criteria

- AC-1: `cacheable_prefix_fingerprint` equals the hash `conversation_key` uses for a
  conversation that names no session, so the two cannot drift.
- AC-2: the fingerprint is unchanged when the message tail is appended **or**
  rewritten, and changes when `system`, `tools`, `model`, or the opening window
  changes.
- AC-3: `message_count` reads `messages` for Chat and Messages, and `input` with
  `messages` as the fallback for Responses.
- AC-4: the prefix line carries `provider`, `route`, `model`, `conversation`,
  `messages` and `prefix`, and `prefix` is the fingerprint of the body the request
  was handed.
- AC-5: the usage line carries the upstream's `input`, `cache_read`, `cache_write` and
  `output` after normalization.
- AC-6: no event is emitted for either line while the level is off. Read from the
  macro's lazy field evaluation, not asserted by a test — a test cannot observe the
  absence of the work, only the absence of the line.

## Non-goals

- No per-request history, no new counter, no change to `usage.json` or the admin
  payload.
- No claim about the tail. Closing that blind spot needs per-conversation state in
  the relay, and a fingerprint that reads the growing tail is the bug ADR-0017
  already fixed once.
- No change to Rotation, affinity, or which account serves a request. The fingerprint is
  computed from the body the request already carries; nothing on the wire moves.

## Verification

```
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
```

AC-1 and AC-3 are `the_prefix_log_hashes_what_the_fallback_key_hashes` and
`the_prefix_log_counts_the_turns_each_dialect_carries`. AC-2 is
`the_prefix_fingerprint_holds_while_the_tail_grows_and_moves_with_the_opening`. AC-4
and AC-5 are `the_request_log_carries_the_cacheable_prefix_and_the_cache_read`, which
drives two real requests through `route_provider_request` and reads the emitted
fields. AC-4 and AC-5 are verified by mutation: a constant prefix of the right shape,
a constant turn count, a constant route, and a swap of `input` with `cache_read` each
fail that test.
