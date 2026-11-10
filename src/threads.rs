//! Message threads: where a conversation is continued on the vendor instead of resent.
//!
//! The relay is the thread client. A harness sends the whole conversation on every turn;
//! this ledger decides whether that can leave as a `continue`, and holds the
//! little the next turn needs to say so. Everything here is per process: a restart costs
//! one `create` per conversation, and nothing about a conversation outlives its traffic.
//!
//! State is dropped on the first doubt, never repaired. A turn the vendor declines as a
//! thread costs one request and is sent again without the caller knowing, so a wrong guess
//! here is cheap. A wrong continuation would be silent: the vendor would answer from its
//! own copy of a conversation the caller has since changed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::upstream::{Threading, meter_only_block};
use crate::utils::sha256_hex;

/// How long a reply stays continuable. The vendor drops a thread five minutes after its
/// last request; the margin keeps a turn that arrives at the edge from being declined.
const IDLE: Duration = Duration::from_secs(270);

/// How many threads are remembered, and how many conversations are remembered as plain.
const CAPACITY: usize = 4096;

/// How many conversations go plain on an Account in a row, with no thread taken there in
/// between, before the vendor is taken to have stopped taking threads for it, and how long
/// that Account is not asked again. A beta is retired one day, and an Account can be without
/// it; without this every new conversation would pay a declined request for as long as
/// nobody noticed.
const PLAIN_BEFORE_REST: u32 = 3;
const REST: Duration = Duration::from_hours(1);

/// The top-level fields a thread may not change between its turns, `system` aside: that
/// one is compared without its billing-header block.
const FIXED: [&str; 9] = [
    "model",
    "tools",
    "thinking",
    "tool_choice",
    "output_config",
    "mcp_servers",
    "context_management",
    "container",
    "cache_control",
];

/// The stop reasons after which the vendor holds the reply. Anything else, a stop this
/// relay has never seen included, leaves the turn unstored as far as it knows.
const STORED_STOPS: [&str; 4] = ["end_turn", "tool_use", "stop_sequence", "max_tokens"];

#[derive(PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    Other,
}

struct Message {
    role: Role,
    hash: String,
}

/// One request of a conversation, reduced to what a thread is checked against.
pub struct Turn {
    /// Names the conversation this request belongs to: everything a thread may not change
    /// (the fixed fields and the beta set) and the message the conversation opens with.
    /// A harness sends side requests under its conversation's session, with another model
    /// or prompt; under this key they neither end its thread nor continue it.
    key: String,
    messages: Vec<Message>,
    /// `prefixes[n]` is the hash of the first `n` messages.
    prefixes: Vec<String>,
}

impl Turn {
    /// `body` is the request as it would go upstream whole, after the Cloaking sanitizer.
    /// `beta` is the
    /// beta set it earns.
    #[must_use]
    pub fn of(body: &Value, beta: &str) -> Self {
        let fixed: Vec<&Value> = FIXED
            .iter()
            .map(|field| body.get(*field).unwrap_or(&Value::Null))
            .collect();
        // Claude Code writes a new billing-header block for every request. It names the
        // request to the vendor's meter and says nothing to the model, so it is no part of
        // what a thread's `system` is.
        let system: Vec<&Value> = match body.get("system") {
            Some(Value::Array(blocks)) => {
                let meter_only = meter_only_block(blocks);
                blocks
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| Some(*index) != meter_only)
                    .map(|(_, block)| block)
                    .collect()
            }
            Some(other) => vec![other],
            None => Vec::new(),
        };
        // Serialized by reference: `system` and `tools` are the largest fields of a request.
        let fingerprint =
            sha256_hex(&serde_json::to_string(&(&system, &fixed, beta)).unwrap_or_default());

        let messages: Vec<Message> = body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|message| Message {
                role: match message.get("role").and_then(Value::as_str) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => Role::Other,
                },
                hash: message_hash(message),
            })
            .collect();
        let mut prefixes = Vec::with_capacity(messages.len() + 1);
        prefixes.push(sha256_hex(""));
        for message in &messages {
            let last = prefixes.last().map_or("", String::as_str);
            prefixes.push(sha256_hex(&format!("{last}{}", message.hash)));
        }
        let opening = messages.first().map_or("", |message| message.hash.as_str());
        Self {
            key: sha256_hex(&format!("{fingerprint}{opening}")),
            messages,
            prefixes,
        }
    }

    /// The whole conversation of this turn, as one hash.
    fn whole(&self) -> &str {
        self.prefixes.last().map_or("", String::as_str)
    }
}

/// A reply the vendor stored, as the next turn must repeat it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    id: String,
    hash: String,
}

