# 18. The injected prefix never carries a shorter TTL than the client's

Status: Accepted

## Context

`apply_cloaking` prepends two system blocks — the billing header and a
`You are Claude Code, Anthropic's official CLI for Claude.` prefix — and the
prefix carries `cache_control: ephemeral`, matching what real Claude Code sends
(ADR-0006).

Anthropic renders `tools`, then `system`, then `messages`, and **refuses** a 1h
breakpoint that appears after a 5m one. That is a 400, not a degradation to the
shorter retention. The injected prefix is the first marked block in `system` by
construction, so it precedes every breakpoint the client sets.

The prefix hardcoded `{"type": "ephemeral"}` — no `ttl`, so the 5m default. A
client marking any breakpoint `ttl: "1h"` therefore put a 1h block behind our 5m
one and was answered 400 on **every** request. The failure was total rather than
partial, which is why it read as "1h retention is not available on this
subscription" rather than as a relay defect.

It is available. Measured through the live relay on the OAuth subscription:
`ephemeral_1h_input_tokens: 10416` on the write, `7216` read back on the
following turn. The 1h TTL is GA and needs no beta header.

## Decision

`requests_long_retention(body)` scans everywhere Anthropic counts a breakpoint —
`tools`, `system`, and each message together with its `content` array — for
`cache_control.ttl == "1h"`. When one is found and the prefix still carries a
`cache_control`, the prefix is lifted to `{"type": "ephemeral", "ttl": "1h"}`.

The relay never chooses a retention; it matches one. `pengepul` holds no opinion
on whether 1h is worth it for a given workload. It guarantees only that a block
**it** injected cannot invalidate a choice the client already made.

Rejected: dropping `cache_control` from the prefix whenever the client asks for
1h. It avoids the 400 with less code, but discards the prefix's own cache segment
on exactly the requests that care most about caching. Lifting keeps it.

Rejected: moving the prefix after the client's blocks. It has to precede the
client's content to cloak it.

## Consequences

- The scan covers `messages`, not only `system`. A client that marks 1h solely on
  message content still lifts the prefix — the arm that
  `a_long_ttl_marked_only_in_messages_still_lifts_the_prefix` exists to keep
  alive, since a test marking only `system` leaves it dead.
  `the_injected_prefix_never_precedes_a_longer_client_ttl` pins the lift itself.
- Composes with ADR-0006 in that order: the lift runs first, then
  `cap_cache_control` drops the **earliest** marked block. A client already at the
  four-breakpoint budget loses the prefix's marker entirely, which makes its TTL
  moot for that request and leaves the request valid either way.
- Any future injected block inherits the rule. A second prepended block carrying
  `cache_control` reintroduces the 400 unless it is lifted the same way.
- **Long retention is not free, and the relay is the wrong place to weigh it.** A
  1h write costs 2× base input against 1.25× for 5m, so it pays when turns are
  minutes apart and loses in a loop that re-reads inside five minutes. The choice
  belongs to the client — for `pi`, `PI_CACHE_RETENTION` together with
  `compat.supportsLongCacheRetention` in the provider extension. Turned on blindly
  for a fast-loop workload it raises cache writes while the hit rate stays flat.
