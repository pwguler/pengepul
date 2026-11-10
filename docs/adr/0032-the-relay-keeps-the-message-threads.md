# 32. The relay keeps the message threads

Status: Accepted (adds Thread, Plain and Declined thread to CONTEXT.md, and the Thread ledger
to ARCHITECTURE.md)

## Context

Anthropic's Message threads let a client send only the messages after the last reply: the
vendor keeps the conversation under the id of that reply, for five minutes after its last
request. A thread is bound to the account that opened it, and any other account is answered
`thread_not_found`.

Behind the relay, the account is not the client's to choose. **Rotation** picks it, and
**Failover** moves a request to another one in the middle of a conversation. A client's own
thread also met **Cloaking** on its way: a continue carries no `system` for it to rebuild and
no `tools` for the tool-name map, and a client that is not Claude Code has its
`anthropic-beta` dropped, the thread's own beta with it. Claude Code and pi turn threads off
by themselves when they are pointed at a relay.

The relay, meanwhile, receives the whole conversation on every turn and knows which Account
served the last one.

## Decision

**The relay is the thread client.** A Harness sends the whole conversation on every turn.
The relay opens a **Thread** on the first turn and continues it on a later one when it
can show the vendor holds what came before: the same Account, a stored reply the client
repeats, the same earlier messages, and nothing changed that the vendor holds fixed. A
client that names a `thread` of its own is answered with the vendor's
`thread_unsupported_request` before any Account is chosen, which the vendor's SDKs answer
by sending the conversation whole.

**A Declined thread is decided by status, never by the words of the error.** Any 4xx to a
threaded turn but 401 and 429 is answered by sending the same turn again without that
Thread: as a new one when the old is gone or busy, **Plain** otherwise. The answer to the
last attempt is the one that is recorded and returned. What the vendor declines is held
against the Account it declined on, and against no other. A vendor fault on a continue,
other than a 529, is sent again Plain as well, and is held against nothing.

**The Relay hands the Upstream client a fact, not a body.** The conversation crosses
`UpstreamClient` whole beside how much of it the vendor holds, and the Upstream client
writes what leaves: the beta set, the billing-header block and the shape of a continue are
vendor identity.

## Considered options

- **Pass a client's thread through, and pin its conversation to the Account.** Rejected:
  Failover breaks the pin and the client must send everything again, and the client's
  continue has nothing for Cloaking or the tool-name map to work from. Every Harness would
  carry the relay's Account choice in its own state.
- **Answer `thread_unsupported_request` and stop there.** Rejected: it keeps every client
  working and saves nothing, and the relay is the one party that holds both the conversation
  and the Account.
- **Read the vendor's error to tell the thread's fault from the client's.** Rejected: the
  first live request was declined in words that name no code, and words change. A wording
  the relay did not expect would reach a client as an error it did not cause, or cool each
  Account in turn.
- **Send every declined continue again as a new thread.** Rejected: a continue declined for
  what it carries is declined again on the next turn, two requests a turn for the life of
  the conversation. Only a thread that is gone or busy opens another.
- **Persist threads across a restart.** Rejected: the vendor keeps one for five minutes.

## Consequences

- A client's own 4xx on a threaded turn costs two Upstream requests, the threaded one and
  the plain one. The vendor bills neither. A vendor fault on a continue costs the same two
  before it fails over, and an overloaded vendor (529) is never sent the second.
- Tokens are billed as for a whole request: the vendor reads the whole conversation and
  counts the held part as a cache read. What shrinks is the request body.
- A Thread pays inside a Harness's tool loop. A turn that arrives more than 270 seconds
  after the last reply is sent whole.
- A Harness that rewrites its earlier messages between turns never continues, and every
  turn of it is sent whole. A debug line names the reason when it is the messages, the
  Account or the idle time. A changed `system`, tool list, fixed field or opening message
  makes the turn a conversation the ledger has not seen, and the line says only `create`.
- A Pool can hold an Account the vendor takes no thread for. Three conversations in a row
  that go Plain there rest that Account for an hour, and the other Accounts are still asked.
- `cloaking.message-threads: false` sends every request Plain.