impl Stored {
    /// From a whole reply. `None` when the vendor did not store the turn, or this relay
    /// cannot tell that it did.
    #[must_use]
    pub fn of(reply: &Value) -> Option<Self> {
        Self::new(
            reply.get("id").and_then(Value::as_str)?,
            reply.get("stop_reason").and_then(Value::as_str),
            reply.get("content")?,
            reply.get("usage"),
        )
    }

    fn new(id: &str, stop: Option<&str>, content: &Value, usage: Option<&Value>) -> Option<Self> {
        let stop = stop?;
        let blocks = content.as_array()?;
        // A tool call the output limit cut is not a turn: its input is half a document.
        let cut_tool_call = stop == "max_tokens"
            && blocks
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"));
        // Another model answered in place of the thread's own.
        let fell_back = usage
            .and_then(|usage| usage.get("iterations"))
            .and_then(Value::as_array)
            .is_some_and(|iterations| {
                iterations
                    .iter()
                    .any(|iteration| iteration.get("fallback_message").is_some())
            });
        if id.is_empty() || !STORED_STOPS.contains(&stop) || cut_tool_call || fell_back {
            return None;
        }
        Some(Self {
            id: id.to_string(),
            hash: message_hash(&json!({"role": "assistant", "content": content})),
        })
    }
}

/// A reply as it arrives in a stream, kept as far as the next turn is checked against it.
#[derive(Default)]
pub struct StreamingReply {
    id: String,
    blocks: BTreeMap<u64, Value>,
    /// A tool call's input arrives as JSON text, piece by piece.
    inputs: BTreeMap<u64, String>,
    stop: Option<String>,
    usage: Option<Value>,
    finished: bool,
    /// The stream held something this reader cannot rebuild, so the reply is not known.
    unreadable: bool,
}

impl StreamingReply {
    /// Events are taken in the order the vendor sent them: a block is opened, filled and
    /// closed before the message is.
    pub fn feed(&mut self, event: &Value) {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                self.id = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
            Some("content_block_start") => {
                match event.pointer("/content_block/type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = event.pointer("/content_block/text").and_then(Value::as_str);
                        self.blocks.insert(
                            index,
                            json!({"type": "text", "text": text.unwrap_or_default()}),
                        );
                    }
                    Some("tool_use") => {
                        self.blocks.insert(
                            index,
                            json!({
                                "type": "tool_use",
                                "id": event.pointer("/content_block/id"),
                                "name": event.pointer("/content_block/name"),
                                "input": {}
                            }),
                        );
                        self.inputs.insert(index, String::new());
                    }
                    // The vendor keeps its own thinking, and no turn is checked against it.
                    Some("thinking" | "redacted_thinking") => {}
                    _ => self.unreadable = true,
                }
            }
            Some("content_block_delta") => {
                match event.pointer("/delta/type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let piece = event.pointer("/delta/text").and_then(Value::as_str);
                        if let Some(Value::String(text)) = self
                            .blocks
                            .get_mut(&index)
                            .and_then(|block| block.get_mut("text"))
                        {
                            text.push_str(piece.unwrap_or_default());
                        }
                    }
                    Some("input_json_delta") => {
                        let piece = event.pointer("/delta/partial_json").and_then(Value::as_str);
                        if let Some(input) = self.inputs.get_mut(&index) {
                            input.push_str(piece.unwrap_or_default());
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                if let Some(input) = self.inputs.remove(&index) {
                    let input = if input.trim().is_empty() {
                        "{}"
                    } else {
                        input.as_str()
                    };
                    match (
                        serde_json::from_str::<Value>(input),
                        self.blocks.get_mut(&index),
                    ) {
                        (Ok(input), Some(block)) => block["input"] = input,
                        _ => self.unreadable = true,
                    }
                }
            }
            Some("message_delta") => {
                if let Some(stop) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(stop.to_string());
                }
                if let Some(usage) = event.get("usage") {
                    self.usage = Some(usage.clone());
                }
            }
            Some("message_stop") => self.finished = true,
            Some("error") => self.unreadable = true,
            _ => {}
        }
    }

    /// Whether the vendor has said the message is over.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// The reply, once it is finished, readable and stored.
    #[must_use]
    pub fn stored(&self) -> Option<Stored> {
        if !self.finished || self.unreadable {
            return None;
        }
        let content = Value::Array(self.blocks.values().cloned().collect());
        Stored::new(
            &self.id,
            self.stop.as_deref(),
            &content,
            self.usage.as_ref(),
        )
    }
}

struct Entry {
    /// The Account that holds the thread. A thread never leaves it.
    account: String,
    reply: Stored,
    /// How many messages the vendor held before `reply`.
    sent: usize,
    /// Hash of those messages, which the next turn must repeat.
    prefix: String,
    noted_at: Instant,
}

