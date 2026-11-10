# Message threads stay on the vendor

## Goal

An anthropic conversation continues on the vendor as a message thread instead of resending
itself. No caller sees a thread: a harness sends the whole conversation on every turn and
receives the answer a plain request would get. Nothing about a thread can fail a request or
hurt a Pool: it stays on its Account, and a turn the vendor declines as a thread is sent
again without the caller knowing.

## Non-goals

- No reproduction of Claude Code's identity: `cch`, the billing-header fields and the
  identifying headers stay as they are. The one beta a thread needs joins the beta set a
  request already carries.
- No pass-through of a caller's `thread`: a caller that sends one is answered
  `thread_unsupported_request`, and the whole conversation it sends next is what the relay
  threads.
- No threading of a Chat Completions or Responses request translated onto anthropic; only a
  Messages request opens or continues a thread.
- No thread state that outlives the process.
- No change to what codex, grok or a configured endpoint is sent, or to how any Provider's
  answers are counted.
- No change to Usage counters, quota reading, or the panels.

## Acceptance criteria

- AC-1 A Messages request bound for the anthropic Upstream that names a `thread` is answered
  `400` with `error.details.error_code == "thread_unsupported_request"`. No Account is
  chosen, nothing is sent upstream, and nothing is recorded.
- AC-2 The same request without `thread`, or with `"thread": null`, is served as any other
  request. A request bound for another Provider is served whether it names a `thread` or
  not.
- AC-3 The first turn of a conversation goes out whole as `thread: {"type": "create"}`, and
  its `anthropic-beta` names `message-threads-2026-08-12` after the beta set the request
  carries anyway: the relay's own, or a Claude Code caller's.
- AC-4 The next turn goes to the same Account as
  `thread: {"type": "continue", "previous_message_id": <reply id>}` with only the messages
  after the reply, no `tools`, one `system` block, and the beta set its whole conversation
  earns, which is its create's. This holds for a whole reply and for a streamed one, and
  for a continue of a continue.
- AC-5 The one `system` block of a continue is the conversation's billing-header block: the
  caller's own when it sent one, otherwise the block a create of the same conversation
  carries. A billing-header block is the first `system` block whose text starts with the
  header; a prompt that mentions the header further on, and a later block that starts with
  it, are `system`.
- AC-6 A turn continues only when the Account is the thread's, the reply is under 270
  seconds old, the earlier messages and the reply are repeated, a user message follows the
  reply, and nothing fixed has changed: `model`, `system`, `tools`, `thinking`,
  `tool_choice`, `output_config`, `mcp_servers`, `context_management`, `container`,
  top-level `cache_control`, and the beta set. `system` is compared without its
  billing-header block when that block is the header and its `name=value;` fields alone,
  which Claude Code writes anew for every request; one that goes on after them is compared
  like any other block. Any other turn goes out as a create.
- AC-7 Messages are compared by what they say: a cache marker that moved, thinking kept or
  dropped, an empty text block, and a string in place of one text block change nothing.
- AC-8 A reply is continued from only when the vendor finished and stored it: it has an id,
  it stopped on `end_turn`, `tool_use`, `stop_sequence`, or `max_tokens` with no tool call
  cut short, and no other model wrote it. A streamed reply is read as it passes, in pieces
  of any size, and counts once its `message_stop` has arrived.
- AC-9 A thread belongs to a conversation: the session its harness names, its fixed fields
  and its opening message. A side request in the same session neither ends the thread nor
  continues it, two conversations in one session keep their own threads, and a caller that
  names no session continues from its second turn.
- AC-10 A thread the vendor declines never reaches the caller. A create answered with a 4xx
  other than 401 and 429 is sent again plain. A continue answered so is sent again as a
  create when its thread is gone or busy (`thread_not_found`, `thread_already_continued`,
  `thread_continue_in_flight`), and plain otherwise. One request records one outcome,
  whatever was sent for it.
- AC-11 An error that is the caller's own reaches it unchanged, after one plain try. It
  ends threading for no conversation, marks none plain and counts toward no rest.
