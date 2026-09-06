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
  passed over on the next turn, not waited for; **Failover** is untouched and still
  moves a rejected request to another account of the same Provider.
- The map is bounded (1024 conversations, cleared wholesale when full) and is not
  persisted. A restart re-learns the mapping on the next turn, at the cost of one
  cold read — the same price a restart already pays for Cooldown.
- A pool of one is unaffected: modulo-1 rotation returned the same account before
  and returns it now.
- Repairing a dead key is now safe. Before this, adding a healthy second account to
  a pool would have halved its cache hit rate.