#[derive(Default)]
pub struct ThreadLedger {
    entries: Mutex<BTreeMap<String, Entry>>,
    /// Conversations that went plain, each on the Account whose thread the vendor
    /// declined. They stay plain there while the set has room for them: what was declined
    /// once would be declined again, one request each time. Another Account is still asked.
    plain: Mutex<BTreeSet<String>>,
    /// By Account: how many conversations went plain there since the vendor last took a
    /// thread on it, and when its rest ends.
    rests: Mutex<BTreeMap<String, (u32, Option<Instant>)>>,
}

/// The lock, whatever became of the last request that held it: the maps are valid after
/// any single operation on them.
fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ThreadLedger {
    /// Whether `account` is resting: the vendor declined enough threads on it, and none of
    /// its conversations is offered as one for a while.
    #[must_use]
    pub fn resting(&self, account: &str, now: Instant) -> bool {
        held(&self.rests)
            .get(account)
            .and_then(|rest| rest.1)
            .is_some_and(|until| now < until)
    }

    /// How this turn goes out on `account`: plain while the Account rests or the
    /// conversation went plain on it. `session` is the session the harness names, or empty
    /// when it names none.
    pub fn plan(&self, session: &str, account: &str, turn: &Turn, now: Instant) -> Threading {
        let key = format!("{session}\n{}", turn.key);
        if self.resting(account, now) || held(&self.plain).contains(&format!("{account}\n{key}")) {
            return Threading::Plain;
        }
        // Taken, not read. A reply is continued once, and a thread that fails a check below
        // is not kept for a later turn to try.
        let Some(entry) = held(&self.entries).remove(&key) else {
            return Threading::Create;
        };
        let sent = entry.sent;
        let repeated = turn.messages.get(sent).is_some_and(|message| {
            message.role == Role::Assistant && message.hash == entry.reply.hash
        });
        let adds = turn
            .messages
            .get(sent + 1)
            .is_some_and(|message| message.role == Role::User);
        // The caller repeats the reply where the vendor's copy of it stands, and adds a
        // message of its own after it. Anything else is a conversation the vendor does not
        // hold: an edited reply, a dropped one, a request that only repeats it.
        let doubt = if entry.account != account {
            "another account"
        } else if now.saturating_duration_since(entry.noted_at) >= IDLE {
            "idle"
        } else if turn.prefixes.get(sent) != Some(&entry.prefix) {
            "earlier messages changed"
        } else if !repeated {
            "the reply is not repeated"
        } else if !adds {
            "no user message after the reply"
        } else {
            return Threading::Continue {
                previous_message_id: entry.reply.id,
                from: sent + 1,
            };
        };
        tracing::debug!(session, doubt, "message thread not continued");
        Threading::Create
    }

    /// The vendor answered `turn` with `reply` and stored both.
    pub fn note_reply(
        &self,
        session: &str,
        account: &str,
        turn: &Turn,
        reply: Stored,
        now: Instant,
    ) {
        // The count, not the rest: a reply that was on its way when the rest began says
        // nothing about what the vendor takes now.
        if let Some(rest) = held(&self.rests).get_mut(account) {
            rest.0 = 0;
        }
        let mut entries = held(&self.entries);
        if entries.len() >= CAPACITY {
            entries.retain(|_, entry| now.saturating_duration_since(entry.noted_at) < IDLE);
        }
        if entries.len() >= CAPACITY {
            entries.clear();
        }
        entries.insert(
            format!("{session}\n{}", turn.key),
            Entry {
                account: account.to_string(),
                reply,
                sent: turn.messages.len(),
                prefix: turn.whole().to_string(),
                noted_at: now,
            },
        );
    }

    /// The vendor declined a thread for `turn` on `account` and then took the turn plain.
    /// The conversation goes out plain there from now on, and enough of them in a row rest
    /// the Account. `why` is what the vendor answered the thread with, for the operator.
    pub fn go_plain(&self, session: &str, account: &str, turn: &Turn, now: Instant, why: &str) {
        let key = format!("{session}\n{}", turn.key);
        held(&self.entries).remove(&key);
        {
            let mut plain = held(&self.plain);
            if plain.len() >= CAPACITY {
                plain.clear();
            }
            plain.insert(format!("{account}\n{key}"));
        }
        let mut rests = held(&self.rests);
        let rest = rests.entry(account.to_string()).or_default();
        rest.0 += 1;
        if rest.0 >= PLAIN_BEFORE_REST {
            tracing::warn!(
                account,
                conversations = rest.0,
                minutes = REST.as_secs() / 60,
                vendor = why,
                "the vendor keeps declining message threads on this Account; its conversations go plain for a while"
            );
            *rest = (0, Some(now + REST));
        }
    }
}

