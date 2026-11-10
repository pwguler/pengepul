# Message threads stay on the vendor

## Goal

An anthropic conversation continues on the vendor as a message thread instead of resending
itself. No caller sees a thread: a harness sends the whole conversation as it always has and
receives the answer it always would. Nothing about a thread can fail a request or hurt a
Pool: it stays on its Account, a turn the vendor refuses is sent again without the caller
knowing, and a 4xx answer never cools an Account.

## Non-goals

- No reproduction of Claude Code's identity: `cch`, the billing-header fields and the
  identifying headers stay as they are. The one beta a thread needs joins the list a request
  already carries.
- No pass-through of a caller's `thread`: a caller that sends one is answered
  `thread_unsupported_request`, and its plain retry is what the relay threads.
- No threading of a Chat Completions or Responses request translated onto anthropic; only a
  Messages request opens or continues a thread.
- No thread state that outlives the process.
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
- AC-5 The first turn of a conversation goes out whole as `thread: {"type": "create"}`, and
  its `anthropic-beta` names `message-threads-2026-08-12` after whatever the request carried:
  the relay's own list, or a Claude Code caller's.
- AC-6 The next turn goes to the same Account as
  `thread: {"type": "continue", "previous_message_id": <reply id>}` with only the messages
  after the reply, no `tools`, one `system` block, and the create's `anthropic-beta`. This
  holds for a whole reply and for a streamed one, and for a continue of a continue.
- AC-7 The one `system` block of a continue is the conversation's billing block: the caller's
  own when it sent one, otherwise the block a create of the same conversation would carry.
- AC-8 A turn continues only when the Account is the thread's, the reply is under 270 seconds
  old, the earlier messages and the reply are repeated, a user message follows the reply, and
  nothing fixed has changed: `model`, `system` without its billing block, `tools`, `thinking`,
  `tool_choice`, `output_config`, `mcp_servers`, `context_management`, `container`, top-level
  `cache_control`, and the beta list. Any other turn goes out as a create.
- AC-9 Messages are compared by what they say: a cache marker that moved, thinking kept or
  dropped, an empty text block, and a string in place of one text block change nothing.
- AC-10 A reply is continued from only when the vendor finished and stored it: it has an id,
  it stopped on `end_turn`, `tool_use`, `stop_sequence`, or `max_tokens` with no tool call cut
  short, and no fallback model wrote it. A streamed reply is read as it passes, unchanged,
  and counts once its `message_stop` has arrived.
- AC-11 A thread belongs to a conversation, its fixed fields and its opening message. A side
  request under the same session neither ends the thread nor continues it, and two
  conversations under one session keep their own threads.
- AC-12 A thread the vendor refuses never reaches the caller. A continue whose thread is
  gone or busy (`thread_not_found`, `thread_already_continued`, `thread_continue_in_flight`)
  is sent again as a create. Any other refused continue (a 4xx but 401, 403 and 429), and a
  create refused for its thread, is sent again with no thread; when the vendor takes that,
  the conversation stays unthreaded. None of it is recorded against the Account.
- AC-13 An error that is the caller's own reaches it unchanged, whole or streamed: at once
  on a first turn, and after one try without a thread on a later one. It does not end
  threading for the conversation and is not counted as a refusal.
- AC-14 Three conversations refused in a row, with no thread taken between them, stop all
  threading for an hour.
