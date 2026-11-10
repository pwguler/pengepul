# Message threads stay on the vendor

## Goal

An anthropic conversation continues on the vendor as a message thread instead of resending
itself, and nothing about a thread can hurt a Pool: it stays on its Account, a thread the
vendor lost costs one request, and a 4xx answer never cools an Account.

## Non-goals

- No reproduction of Claude Code's identity: `cch`, the billing-header fields, the beta set or
  the identifying headers stay as they are.
- No pass-through of a caller's `thread`: a caller that sends one is answered
  `thread_unsupported_request`, and its plain retry is what the relay threads.
- No threading of a Chat Completions or Responses request translated onto anthropic; only a
  Messages request opens or continues a thread.
- No change to codex, grok or configured-endpoint traffic.
- No change to Usage counters, quota reading, or the panels.

## Acceptance criteria

- AC-1 A Messages request carrying `thread` is answered `400` with
  `error.details.error_code == "thread_unsupported_request"`, reaches no Account, and produces
  no Upstream call.
- AC-2 A caller that resends the same request without `thread` is served as any other request.
- AC-3 An Upstream 4xx other than 401, 403 and 429 is recorded as a refusal: no Cooldown, no
  failure streak, and the counters still balance against total requests.
- AC-4 A 401, 403 or 429 still costs the Account health and still fails over.
- AC-5 The first turn of an anthropic conversation goes out as `thread: {"type": "create"}`.
- AC-6 The next turn, whose messages extend the first by the reply and a new message, goes to
  the same Account as `thread: {"type": "continue", "previous_message_id": <reply id>}`, with
  only the new messages and no `tools`.
- AC-7 AC-6 holds when the first turn was answered as a stream, with the id read from
  `message_start` while the stream passed through unchanged.
- AC-8 A rewritten earlier message, a changed fixed field, an idle thread, another Account or
  a reply already continued all go out as a create.
- AC-13 A caller that changes its `system`, its tool list, or its copy of the reply goes out as
  a create, so a new instruction is never swallowed and an edited reply is never served from
  the vendor's copy. Holds for a streamed reply too, which is rebuilt from its own events for
  the comparison.
- AC-9 A continue carries the billing block alone as its `system`, because a thread may not
  change `system` and the create's blocks are already inside it.
- AC-10 An Upstream thread rejection is answered by one create from the relay's copy: as a new
  thread, or unthreaded when the endpoint has no threads, which that conversation then keeps.
- AC-11 A reply the vendor does not store (refusal, a tool call cut by `max_tokens`, a fallback
  model's answer, no id) records no thread, so the next turn creates.
- AC-12 With `cloaking.message-threads: false` no request carries `thread`.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Demo: `~/handoffs/pengepul-anthropic-cache/demo-isolated-env.sh <binary>`. It starts the
  relay on its own port with a synthetic Account and no network, and shows the thread request
  answered locally with zero Upstream calls, the same request without `thread` reaching the
  Upstream, and no Cooldown anywhere.
- Thread behaviour itself is proved by the tests, which drive a fake Upstream:
  `a_conversation_creates_a_thread_then_continues_it`,
  `a_streamed_reply_names_the_thread_the_next_turn_continues`,
  `a_thread_the_vendor_lost_is_replayed_as_a_create`, `threads_off_sends_the_conversation_unthreaded`,
  `apply_cloaking_gives_a_continue_the_billing_block_alone`,
  `a_changed_system_prompt_creates_instead_of_continuing`,
  `a_changed_tool_list_creates_instead_of_continuing`,
  `an_edited_reply_creates_instead_of_continuing`,
  `an_edited_streamed_reply_creates_instead_of_continuing`,
  `a_tool_use_reply_continues_with_the_name_the_caller_holds`,
  `two_conversations_on_one_key_never_cross`, `a_tool_call_cut_by_max_tokens_creates_the_next_turn`,
  and the `threads` unit tests.

## Decisions so far

- [Approach]: compatibility only. A thread request is refused with the vendor's own code, which
  the SDK thread helper, the SDK tool runner and pi already fall back on (commit `8986eb88`).
- [Why not support threads]: a thread lives on the Account that created it, and the relay's
  Cloaking rebuilds `system` while the tool-name map comes from `tools`, so a continuation
  cannot cross the relay intact.
- [Why not imitate]: Claude Code 2.1.322 computes its billing `cch` in bundled native code that
  an outside client cannot reproduce without forging the vendor's integrity check.
- [4xx rule]: only 401, 403 and 429 are Account health; every other 4xx is the request's.
- [Continue shape]: new messages only, no `tools`, billing-block `system`, the create's beta set
  repeated through the relay's own `x-pengepul-thread-beta` header, which never goes upstream.
- [State]: in memory, bounded at 4096 conversations, dropped on the first doubt rather than
  repaired; a restart costs one create per conversation.
- [Testing]: isolated env only — own port, own config and auth dir, synthetic Account, no
  network. The running service is never used.