/// A message as a hash that its two holders agree on.
///
/// The vendor holds what it was sent and what it answered; the caller holds what it sent
/// and what it kept of the answer. They differ where a harness is free to differ: a cache
/// marker moves every turn, thinking is kept or dropped, an empty text block is dropped,
/// a text-only message is a string or one block. None of that changes what was said.
fn message_hash(message: &Value) -> String {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let assistant = role == "assistant";
    let content: Vec<Value> = match message.get("content") {
        Some(Value::String(text)) if !text.is_empty() => {
            vec![json!({"type": "text", "text": text})]
        }
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| said(block, assistant))
            .collect(),
        _ => Vec::new(),
    };
    sha256_hex(&json!({"role": role, "content": content}).to_string())
}

/// One content block, reduced to what it says.
fn said(block: &Value, assistant: bool) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (!text.is_empty()).then(|| json!({"type": "text", "text": text}))
        }
        Some("thinking" | "redacted_thinking") if assistant => None,
        Some("tool_use") if assistant => Some(json!({
            "type": "tool_use",
            "id": block.get("id"),
            "name": block.get("name"),
            "input": block.get("input")
        })),
        _ => {
            let mut block = block.clone();
            unmark(&mut block);
            if let Some(nested) = block.get_mut("content").and_then(Value::as_array_mut) {
                nested.iter_mut().for_each(unmark);
            }
            Some(block)
        }
    }
}