- AC-15 A tool call in the reply to a continue comes back under the caller's tool name.
- AC-16 With `cloaking.message-threads: false` no request carries `thread` or its beta.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --no-fail-fast`
- By criterion, in `tests/app.rs` unless another file is named:
  - AC-1, AC-2: `messages_route_refuses_message_threads_before_any_account`
  - AC-3, AC-4: `a_not_found_upstream_answer_does_not_cool_the_account`
  - AC-5, AC-6: `a_conversation_creates_a_thread_then_continues_it`,
    `the_thread_beta_joins_whatever_beta_list_the_request_carries`;
    `apply_cloaking_gives_a_continue_the_billing_block_alone` in `tests/upstream.rs`
  - AC-7: `a_continue_carries_the_billing_block_of_its_whole_conversation`,
    `a_continue_passes_on_the_callers_own_billing_block`;
    `apply_cloaking_keeps_the_billing_block_a_continue_was_given` in `tests/upstream.rs`
  - AC-8: `a_changed_conversation_creates_instead_of_continuing`,
    `a_changed_beta_list_creates_instead_of_continuing`,
    `a_thread_does_not_follow_its_conversation_to_another_account`,
    `a_request_that_adds_no_user_message_after_the_reply_is_not_continued`,
    `a_message_that_only_looks_like_the_reply_is_not_skipped`; `an_idle_thread_creates` in
    `src/threads.rs`
  - AC-9: `a_cache_marker_that_moved_does_not_end_the_thread`,
    `a_reply_repeated_with_or_without_its_thinking_continues`;
    `what_a_harness_is_free_to_change_does_not_end_the_thread` in `src/threads.rs`
  - AC-10: `a_turn_the_vendor_did_not_store_is_not_continued`;
    `only_a_reply_the_vendor_holds_is_stored`,
    `a_streamed_reply_is_the_reply_it_would_be_whole`,
    `a_stream_the_vendor_did_not_finish_or_store_is_no_reply` in `src/threads.rs`
  - AC-11: `two_conversations_on_one_key_never_cross`,
    `a_side_request_under_the_same_session_does_not_end_the_thread`
  - AC-12: `a_create_the_vendor_refuses_is_sent_again_without_a_thread`,
    `a_continue_whose_thread_is_gone_is_replayed_as_a_create`,
    `a_continue_refused_for_what_it_carries_leaves_the_conversation_plain`,
    `a_continue_the_endpoint_cannot_take_leaves_the_conversation_plain`
  - AC-13: `an_error_that_is_the_callers_reaches_it_once`,
    `a_callers_own_error_on_a_later_turn_does_not_end_threading`
  - AC-14: `a_vendor_that_stopped_taking_threads_is_not_asked_for_every_conversation`
  - AC-15: `a_tool_call_on_a_continue_comes_back_under_the_callers_name`
  - AC-16: `threads_off_sends_the_conversation_unthreaded`;
    `message_threads_are_on_until_the_config_turns_them_off` in
    `tests/config_tokens_providers.rs`
  - the non-goal on translated routes: `a_request_translated_onto_anthropic_is_never_threaded`
- Demo without network: `~/handoffs/pengepul-anthropic-cache/demo-isolated-env.sh <binary>`
  starts the relay on its own port with a synthetic Account, and passes when a caller's
  thread request is answered locally with no Upstream call and the same request without
  `thread` makes one.
- Demo against the vendor: `~/handoffs/pengepul-anthropic-cache/live-check.py`. It runs the
  relay in isolation with one Account's access token, drives it with an echo client, pi and
  Claude Code, and judges each turn from outside the relay: Upstream calls made, bytes sent
  to the vendor against bytes the harness resent, and an answer that only the held
  conversation can give.

## Decisions so far

- [Who threads]: the relay. A thread is bound to the Account that created it, and the relay
  is the only party that knows which Account that is.
- [Continue shape]: what Claude Code 2.1.322 sends when it threads on its own: the messages
  after the reply, no `tools`, the billing block as the only `system`, the create's beta set.
- [Beta]: the list a threaded turn carries is worked out once, from the whole request, and
  sent with the create and every continue through `UpstreamRequest.beta`. It is a field, not
  a header, so no caller can set it.
- [Fixed fields]: the vendor's documented list. Sent live, a continue that changed
  `temperature`, `max_tokens`, `stop_sequences` and `stream` was taken, so those stay free.
- [Idle]: 270 seconds against the vendor's five minutes. Sent live, a continue 262 seconds
  after its reply was taken.
- [Refusals]: a continue is the relay's own shape, so every refusal of one is answered from
  the whole conversation; a create is refused for the thread only when the vendor's answer
  names threads. A refusal that is not about a lost or busy thread would repeat on the next
  continue, so it ends threading for that conversation instead of costing two requests a
  turn. A refusal counts only once the vendor has taken the same turn without a thread. The
  rest after three refusals exists because a beta is retired one day.
- [State]: in memory, one entry per thread, 4096 at most, taken on use and dropped on the
  first doubt; a restart costs one create per conversation.
- [Bodies]: a create or a plain request moves the conversation upstream; only a continue
  builds a new, small body. The second attempt after a refusal rebuilds from the caller's
  request.
- [4xx rule]: only 401, 403 and 429 are Account health; every other 4xx is the request's.
- [Why not imitate]: Claude Code computes its billing `cch` in bundled native code that an
  outside client cannot reproduce without forging the vendor's integrity check.
- [Default]: on. A thread changes what the relay sends and never what a caller receives, and
  every refusal ends in the request the relay sent before threads existed.
- [Testing]: tests drive a fake Upstream. The live check runs beside the service, never
  through it: own port, config, HOME and auth dir, one Account's access token read once
  with its refresh token left behind, and a proxy that passes `api.anthropic.com` only.
