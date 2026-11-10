# Claude Code 2.1.321 alignment

## Goal

Every request pengepul sends to the anthropic Upstream matches what Claude Code 2.1.321
sends for the same conversation, message threads included: the same headers, the same
beta set, the same body shape, and create or continue on the same turns.

## Non-goals

- No change to codex, grok or configured-endpoint traffic.
- No change to what a client receives: response bodies, streams, tool names and errors
  read as they do today. A client never sees a thread error that the relay can recover from.
- No change to Usage counters, quota reading, or the token totals the panels show.
- No pass-through of a client's own `thread`: a request that carries one keeps getting
  `400 thread_unsupported_request`.
- No persistence of thread state across a restart.
- No Classifier-rewrite table changes (`masquerade.rs` keeps its tables).

## Design

**Reference.** `docs/research/claude-code-2.1.321-cloaking-audit.md`, read from the
`@anthropic-ai/claude-code-linux-x64` 2.1.321 binary the same way as the 2.1.280 audit.
It names every beta flag and its gate, the headers, the bundled SDK version, the billing
header, cache-breakpoint placement, and the thread engine's rules. Where this spec and the
audit differ on what Claude Code sends, the audit wins.

**Thread engine.** pengepul is the thread client, as Claude Code is:

- A Harness keeps sending full history. pengepul turns each anthropic-bound request,
  after Translation, masking and Cloaking, into a create or a continue with
  `message-threads-2026-08-12`.
- A continue carries only the new messages, no `tools`, and a `system` holding only the
  billing-header block, as Claude Code 2.1.322 sends it (captured, see the audit). The cache
  breakpoint goes on the last new block.
- A request continues a thread only when that thread's stored transcript (every message
  sent, plus the stored response) is an exact prefix of the request's messages, and its
  fingerprint is unchanged. The fingerprint covers `model`, `system`, `tools`, `thinking`,
  the sorted beta set, `tool_choice` and `effort`.
- A thread is pinned to the Account that created it. A continue goes to that Account only.
  When it cannot serve, the request goes out as a create on the Account that Rotation picks.
- A thread idle past its lifetime, one whose last response did not finish (dropped stream,
  error), or one whose last response the API does not store (`max_tokens` cut in a tool
  call, refusal, fallback model) is not continued.
- Upstream `thread_not_found` or `thread_already_continued`: pengepul replays a create once,
  with the full history it holds. `thread_fingerprint_mismatch`: create.
  `thread_unsupported_request` or an unrecognised 400 on a threaded request: the request is
  resent without `thread`, and that conversation and model stay unthreaded for the process
  lifetime. None of these is recorded as an Account failure.
- `cloaking.message-threads` in `config.yaml` turns the engine off; absent means on.

**State.** In memory, bounded like the affinity map. A restart costs one create per
conversation.

## Acceptance criteria

- AC-1 For a request without `thread`, the anthropic beta set and identifying headers equal
  the 2.1.321 audit's tables for the same model and request shape.
- AC-2 The first anthropic request of a conversation carries
  `"thread": {"type": "create"}` and `message-threads-2026-08-12` in `anthropic-beta`.
- AC-3 The next request of that conversation, whose messages extend the first request's
  messages plus the returned response, goes to the same Account with
  `"thread": {"type": "continue", "previous_message_id": <first response id>}`, no `tools`,
  a `system` holding only the billing-header block, and only the new messages.
- AC-4 AC-3 holds when the first response was streamed, with the id read from `message_start`.
- AC-5 A request whose messages differ from the stored transcript anywhere before its tail,
  or whose fingerprint differs, goes out as a create.
- AC-6 Two conversations that share one conversation key (one session id) each continue their
  own thread.
- AC-7 When the pinned Account is on Cooldown or Disabled, the request goes out as a create
  on another Account, and the client gets a normal response.
- AC-8 An upstream `404 thread_not_found` is answered by one create on the same Account; the
  client receives that create's response, and the Account records no failure.
- AC-9 After `thread_unsupported_request`, the request is resent without `thread` and the
  client receives that response; later requests of that conversation and model carry no
  `thread`.
- AC-10 A request after a response that ended in a dropped stream, `refusal`, or
  `max_tokens` inside a `tool_use` goes out as a create.
- AC-11 A request after the thread's lifetime has passed goes out as a create.
- AC-12 With `cloaking.message-threads: false`, no request carries `thread` or the
  message-threads beta.
- AC-13 Tool names in a continue's response reach the client restored, as on a create.
- AC-14 A client request carrying `thread` gets `400` with
  `error.details.error_code == "thread_unsupported_request"` and reaches no Account.
- AC-15 Usage counters after a threaded turn equal the response's `usage`, as for an
  unthreaded turn.
- AC-16 `docs/research/claude-code-2.1.321-cloaking-audit.md` exists and each table row cites
  the binary's own code.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Demo: record one Claude Code 2.1.321 session's upstream requests and the same conversation
  replayed through pengepul against one local recorder, and diff headers, beta set and body
  shape turn by turn. Then one live threaded session through the relay, with each continue
  confirmed by its `input_tokens`.

## Decisions so far

- [Approach]: pengepul runs its own thread engine for every Harness, not pass-through of client threads.
- [Goal]: the upstream request matches Claude Code 2.1.321; pengepul deviates only where pooling or tool-name masking requires it.
- [Continue body]: what Claude Code sends, read from captured traffic: billing-header `system` block, no `tools`, new messages only. Supersedes the earlier binary read (full `system` and `tools`).
- [Off switch]: `cloaking.message-threads`, default on.
- [Client threads]: refused with `thread_unsupported_request` (commit `8986eb88`).
- [State]: memory only.
- [Order]: the 2.1.321 audit lands before the thread engine.
