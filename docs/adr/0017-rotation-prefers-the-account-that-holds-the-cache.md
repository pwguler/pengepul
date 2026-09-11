# 17. Rotation prefers the account that already holds the prefix

Status: Accepted (amends the Rotation entry in CONTEXT.md)

## Context

Every upstream **Account** holds its own prompt cache. **Rotation** hands each
request to the account after the one used last and, as CONTEXT.md put it, holds
"no affinity to a client or session". Those two facts fight: consecutive turns of
one conversation land on different accounts, and each turn re-reads a prefix the
other account has already paid to cache.

Measured on the live relay before changing anything:

| Pool | Accounts | Cache share of input tokens |
|---|---|---|
| anthropic | 1 | 99.8% |
| commandcode | 2 | 45.6% |

The single-account pool cannot rotate and caches almost perfectly. That is not
proof on its own — the two pools run different workloads — but the mechanism is
not in doubt, and the topology was hiding it: of the two commandcode keys, one
had 58 requests and 0 successes, so every *successful* request was landing on the
same key anyway. Repairing that key would have split the traffic and dropped the
hit rate, making a fix look like a regression.

A separate measurement rules out the alternative explanation. The same 13.9K-token
body sent twice, two seconds apart, returned `cached=13888/13919` — 99.8%. Adding
`prompt_cache_key`, `user`, or `store=true` changed nothing. Nothing about the
request shape is wrong; only *which account receives it* is.

## Decision

`AccountManager::account_for(affinity)` returns the account that last served that
affinity key when it is off **Cooldown**, and otherwise falls through to
`next_account_result()` — plain Rotation — recording whatever it picks.

The affinity key is the session the harness names (`x-claude-code-session-id`),
and otherwise a hash of the request's cacheable prefix: `system`, `tools`, and
`model`. Turns of one conversation repeat that prefix byte for byte — verified by
capturing consecutive requests from a real pi session, whose 15,608-byte developer
message and 42,983-byte tool list were identical between turns and across sessions
in the same project.

Rejected: keying on the client credential. It is stable and trivially available,
and it makes every request from one API key a single conversation — which does not
preserve a cache so much as switch Rotation off for that client. The prefix hash
groups what actually shares a cache and still spreads unrelated work.

## Consequences

- **Two identical requests no longer alternate accounts.** They share a prefix, so
  they share an account. `messages_route_rotates_available_anthropic_accounts` now
  sends two different system prompts to assert the pool is still spread across.
- **Availability still outranks cache locality.** A pinned account on Cooldown is
  passed over at the next selection, not waited for.
- **Failover re-pins, and that is what heals a bad pin.** Its loop re-enters
  `account_for` on every attempt instead of reusing the account it already holds,
  so the fall-through that rescues a rejected request also records the rescuer as
  that conversation's account. A conversation therefore migrates inside the request
  that hit the bad account, not a turn later, and stays there once the first one
  recovers. Reusing the already-fetched account to save a lock would look like a
  harmless optimisation and would instead steer every later turn back into the same
  rejection; `a_conversation_that_failed_over_stays_on_the_account_that_rescued_it`
  is the test that fails when it does.
- The map is bounded (1024 conversations, cleared wholesale when full) and is not
  persisted. A restart re-learns the mapping on the next turn, at the cost of one
  cold read — the same price a restart already pays for Cooldown.
- Widening the fallback to the Messages dialect costs each live Claude session one
  cold read, once. A Claude conversation is not therefore split across accounts
  afterwards, which is the failure this amendment exists to stop.
- A pool of one is unaffected: modulo-1 rotation returned the same account before
  and returns it now.
- Repairing a dead key is now safe. Before this, adding a healthy second account to
  a pool would have halved its cache hit rate.

## Amendment: the affinity key reads every dialect's opening

Measured on the live relay, `commandcode` split one deepseek conversation across
two of its three accounts (656 requests on one, 96 on another), and a turn sent
four seconds after the previous one re-billed its whole 17,920-token prefix
(`cached=1920/22651`). A four-second gap cannot be a TTL expiry. The upstream
cache is per account, so the request had been served by a different account.

`conversation_key` could not tell conversations apart in the OpenAI Chat
Completions dialect. It read only two session headers, which pi did not send for
an `openai` affinity format at the time of this measurement — its client emitted
`session_id`, `x-client-request-id` and `x-session-affinity` instead, none of
which this relay reads. (The sibling provider has since pinned `x-session-id`;
see the amendment below for what that does and does not change.) Otherwise the key
hashed `system`, `tools` and `model` — but a Chat Completions body has no
top-level `system`, so the key collapsed to the model and tool list. Every
conversation on that model shared one affinity entry, so one account's Cooldown or
one failover re-pinned all of them at once. `prompt_cache_key` reached the
upstream unchanged but was never read for affinity.

The key now resolves in this order:

