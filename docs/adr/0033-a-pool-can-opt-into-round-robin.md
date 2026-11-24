# 33. A Pool can opt into round-robin, which hands new conversations out in turn and reads no Priority

Status: Accepted (amends ADR-0031 and the Rotation entry in CONTEXT.md)

## Context

ADR-0031 replaced the modulo cursor with a Priority order and rejected round-robin among
equals: the chosen Account would depend on history, so identical Pools would answer
differently after a restart. Its own consequence names the cost. With equal Priorities nothing
spreads new work any more, and every new conversation lands on one Account until that Account
cannot serve. The operator asked for a Pool whose Accounts are equal to share new
conversations evenly, while another Pool keeps its Priority order.

## Decision

`rotation:` in `config.yaml` maps a Provider id to `priority`, the default and ADR-0031
unchanged, or to `round-robin`. A round-robin Pool hands each new conversation to the next
serving Account after the one it placed last, in Account-key order, wrapping after the last
key. It reads neither Priority nor the weekly reset. Cache affinity still outranks it
(ADR-0017): a conversation keeps its Account while that Account can serve, and only a
placement takes a turn. A placement is a new conversation, or one whose Account cannot serve
or that Failover leaves. The turn is held per Pool as the key of the last placement, in memory
only, so a restart starts at the first key. `/admin/reload` re-reads `rotation:`.

## Considered options

- **Round-robin inside a Priority tier.** Rejected, as ADR-0031 rejected it. An operator who
  sets Priorities wants an order, and one who wants a spread wants every Account equal. A Pool
  picks one, and nothing mixes them.
- **Spread requests, not conversations.** Rejected by ADR-0017's cost model: moving a running
  conversation re-bills the prefix that its last Account cached.
- **Weights or shares.** Rejected for the reason ADR-0031 gives: the request is an even spread,
  and a share is a second number to keep in step with the first.
- **Persist the cursor.** Rejected: one restart skews the spread by at most one conversation
  per Account, and a fresh start at the first key is the deterministic answer ADR-0031 wanted
  from a relay that has just started.
- **One switch for the whole relay.** Rejected: Priority is already per Pool, and one Pool's
  order (a Max plan beside a Pro one) says nothing about another Pool's (equal static keys).

## Consequences

- ADR-0031's objection holds inside a round-robin Pool, by the operator's choice: which Account
  a new conversation gets depends on what the Pool placed since the relay started. The tests
  assert it from a fresh manager, where the turn starts at the first key.
- The turn is a key, not an index, so a reload that adds an Account or a `disable` that removes
  one does not shift it. The last placement is recorded in both modes, so a Pool switched to
  round-robin continues after the Account that the Priority order placed last.
- `/admin/reload` re-reads `rotation:` from the file the relay loaded. A file that no longer
  parses refuses the reload and changes nothing. `priority:` still needs a restart (ADR-0031).
- `priority:` stays valid for a round-robin Pool and is not read. The admin payload names each
  Pool's rotation, and `pengepul accounts` lists a round-robin Pool's Accounts in turn order
  with no Priority beside them, so no number that decides nothing is printed.
- `rotation:` is a forward-only key, like `priority:`: a build older than this change refuses
  a config that carries it.