fn unmark(block: &mut Value) {
    if let Some(object) = block.as_object_mut() {
        object.remove("cache_control");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "a@example.com";

    /// A conversation as a harness resends it: `turns` user messages, each earlier one
    /// followed by the `pong` the vendor answered.
    fn conversation(turns: usize) -> Value {
        let mut messages = Vec::new();
        for turn in 1..=turns {
            if turn > 1 {
                messages.push(json!({"role": "assistant", "content": "pong"}));
            }
            messages.push(json!({"role": "user", "content": format!("turn {turn}")}));
        }
        json!({"model": "m", "messages": messages})
    }

    fn turn(body: &Value) -> Turn {
        Turn::of(body, "beta")
    }

    fn pong(n: usize) -> Stored {
        Stored::of(&json!({
            "id": format!("msg_{n}"),
            "content": [{"type": "text", "text": "pong"}],
            "stop_reason": "end_turn"
        }))
        .expect("a stored reply")
    }

    /// A ledger whose vendor answered the first `turns` turns of the conversation.
    fn answered(turns: usize) -> (ThreadLedger, Instant) {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        ledger.note_reply("c", ACCOUNT, &turn(&conversation(turns)), pong(turns), now);
        (ledger, now)
    }

    fn continues(n: usize, from: usize) -> Threading {
        Threading::Continue {
            previous_message_id: format!("msg_{n}"),
            from,
        }
    }

    #[test]
    fn a_conversation_with_no_thread_creates() {
        let ledger = ThreadLedger::default();
        let plan = ledger.plan("c", ACCOUNT, &turn(&conversation(1)), Instant::now());
        assert_eq!(plan, Threading::Create);
    }

    #[test]
    fn a_grown_conversation_continues_after_its_reply_and_only_once() {
        let (ledger, now) = answered(1);
        let next = turn(&conversation(2));
        assert_eq!(ledger.plan("c", ACCOUNT, &next, now), continues(1, 2));
        assert_eq!(ledger.plan("c", ACCOUNT, &next, now), Threading::Create);
    }

    #[test]
    fn a_continued_thread_continues_again() {
        let (ledger, now) = answered(1);
        let second = turn(&conversation(2));
        assert_eq!(ledger.plan("c", ACCOUNT, &second, now), continues(1, 2));
        ledger.note_reply("c", ACCOUNT, &second, pong(2), now);
        let third = turn(&conversation(3));
        assert_eq!(ledger.plan("c", ACCOUNT, &third, now), continues(2, 4));
    }

    #[test]
    fn an_idle_thread_creates() {
        let next = turn(&conversation(2));
        let (ledger, now) = answered(1);
        let fresh = now + Duration::from_secs(269);
        assert_eq!(ledger.plan("c", ACCOUNT, &next, fresh), continues(1, 2));

        let (ledger, now) = answered(1);
        let idle = now + Duration::from_secs(270);
        assert_eq!(ledger.plan("c", ACCOUNT, &next, idle), Threading::Create);
    }

    #[test]
    fn another_account_creates() {
        let (ledger, now) = answered(1);
        let plan = ledger.plan("c", "b@example.com", &turn(&conversation(2)), now);
        assert_eq!(plan, Threading::Create);
    }

    #[test]
    fn a_changed_fixed_field_or_beta_creates() {
        // AC-6's list, written out: a field dropped from `FIXED` fails here.
        let fixed = [
            "model",
            "tools",
            "thinking",
            "tool_choice",
            "output_config",
            "mcp_servers",
            "context_management",
            "container",
            "cache_control",
        ];
        assert_eq!(fixed, FIXED);
        for field in fixed {
            let (ledger, now) = answered(1);
            let mut changed = conversation(2);
            changed[field] = json!("changed");
            assert_eq!(
                ledger.plan("c", ACCOUNT, &turn(&changed), now),
                Threading::Create,
                "{field}"
            );
        }
        let (ledger, now) = answered(1);
        let wider = Turn::of(&conversation(2), "beta,another-beta");
        assert_eq!(ledger.plan("c", ACCOUNT, &wider, now), Threading::Create);
    }

    #[test]
    fn a_changed_system_creates_and_a_changed_billing_block_does_not() {
        let talk = |turns: usize, billing: &str, persona: &str| {
            let mut body = conversation(turns);
            body["system"] = json!([
                {"type": "text", "text": format!("x-anthropic-billing-header: cch={billing};")},
                {"type": "text", "text": persona}
            ]);
            body
        };
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        ledger.note_reply(
            "c",
            ACCOUNT,
            &turn(&talk(1, "00001", "be brief")),
            pong(1),
            now,
        );
        let plan = ledger.plan("c", ACCOUNT, &turn(&talk(2, "00002", "be brief")), now);
        assert_eq!(plan, continues(1, 2));

        ledger.note_reply(
            "c",
            ACCOUNT,
            &turn(&talk(1, "00001", "be brief")),
            pong(1),
            now,
        );
        let plan = ledger.plan("c", ACCOUNT, &turn(&talk(2, "00001", "be thorough")), now);
        assert_eq!(plan, Threading::Create);
    }

    #[test]
    fn a_prompt_that_mentions_the_billing_header_is_part_of_the_system() {
        // Only a block that is the header line is a billing-header block. A prompt that quotes
        // the header is what the model is told, and a change to it is a change of system.
        let talk = |turns: usize, prompt: &str| {
            let mut body = conversation(turns);
            body["system"] = json!([{"type": "text", "text": prompt}]);
            body
        };
        let quoted = "Never print the x-anthropic-billing-header: line.";
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        ledger.note_reply("c", ACCOUNT, &turn(&talk(1, quoted)), pong(1), now);
        let changed = talk(2, "Always print the x-anthropic-billing-header: line.");
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&changed), now),
            Threading::Create
        );

        ledger.note_reply("c", ACCOUNT, &turn(&talk(1, quoted)), pong(1), now);
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk(2, quoted)), now),
            continues(1, 2)
        );
    }

    #[test]
    fn only_a_billing_block_that_is_the_header_alone_is_left_out_of_the_system() {
        let header = "x-anthropic-billing-header: cc_version=2.1.322.6ba; cch=00000;";
        let talk = |turns: usize, system: Value| {
            let mut body = conversation(turns);
            body["system"] = system;
            body
        };
        let changes = [
            // An instruction after the header's fields, in the same block.
            (
                json!([{"type": "text", "text": format!("{header}\nAnswer in English.")}]),
                json!([{"type": "text", "text": format!("{header}\nAnswer in French.")}]),
            ),
            (
                json!([{"type": "text", "text": format!("{header} Answer in English.")}]),
                json!([{"type": "text", "text": format!("{header} Answer in French.")}]),
            ),
            // A second block that starts like the first: Cloaking sends it as `system`.
            (
                json!([{"type": "text", "text": header}, {"type": "text", "text": format!("{header} english")}]),
                json!([{"type": "text", "text": header}, {"type": "text", "text": format!("{header} french")}]),
            ),
        ];
        for (before, after) in changes {
            let ledger = ThreadLedger::default();
            let now = Instant::now();
            ledger.note_reply("c", ACCOUNT, &turn(&talk(1, before.clone())), pong(1), now);
            let plan = ledger.plan("c", ACCOUNT, &turn(&talk(2, after)), now);
            assert_eq!(plan, Threading::Create, "{before}");

            ledger.note_reply("c", ACCOUNT, &turn(&talk(1, before.clone())), pong(1), now);
            let plan = ledger.plan("c", ACCOUNT, &turn(&talk(2, before)), now);
            assert_eq!(plan, continues(1, 2));
        }
    }

    #[test]
    fn a_side_request_under_the_same_session_leaves_the_thread_alone() {
        // A harness asks a small model for a title under its conversation's session.
        let (ledger, now) = answered(1);
        let side = json!({"model": "small", "messages": [{"role": "user", "content": "title?"}]});
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&side), now),
            Threading::Create
        );
        ledger.note_reply("c", ACCOUNT, &turn(&side), pong(9), now);

        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&conversation(2)), now),
            continues(1, 2)
        );
    }

    #[test]
    fn two_conversations_under_one_session_keep_their_own_threads() {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let talk = |opening: &str, grown: bool| {
            let mut messages = vec![json!({"role": "user", "content": opening})];
            if grown {
                messages.push(json!({"role": "assistant", "content": "pong"}));
                messages.push(json!({"role": "user", "content": "more"}));
            }
            json!({"model": "m", "messages": messages})
        };
        ledger.note_reply("c", ACCOUNT, &turn(&talk("alpha", false)), pong(1), now);
        ledger.note_reply("c", ACCOUNT, &turn(&talk("beta", false)), pong(2), now);

        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("alpha", true)), now),
            continues(1, 2)
        );
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("beta", true)), now),
            continues(2, 2)
        );
    }

    /// The opening is part of what names a conversation, and the reply is compared on its
    /// own. Every message between them is held by this check alone: a continue past a
    /// change there would be answered from the vendor's old copy.
    #[test]
    fn a_message_changed_between_the_opening_and_the_reply_creates() {
        let edits: [(usize, &str); 2] = [(1, "an earlier reply, edited"), (2, "turn 2, edited")];
        for (index, text) in edits {
            let ledger = ThreadLedger::default();
            let now = Instant::now();
            ledger.note_reply("c", ACCOUNT, &turn(&conversation(2)), pong(2), now);
            let mut next = conversation(3);
            next["messages"][index]["content"] = json!(text);

            let plan = ledger.plan("c", ACCOUNT, &turn(&next), now);
            assert_eq!(plan, Threading::Create, "message {index}");
        }
        // The same thread, with nothing changed, goes on.
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        ledger.note_reply("c", ACCOUNT, &turn(&conversation(2)), pong(2), now);
        let plan = ledger.plan("c", ACCOUNT, &turn(&conversation(3)), now);
        assert_eq!(plan, continues(2, 4));
    }

    #[test]
    fn a_rewritten_message_or_reply_creates() {
        for (index, content) in [(0, "turn 1, edited"), (1, "not what the vendor said")] {
            let (ledger, now) = answered(1);
            let mut edited = conversation(2);
            edited["messages"][index]["content"] = json!(content);
            assert_eq!(
                ledger.plan("c", ACCOUNT, &turn(&edited), now),
                Threading::Create,
                "{index}"
            );
        }
    }

    #[test]
    fn what_a_harness_is_free_to_change_does_not_end_the_thread() {
        let mark = json!({"type": "ephemeral"});
        let variants = [
            // The reply as blocks instead of a string, with a cache marker on it.
            json!([{"type": "text", "text": "pong", "cache_control": mark}]),
            // The reply with the thinking the vendor sent, and an empty text block.
            json!([
                {"type": "thinking", "thinking": "hm", "signature": "s"},
                {"type": "text", "text": ""},
                {"type": "text", "text": "pong"}
            ]),
        ];
        for reply in variants {
            let (ledger, now) = answered(1);
            let mut next = conversation(2);
            next["messages"][1]["content"] = reply;
            // The first message, now as a block with a marker it did not have before.
            next["messages"][0]["content"] =
                json!([{"type": "text", "text": "turn 1", "cache_control": mark}]);
            assert_eq!(
                ledger.plan("c", ACCOUNT, &turn(&next), now),
                continues(1, 2)
            );
        }
    }

    #[test]
    fn a_cache_marker_inside_a_tool_result_does_not_end_the_thread() {
        let talk = |marked: bool, grown: bool| {
            let mut result = json!({"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "text", "text": "out"}
            ]});
            if marked {
                result["cache_control"] = json!({"type": "ephemeral"});
                result["content"][0]["cache_control"] = json!({"type": "ephemeral"});
            }
            let mut messages = vec![json!({"role": "user", "content": [result]})];
            if grown {
                messages.push(json!({"role": "assistant", "content": "pong"}));
                messages.push(json!({"role": "user", "content": "more"}));
            }
            json!({"model": "m", "messages": messages})
        };
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        ledger.note_reply("c", ACCOUNT, &turn(&talk(true, false)), pong(1), now);
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk(false, true)), now),
            continues(1, 2)
        );
    }

    #[test]
    fn a_user_message_with_the_replys_words_is_not_the_reply() {
        let (ledger, now) = answered(1);
        let mut next = conversation(2);
        next["messages"][1]["role"] = json!("user");
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&next), now),
            Threading::Create
        );
    }

    #[test]
    fn a_turn_with_no_user_message_after_the_reply_creates() {
        for after in [json!([]), json!([{"role": "assistant", "content": "more"}])] {
            let (ledger, now) = answered(1);
            let mut next = conversation(2);
            let messages = next["messages"].as_array_mut().expect("messages");
            messages.truncate(2);
            messages.extend(after.as_array().cloned().unwrap_or_default());
            assert_eq!(
                ledger.plan("c", ACCOUNT, &turn(&next), now),
                Threading::Create
            );
        }
    }

    #[test]
    fn conversations_gone_plain_in_a_row_rest_their_account_for_an_hour() {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let talk = |opening: &str| json!({"model": "m", "messages": [{"role": "user", "content": opening}]});
        // A vendor that has stopped taking threads declines each conversation's first one.
        for opening in ["one", "two"] {
            ledger.go_plain("c", ACCOUNT, &turn(&talk(opening)), now, "400 no");
        }
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("still asked")), now),
            Threading::Create
        );
        ledger.go_plain("c", ACCOUNT, &turn(&talk("three")), now, "400 no");

        // The Account is not asked again until its rest is over: an hour, to the second.
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("four")), now),
            Threading::Plain
        );
        let almost = now + Duration::from_secs(3599);
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("four")), almost),
            Threading::Plain
        );
        let rested = now + Duration::from_hours(1);
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("four")), rested),
            Threading::Create
        );
        // The conversations that were declined themselves stay plain.
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("one")), rested),
            Threading::Plain
        );
    }

    #[test]
    fn a_rest_and_a_plain_conversation_are_their_accounts_alone() {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let talk = |opening: &str| json!({"model": "m", "messages": [{"role": "user", "content": opening}]});
        // One Account of the Pool has no threads; the other takes them all along.
        for opening in ["one", "two", "three"] {
            ledger.go_plain(
                "c",
                "none@example.com",
                &turn(&talk(opening)),
                now,
                "403 no",
            );
            ledger.note_reply("c", ACCOUNT, &turn(&conversation(1)), pong(1), now);
        }

        // The Account that declined rests, though the other was served in between...
        let plan = |account: &str, body: &Value| ledger.plan("c", account, &turn(body), now);
        assert_eq!(plan("none@example.com", &talk("four")), Threading::Plain);
        // ...and the other is still asked, also for a conversation the first declined.
        assert_eq!(plan(ACCOUNT, &talk("four")), Threading::Create);
        assert_eq!(plan(ACCOUNT, &talk("one")), Threading::Create);
        assert_eq!(plan(ACCOUNT, &conversation(2)), continues(1, 2));
    }

    #[test]
    fn a_full_ledger_forgets_and_goes_on() {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let (first, next) = (turn(&conversation(1)), turn(&conversation(2)));
        let session = |n: usize| format!("session {n}");
        // One more conversation than the ledger holds, each gone plain on one Account and
        // then answered on another.
        for n in 0..=CAPACITY {
            ledger.go_plain(&session(n), "other@example.com", &first, now, "400 no");
            ledger.note_reply(&session(n), ACCOUNT, &first, pong(1), now);
        }

        // The last reply noted is still held; one that was dropped starts over.
        let plan = ledger.plan(&session(CAPACITY), ACCOUNT, &next, now);
        assert_eq!(plan, continues(1, 2));
        assert_eq!(
            ledger.plan(&session(0), ACCOUNT, &next, now),
            Threading::Create
        );
        // The same for what went plain, once the Account it went plain on has rested.
        let rested = now + REST;
        let plan = |n: usize| ledger.plan(&session(n), "other@example.com", &first, rested);
        assert_eq!(plan(CAPACITY), Threading::Plain);
        assert_eq!(plan(0), Threading::Create);
    }

    #[test]
    fn a_reply_that_was_on_its_way_does_not_end_the_rest() {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let talk = |opening: &str| json!({"model": "m", "messages": [{"role": "user", "content": opening}]});
        for opening in ["one", "two", "three"] {
            ledger.go_plain("c", ACCOUNT, &turn(&talk(opening)), now, "400 no");
        }
        // A stream the vendor opened before it stopped taking threads finishes now.
        ledger.note_reply("c", ACCOUNT, &turn(&conversation(1)), pong(1), now);

        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("four")), now),
            Threading::Plain
        );
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&conversation(2)), now),
            Threading::Plain
        );
    }

    #[test]
    fn a_thread_the_vendor_takes_on_an_account_ends_its_count() {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let talk = |opening: &str| json!({"model": "m", "messages": [{"role": "user", "content": opening}]});
        // One shape of request the vendor will not thread, among others it will.
        for opening in ["one", "two", "three", "four"] {
            ledger.go_plain("c", ACCOUNT, &turn(&talk(opening)), now, "400 no");
            ledger.note_reply("c", ACCOUNT, &turn(&conversation(1)), pong(1), now);
        }
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&talk("five")), now),
            Threading::Create
        );
    }

    #[test]
    fn a_conversation_gone_plain_stays_plain_and_others_do_not() {
        let (ledger, now) = answered(1);
        ledger.go_plain("c", ACCOUNT, &turn(&conversation(1)), now, "400 no");
        for turns in 1..=3 {
            let plan = ledger.plan("c", ACCOUNT, &turn(&conversation(turns)), now);
            assert_eq!(plan, Threading::Plain);
        }
        let other = json!({"model": "m", "messages": [{"role": "user", "content": "other"}]});
        assert_eq!(
            ledger.plan("c", ACCOUNT, &turn(&other), now),
            Threading::Create
        );
        assert_eq!(
            ledger.plan("d", ACCOUNT, &turn(&conversation(1)), now),
            Threading::Create
        );
    }

    fn reply(stop: Option<&str>, content: &Value) -> Value {
        json!({"id": "msg_1", "content": content, "stop_reason": stop})
    }

    #[test]
    fn only_a_reply_the_vendor_holds_is_stored() {
        let text = json!([{"type": "text", "text": "pong"}]);
        let call = json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {}}]);
        for stop in STORED_STOPS {
            assert!(Stored::of(&reply(Some(stop), &text)).is_some(), "{stop}");
        }
        assert!(Stored::of(&reply(Some("tool_use"), &call)).is_some());
        for (what, unstored) in [
            (
                "the model declining to answer",
                reply(Some("refusal"), &text),
            ),
            ("an unknown stop", reply(Some("pause_turn"), &text)),
            ("no stop", reply(None, &text)),
            ("a cut tool call", reply(Some("max_tokens"), &call)),
            ("no id", json!({"content": text, "stop_reason": "end_turn"})),
            (
                "an empty id",
                json!({"id": "", "content": text, "stop_reason": "end_turn"}),
            ),
            (
                "another model's answer",
                json!({"id": "msg_1", "content": text, "stop_reason": "end_turn",
                    "usage": {"iterations": [{"fallback_message": {}}]}}),
            ),
        ] {
            assert_eq!(Stored::of(&unstored), None, "{what}");
        }
    }

    fn streamed(events: &[Value]) -> StreamingReply {
        let mut reply = StreamingReply::default();
        for event in events {
            reply.feed(event);
        }
        reply
    }

    fn stream_of_pong_and_a_tool_call() -> Vec<Value> {
        vec![
            json!({"type": "message_start", "message": {"id": "msg_1"}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hm"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "po"}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "ng"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t", "name": "Bash", "input": {}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"cmd\":"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"ls\"}"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 3}}),
            json!({"type": "message_stop"}),
        ]
    }

    #[test]
    fn a_streamed_reply_is_the_reply_it_would_be_whole() {
        let whole = Stored::of(&reply(
            Some("tool_use"),
            &json!([
                {"type": "text", "text": "pong"},
                {"type": "tool_use", "id": "t", "name": "Bash", "input": {"cmd": "ls"}}
            ]),
        ));
        assert!(whole.is_some());
        assert_eq!(streamed(&stream_of_pong_and_a_tool_call()).stored(), whole);
    }

    #[test]
    fn a_tool_call_with_no_input_text_has_an_empty_input() {
        let events = [
            json!({"type": "message_start", "message": {"id": "msg_1"}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t", "name": "Bash", "input": {}}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
            json!({"type": "message_stop"}),
        ];
        let whole = Stored::of(&reply(
            Some("tool_use"),
            &json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {}}]),
        ));
        assert_eq!(streamed(&events).stored(), whole);
    }

    #[test]
    fn a_stream_the_vendor_did_not_finish_or_store_is_no_reply() {
        let whole = stream_of_pong_and_a_tool_call();
        let without = |kind: &str| -> Vec<Value> {
            whole
                .iter()
                .filter(|event| event["type"] != kind)
                .cloned()
                .collect()
        };
        let with = |extra: Value| -> Vec<Value> {
            let mut events = whole.clone();
            events.insert(1, extra);
            events
        };
        let mut unanswered = whole.clone();
        unanswered[12]["delta"]["stop_reason"] = json!("refusal");
        let mut garbled = whole.clone();
        garbled[10]["delta"]["partial_json"] = json!("\"ls\"");
        for (what, events) in [
            ("no message_stop", without("message_stop")),
            ("no stop reason", without("message_delta")),
            ("the model declining to answer", unanswered),
            (
                "an error event",
                with(json!({"type": "error", "error": {"type": "overloaded_error"}})),
            ),
            (
                "a block this reader does not know",
                with(
                    json!({"type": "content_block_start", "index": 7, "content_block": {"type": "server_tool_use"}}),
                ),
            ),
            ("a tool input that is not JSON", garbled),
        ] {
            assert_eq!(streamed(&events).stored(), None, "{what}");
        }
    }
}
