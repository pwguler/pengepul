//! Message threads: where a conversation is continued on the vendor instead of resent.
//!
//! The relay is the thread client. A harness keeps sending the whole conversation, as it
//! always has; this ledger decides whether that can leave as a `continue`, and holds the
//! little the next turn needs to say so. Everything here is per process: a restart costs
//! one `create` per conversation, and nothing about a conversation outlives its traffic.
//!
//! State is dropped on the first doubt, never repaired. A `continue` that the vendor
//! rejects costs one request and falls back to a `create`, so a wrong guess here is
//! cheap while a wrong continuation is silent.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::utils::sha256_hex;

/// How long a thread is continued. The vendor drops a thread five minutes after its last
/// request, so continuing after that is a create either way.
const IDLE: Duration = Duration::from_mins(5);

/// How many conversations keep thread state before the whole map is dropped, and how many
/// remember that their endpoint has no threads.
const CAPACITY: usize = 4096;

/// What the next request of a conversation sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The whole conversation, as a new thread.
    Create,
    /// Only `messages[from..]`, continuing `previous_message_id` with the beta set its
    /// create carried.
    Continue {
        previous_message_id: String,
        from: usize,
        beta: String,
    },
}

/// What the vendor answered, as far as the next turn needs it.
pub struct Reply<'a> {
    /// The Account that served the turn. A thread never leaves it.
    pub account: &'a str,
    pub reply_id: &'a str,
    /// How many messages of the conversation the vendor now holds.
    pub sent: usize,
    pub prefix: String,
    pub fingerprint: String,
    pub beta: String,
}

struct Entry {
    account: String,
    reply_id: String,
    /// How many messages of the conversation the vendor holds, once `reply_id` is counted.
    sent: usize,
    /// Hash of those messages as they went upstream, which is what the next turn must repeat.
    prefix: String,
    /// Hash of the fields a thread cannot change, as they went upstream.
    fingerprint: String,
    /// The `anthropic-beta` the create carried. A continue must carry the same set, and it
    /// cannot be recomputed: the tools the set is derived from are not in a continue body.
    beta: String,
    idle_at: Instant,
}

#[derive(Default)]
pub struct ThreadLedger {
    entries: Mutex<BTreeMap<String, Entry>>,
    /// Conversations whose endpoint answered without threads. They stay unthreaded for the
    /// life of the process: a beta the vendor refuses will be refused again.
    stateless: Mutex<BTreeSet<String>>,
}

impl ThreadLedger {
    /// How the conversation's next request goes out. `messages` is the whole conversation
    /// as it would be sent upstream, after masking; `account` is the Account serving it.
    ///
    /// # Panics
    /// If a thread lock was poisoned by a panic in another request.
    pub fn plan(
        &self,
        conversation: &str,
        account: &str,
        messages: &[Value],
        fingerprint: &str,
        now: Instant,
    ) -> Plan {
        if self
            .stateless
            .lock()
            .expect("stateless lock is poisoned")
            .contains(conversation)
        {
            return Plan::Create;
        }
        let mut entries = self.entries.lock().expect("thread lock is poisoned");
        let Some(entry) = entries.get(conversation) else {
            return Plan::Create;
        };
        // A thread lives on one Account. Moving it, or continuing a turn the vendor did not
        // finish, is a create.
        let same_account = entry.account == account;
        let fresh = now.duration_since(entry.idle_at) < IDLE;
        let grew = messages.len() > entry.sent;
        let same_prefix = grew
            && sha256_hex(&Value::Array(messages[..entry.sent].to_vec()).to_string())
                == entry.prefix;
        if !(same_account && fresh && grew && same_prefix && entry.fingerprint == fingerprint) {
            entries.remove(conversation);
            return Plan::Create;
        }
        // Each reply can be continued once. Taking the entry here keeps a second request
        // from racing the same reply into `thread_already_continued`.
        let entry = entries.remove(conversation).expect("entry is present");
        Plan::Continue {
            previous_message_id: entry.reply_id,
            from: entry.sent + 1,
            beta: entry.beta,
        }
    }

    /// Remember where the conversation can continue from.
    ///
    /// # Panics
    /// If a thread lock was poisoned by a panic in another request.
    pub fn note_reply(&self, conversation: &str, reply: Reply, now: Instant) {
        let mut entries = self.entries.lock().expect("thread lock is poisoned");
        if entries.len() >= CAPACITY {
            entries.clear();
        }
        entries.insert(
            conversation.to_string(),
            Entry {
                account: reply.account.to_string(),
                reply_id: reply.reply_id.to_string(),
                sent: reply.sent,
                prefix: reply.prefix,
                fingerprint: reply.fingerprint,
                beta: reply.beta,
                idle_at: now,
            },
        );
    }

    /// Drop the conversation's thread, keeping it continuable from scratch on the next turn.
    ///
    /// # Panics
    /// If a thread lock was poisoned by a panic in another request.
    pub fn forget(&self, conversation: &str) {
        self.entries
            .lock()
            .expect("thread lock is poisoned")
            .remove(conversation);
    }

    /// The conversation's endpoint has no threads. Nothing about it is kept or sent again.
    ///
    /// # Panics
    /// If a thread lock was poisoned by a panic in another request.
    pub fn block(&self, conversation: &str) {
        self.forget(conversation);
        let mut stateless = self.stateless.lock().expect("stateless lock is poisoned");
        if stateless.len() >= CAPACITY {
            stateless.clear();
        }
        stateless.insert(conversation.to_string());
    }