- AC-12 A 401 or a 429 to a threaded turn is the Account's: it costs the Account its health
  and fails over as for any request, and the turn is not sent again to that Account. So is
  a 529, and a vendor fault on a create. A vendor fault other than 529 on a continue is
  sent again plain, and is held against nothing: the next turn is offered as a thread.
- AC-13 A conversation the vendor took plain after declining its thread stays plain on that
  Account. Three in a row on one Account, with no thread taken there between them, rest that
  Account for an hour: it is not asked for a thread, and another Account still is. A reply
  that was on its way when a rest began does not end it. The ledger holds at most 4096
  threads and 4096 plain conversations; one that is full is emptied, and the conversations
  in it start over.
- AC-14 A tool call in the reply to a continue comes back under the caller's tool name.
- AC-15 With threads and without them, a caller receives the same status, headers and body.
- AC-16 With `cloaking.message-threads: false` every request goes out plain. The relay
  writes the key into a config only when an operator set it.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --no-fail-fast`
- By criterion, in `tests/app.rs` unless another file is named:
  - AC-1, AC-2: `a_callers_own_thread_is_answered_before_any_account`,
    `a_thread_field_bound_for_another_provider_is_left_alone`
  - AC-3: `a_create_is_the_plain_request_with_a_thread_and_its_beta`,
    `the_thread_beta_joins_the_set_the_request_already_carries` in `tests/upstream.rs`
  - AC-4: `a_conversation_opens_a_thread_then_continues_it`;
    `a_continue_carries_what_is_new_under_its_conversations_identity` in
    `tests/upstream.rs`
  - AC-5: `a_continue_passes_on_the_callers_own_billing_block`,
    `a_prompt_that_mentions_the_billing_header_is_not_the_billing_block` in
    `tests/upstream.rs`; `a_billing_block_that_changes_every_request_does_not_end_the_thread`;
    `a_prompt_that_mentions_the_billing_header_is_part_of_the_system`,
    `only_a_billing_block_that_is_the_header_alone_is_left_out_of_the_system` in
    `src/threads.rs`
  - AC-6: `a_changed_conversation_creates_instead_of_continuing`,
    `a_changed_beta_set_creates_instead_of_continuing`,
    `a_thread_does_not_follow_its_conversation_to_another_account`,
    `a_request_that_adds_no_user_message_after_the_reply_is_not_continued`,
    `a_message_that_only_looks_like_the_reply_is_not_skipped`;
    `a_changed_fixed_field_or_beta_creates`, `an_idle_thread_creates`,
    `a_message_changed_between_the_opening_and_the_reply_creates` in `src/threads.rs`;
    `whatever_a_continue_leaves_out_the_ledger_compares` in `tests/upstream.rs`
  - AC-7: `a_cache_marker_that_moved_does_not_end_the_thread`,
    `a_reply_repeated_with_or_without_its_thinking_continues`;
    `what_a_harness_is_free_to_change_does_not_end_the_thread` in `src/threads.rs`
  - AC-8: `a_turn_the_vendor_did_not_store_is_not_continued`,
    `a_reply_streamed_in_small_pieces_is_continued_from`;
    `only_a_reply_the_vendor_holds_is_stored`,
    `a_streamed_reply_is_the_reply_it_would_be_whole`,
    `a_stream_the_vendor_did_not_finish_or_store_is_no_reply` in `src/threads.rs`
  - AC-9: `two_conversations_in_one_session_never_cross`,
    `a_side_request_in_the_same_session_does_not_end_the_thread`,
    `a_caller_that_names_no_session_continues_from_its_second_turn`
  - AC-10: `a_create_the_vendor_declines_is_sent_again_plain`,
    `a_continue_whose_thread_is_gone_is_sent_again_as_a_create`,
    `a_continue_declined_for_what_it_carries_leaves_the_conversation_plain`;
    `only_some_answers_send_a_threaded_turn_again` in `src/app.rs`
  - AC-11: `an_error_that_is_the_callers_reaches_it_and_ends_nothing`,
    `a_callers_own_error_on_a_later_turn_does_not_end_threading`
  - AC-12: `a_rate_limit_on_a_continue_is_the_accounts_and_fails_over`,
    `a_vendor_fault_on_a_continue_is_answered_plain_and_held_against_nothing`;
    `only_some_answers_send_a_threaded_turn_again` in `src/app.rs`
  - AC-13: `a_vendor_that_stopped_taking_threads_is_not_asked_for_every_conversation`,
    `what_one_account_declines_is_not_held_against_another`;
    `conversations_gone_plain_in_a_row_rest_their_account_for_an_hour`,
    `a_rest_and_a_plain_conversation_are_their_accounts_alone`,
    `a_conversation_gone_plain_stays_plain_and_others_do_not`,
    `a_reply_that_was_on_its_way_does_not_end_the_rest`,
    `a_full_ledger_forgets_and_goes_on` in `src/threads.rs`
  - AC-14: `a_tool_call_on_a_continue_comes_back_under_the_callers_name`
  - AC-15: `a_thread_changes_nothing_a_caller_receives`
  - AC-16: `threads_off_sends_every_conversation_plain`;
    `message_threads_are_on_until_the_config_turns_them_off`,
    `the_relay_does_not_write_the_thread_switch_unless_the_operator_set_it` in
    `tests/config_tokens_providers.rs`
  - the non-goal on translated routes: `a_request_translated_onto_anthropic_is_never_threaded`

## Decisions

- [Who threads]: the relay. A thread is bound to the Account that opened it, and the relay
  is the only party that knows which Account serves a conversation.
- [What crosses the seam]: the conversation whole and a `Threading` saying how much of it
  the vendor holds. What leaves is the Upstream client's to write: the beta set, the
  billing-header block and the shape of a continue are vendor identity, and vendor identity
  is decided below `UpstreamClient`. A test double therefore sees the pre-cloak conversation
  on every turn.
- [Continue shape]: what Claude Code sends when it threads on its own: the messages after
  the reply, no `tools`, the billing-header block as the only `system`, the create's beta
  set.
- [Beta set]: derived from the whole conversation on every turn, so a continue earns what
  its create earned though it carries no tools. The relay compares the set a request earns
  and never writes it.
- [Fixed fields]: the vendor's documented list. The vendor takes a continue that changes
  `temperature`, `max_tokens`, `stop_sequences` and `stream`, so those are free.
- [Idle]: 270 seconds against the vendor's five minutes. The vendor takes a continue 262
  seconds after its reply.
- [Declined threads]: decided by status, never by the words of an error, which change. Any
  4xx to a threaded turn but 401 and 429 may be the thread's doing, so the turn is sent
  again plain and the answer to that is the one that counts. Only a thread that is gone or
  busy opens another: anything else would be declined the same way on the next continue,
  two requests a turn.
- [Vendor faults]: a fault on a continue is sent again plain too, because a continue is the
  one request that asks the vendor for what it holds. It is not the vendor declining, so it
  marks nothing: one bad answer does not cost a long conversation its thread. A fault on a
  create fails over as for any request, and a 529 is never answered with another request.
- [Going plain]: a conversation goes plain only once the vendor has taken the same turn
  plain, so a caller's own error marks nothing. It is held against the Account the thread
  was declined on: a Pool can hold an Account the vendor takes no thread for beside one it
  does. The rest after three exists because a beta is retired one day.
- [State]: in memory, one entry per thread, 4096 at most, taken on use and dropped on the
  first doubt; a restart costs one create per conversation.
- [Why not imitate]: Claude Code computes its billing `cch` in bundled native code that an
  outside client cannot reproduce without forging the vendor's integrity check.
- [Switch]: `cloaking.message-threads`, on by default. It sits with Cloaking because the
  shape of a threaded request is the vendor CLI's own. A thread changes what the relay
  sends and never what a caller receives, and every declined thread ends in a plain
  request. A build without the key refuses a config that carries it, so the relay writes
  the key only when an operator set it, and a config it writes still loads on the build
  before.