1. `x-claude-code-session-id`, then `x-session-id`;
2. `body.prompt_cache_key`, the standard `OpenAI` field for exactly this and the
   only affinity signal an OpenAI-dialect client can send;
3. a hash of the cacheable prefix: `system`, `tools` and `model`, plus a fixed
   window over the opening of the message list — the first two messages, each cut
   to 4 KiB (`messages` for Chat and Messages, `instructions` then `input` for
   Responses), canonicalized.

The window is a fixed *count* of leading messages, not a growing slice of the list,
and that is the whole design. Appends land at the end, so a window over a fixed
prefix keeps the same key turn after turn, while two conversations that differ
anywhere inside the window never collide. The first cut of this change instead read
the list up to an 8 KiB budget, which is wrong twice over, and both ways were
measured before the fix landed:

- A conversation shorter than the budget re-keyed itself on **every** turn, because
the budget still reached into the growing tail. A re-keyed conversation is handed to
whatever Rotation picks, so the conversation this change exists to protect kept
alternating accounts.
- Worse, the budget filled up *inside* pi's developer message, which is roughly 15 KB.
  The hash then never reached the first user turn — the one thing that separates two
  conversations in a project — so two conversations sharing that developer message
  hashed **identically**. That is the original collapse, unfixed, for all Messages
  traffic and for Chat traffic with no `prompt_cache_key`.

Budgets are therefore per message, so a large shared opening cannot crowd out the
turn after it. Two messages is the smallest window that can satisfy both
requirements on a first turn, and **no window can satisfy both on a single-message
first turn**: with only message 0 in hand, "this conversation grows" and "this is a
different conversation" are the same observation. A conversation that opens with one
message settles on its second turn; one that opens with two or more — every pi
request, which always carries a developer or system message — never moves. The
earlier draft of this ADR justified the instability as "nothing shorter than the
minimum cacheable unit has a prefix worth preserving"; that claim was unverifiable
from inside this repository and is withdrawn in favour of the argument above.

The Messages dialect needed rule 3 as much as Chat did, which is not obvious from
its body: it has a real top-level `system`, so the old key was not empty — it just
could not tell two conversations in one project apart when they shared a system
prompt and a tool list. What made that live rather than latent is that pi names no
session on that wire at all. Measured against a mock relay through the provider's
production model configs (`pi-pengepul-provider`, `test/affinity-wire.test.ts`): an
`openai-completions` request carries the pinned `x-session-id`, and carries
`prompt_cache_key` when cache retention resolves to long; an `anthropic-messages`
request carries no `prompt_cache_key` and hardcodes `x-session-affinity`, which this
relay does not read. Messages traffic therefore rested entirely on rule 3.

The Chat pin is new, and that is what reconciles the two measurements. The
provider landed it in `pi-pengepul-provider` `ecf9fcc` (2026-09-12, released in
v0.2.2), the same hour this relay's fix was written. Before it, rule 1 matched
nothing on the Chat wire and rule 2 matched only under long cache retention; after
it, rule 1 covers every pi Chat request and rules 2-3 serve other clients. Both
readings are in this ADR on purpose: the Context section describes the relay as it
was when the miss was measured, and this section describes it now. Neither rule is
dead code — rule 3 is still the only signal for Messages, and rules 2-3 still cover
any OpenAI-dialect client that pins no header.

This is a deliberate override of the "no behaviour change for Anthropic Messages
bodies" constraint the change was originally scoped to, taken with the operator's
explicit approval once the measurement above showed the constraint was protecting the
collapse rather than a working key. The cost is one cold read per live Claude
conversation, once.

Every selection logs the resolved key, the chosen account and whether the affinity
entry was honored or fell through to Rotation, so a re-billed prefix can be tied
to the account switch that caused it. The `honored`/`rotation` value that line prints
is unit-tested through `AccountResult`; the log format itself was verified by
running the relay with `RUST_LOG=pengepul=debug` and reading its output, and is not
covered by an automated test.

## Verification

Re-run on the live relay after the change, with the request shape and the idle gap
taken from the miss above — one `prompt_cache_key` on every turn, four seconds of
idle after the first:

| turn | prompt | cached | re-billed | hit |
|---|---|---|---|---|
| 1 | 7394 | 0 | 7394 | 0% |
| 2 | 7404 | 7168 | 226 | 96% |
| 3 | 7414 | 7168 | 246 | 96% |
| 4 | 7424 | 7168 | 256 | 96% |

The four-second gap that used to collapse the prefix from 17,920 to 1,920 now costs
nothing: every turn after the first re-bills only its own increment. Re-billed is
flat at ~240 tokens per turn instead of the whole prefix.

The account-switch case is covered by
`one_chat_conversation_failing_over_does_not_re_bill_another`, which runs against an
upstream whose cache is per Account — the property that makes the switch expensive —
and asserts that a failure on one conversation's Account leaves a sibling
conversation's prefix cached. Removing either the `prompt_cache_key` rule or the
message opening from `conversation_key` makes it fail.