    /// The conversation, as it stands, for the fingerprint a `create` records: everything a
    /// thread cannot change between turns.
    #[must_use]
    pub fn fingerprint(body: &Value) -> String {
        sha256_hex(
            &Value::Array(
                [
                    "model",
                    "tools",
                    "thinking",
                    "tool_choice",
                    "output_config",
                    "mcp_servers",
                ]
                .into_iter()
                .map(|field| body.get(field).cloned().unwrap_or(Value::Null))
                .collect(),
            )
            .to_string(),
        )
    }

    /// The messages of `body` as one hash, for the prefix a `continue` must repeat.
    #[must_use]
    pub fn prefix(body: &Value, sent: usize) -> String {
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let held = messages.get(..sent).unwrap_or(messages);
        sha256_hex(&Value::Array(held.to_vec()).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn messages(n: usize) -> Vec<Value> {
        (0..n)
            .map(|turn| json!({"role": "user", "content": format!("turn {turn}")}))
            .collect()
    }

    /// A conversation after `sent` messages and the reply the caller repeats back.
    fn with_reply(sent: usize, total: usize) -> Vec<Value> {
        let mut list = messages(sent);
        list.push(json!({"role": "assistant", "content": "reply"}));
        list.extend(messages(total).into_iter().skip(sent));
        list
    }

    fn ledger_with_reply(sent: usize) -> (ThreadLedger, Instant) {
        let ledger = ThreadLedger::default();
        let now = Instant::now();
        let body = json!({"messages": messages(sent), "model": "m"});
        ledger.note_reply(
            "c",
            Reply {
                account: "a@example.com",
                reply_id: "msg_1",
                sent,
                prefix: ThreadLedger::prefix(&body, sent),
                fingerprint: ThreadLedger::fingerprint(&body),
                beta: "beta".to_string(),
            },
            now,
        );
        (ledger, now)
    }

    #[test]
    fn a_conversation_with_no_thread_creates() {
        let ledger = ThreadLedger::default();
        let body = json!({"messages": messages(2)});
        assert_eq!(
            ledger.plan(
                "c",
                "a@example.com",
                &messages(2),
                &ThreadLedger::fingerprint(&body),
                Instant::now()
            ),
            Plan::Create
        );
    }

    #[test]
    fn a_grown_conversation_continues_from_its_reply() {
        let (ledger, now) = ledger_with_reply(3);
        let body = json!({"messages": messages(5), "model": "m"});
        assert_eq!(
            ledger.plan(
                "c",
                "a@example.com",
                &with_reply(3, 5),
                &ThreadLedger::fingerprint(&body),
                now + Duration::from_secs(1)
            ),
            Plan::Continue {
                previous_message_id: "msg_1".to_string(),
                from: 4,
                beta: "beta".to_string()
            }
        );
    }

    #[test]
    fn a_reply_is_continued_once() {
        let (ledger, now) = ledger_with_reply(3);
        let body = json!({"messages": messages(5), "model": "m"});
        let fingerprint = ThreadLedger::fingerprint(&body);
        let _ = ledger.plan("c", "a@example.com", &with_reply(3, 5), &fingerprint, now);
        assert_eq!(
            ledger.plan("c", "a@example.com", &with_reply(3, 5), &fingerprint, now),
            Plan::Create
        );
    }

    #[test]
    fn a_rewritten_prefix_creates() {
        let (ledger, now) = ledger_with_reply(3);
        let mut rewritten = with_reply(3, 5);
        rewritten[1] = json!({"role": "user", "content": "edited"});
        let body = json!({"messages": messages(5), "model": "m"});
        assert_eq!(
            ledger.plan(
                "c",
                "a@example.com",
                &rewritten,
                &ThreadLedger::fingerprint(&body),
                now
            ),
            Plan::Create
        );
    }

    #[test]
    fn another_account_creates() {
        let (ledger, now) = ledger_with_reply(3);
        let body = json!({"messages": messages(5), "model": "m"});
        assert_eq!(
            ledger.plan(
                "c",
                "b@example.com",
                &with_reply(3, 5),
                &ThreadLedger::fingerprint(&body),
                now
            ),
            Plan::Create
        );
    }

    #[test]
    fn a_changed_fixed_field_creates() {
        let (ledger, now) = ledger_with_reply(3);
        let body = json!({"messages": messages(5), "model": "other"});
        assert_eq!(
            ledger.plan(
                "c",
                "a@example.com",
                &with_reply(3, 5),
                &ThreadLedger::fingerprint(&body),
                now
            ),
            Plan::Create
        );
    }

    #[test]
    fn an_idle_thread_creates() {
        let (ledger, now) = ledger_with_reply(3);
        let body = json!({"messages": messages(5), "model": "m"});
        assert_eq!(
            ledger.plan(
                "c",
                "a@example.com",
                &messages(5),
                &ThreadLedger::fingerprint(&body),
                now + IDLE
            ),
            Plan::Create
        );
    }

    #[test]
    fn a_blocked_conversation_never_continues() {
        let (ledger, now) = ledger_with_reply(3);
        ledger.block("c");
        let body = json!({"messages": messages(5), "model": "m"});
        assert_eq!(
            ledger.plan(
                "c",
                "a@example.com",
                &with_reply(3, 5),
                &ThreadLedger::fingerprint(&body),
                now
            ),
            Plan::Create
        );
    }
}
