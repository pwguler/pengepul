# 19. Spend the injected breakpoint inside the conversation

Status: Accepted (amends ADR-0006)

## Context

Anthropic accepts four `cache_control` breakpoints and caches the prefix ending
at each one. A client that marks its system prompt, its last tool, and its newest
message therefore leaves the cache holding two entries — `system + tools`, and
`system + tools + every message` — with **nothing between them**. Any failure to
match the tail entry falls all the way back to `system + tools` and re-reads the
whole conversation.

That is not hypothetical. Measured on a 68-hour `pi` session of ~460K tokens,
three separate turns re-read the history:

| turn | gap since previous request | cache read | cache write |
|---|---|---|---|
| 16:33:03 | 34.2m | 27,901 | 426,175 |
| 16:37:23 | 2.9m | 27,901 | 431,422 |
| 16:39:37 | 1.9m | 27,901 | 433,406 |

`cache read` is 27,901 every time, identical to the token, while the conversation
grew from 426K to 433K. That constant is `system + tools` surviving and everything
after it being lost, because no intermediate entry exists to land on.

Captured request bodies confirm the client's side: `pi` sends exactly three
breakpoints on the API-key path — `system[0]`, `tools[last]`, `messages[last]` —
and four on the OAuth path, where an identity block takes the spare. So on the
wire through this relay the total is already four, and the client has no room to
add one itself.

`apply_cloaking` spends one of those four on the injected `You are Claude Code`
prefix. ADR-0006 already describes that marker as "a ten-token static line whose
content is still cached under the client's next breakpoint, so the loss is
negligible" — and already drops it whenever a client spends all four itself.

## Decision

`reallocate_prefix_breakpoint` moves that marker off the prefix and onto a stable
point inside `messages`, whenever the client left room (total ≤ 4).

The checkpoint anchors at `floor(tail / CHECKPOINT_STRIDE) * CHECKPOINT_STRIDE`
with a stride of 20, stepping back a **whole stride** when that lands on the tail.
Stepping back one message instead would let the anchor follow the tail, which
writes an entry every turn and reads it never.

A client already spending four keeps ADR-0006 behaviour exactly: nothing is
reallocated, and the cap drops our prefix marker as before.

Rejected: adding a fifth breakpoint. Five is a hard 400, and the OAuth path of at
least one client is already at four.

Rejected: leaving this to the client. The relay is the only party that knows the
true total, because it is the one adding a breakpoint the client cannot see.

## Consequences

- **The injected prefix goes out unmarked more often.** This is not a new shape on
  the wire: ADR-0006 already ships it whenever a client is at budget, and ADR-0014
  states cloaking follows Claude Code except where fidelity costs the client.
- A tail miss now falls back to the checkpoint instead of to `system + tools`. It
  bounds the cost of a miss; it does not prevent one.
- **The misses in the table were not the relay's, and not time.** They were traced
  after this landed: the `pi-goal-list-loop-audit` extension inserts an ephemeral
  checkpoint message early in the conversation and, in npm `0.38.22`, fills it with
  live counters (`iteration`, `tokensUsed`, `lastIterationCompletedAt`), so every
  iteration rewrote the prefix at that point. That is why `cache read` sat at
  27,901 regardless of gap. Upstream fixed it in `451de16e` by making the checkpoint
  byte-stable; the fix was unpublished and was installed from a git ref. This ADR's
  checkpoint would not have prevented those misses either — the rewrite happened
  *before* any anchor — which is the point of recording the cause here: a floor that
  never moves with time is a prefix that changes, not a cache that expires.
- The checkpoint re-anchors once per stride, costing one extra cache write every
  20 messages.
- Conversations shorter than a stride get no checkpoint: the anchor would land at
  or before the first message, which caches nothing the `system + tools` entry does
  not already hold.
- Every client benefits, not only the one that prompted it. A relay serving several
  harnesses cannot ask each of them to leave a slot free.
