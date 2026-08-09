//! The model catalog: which models each provider currently serves, fetched live from the
//! upstreams and used as the source of truth for `/v1/models` and for routing.
//!
//! opencode stays prefix-addressed (`opencode/<id>`) because its credits list overlaps the
//! ids anthropic and codex serve (`claude-opus-5`, `gpt-5.6-*`); the prefix disambiguates,
//! the catalog supplies the list and the go-vs-credits base.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::providers::OPENCODE_PREFIX;
use crate::types::ProviderKind;

/// Which opencode upstream serves a model. go is the subscription plan, credits the spillover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpencodeBase {
    Go,
    Credits,
}

/// A snapshot of the models each provider serves. Empty for a provider whose fetch has never
/// succeeded, so it advertises nothing and its ids fall back to the prefix heuristic.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    /// anthropic and codex model id -> its provider. Their id namespaces do not overlap.
    direct: BTreeMap<String, ProviderKind>,
    /// opencode model id (unprefixed) -> the base that serves it.
    opencode: BTreeMap<String, OpencodeBase>,
}

impl ModelCatalog {
    /// Replace the anthropic or codex entries with a freshly fetched list.
    pub fn set_direct(&mut self, kind: ProviderKind, ids: Vec<String>) {
        self.direct.retain(|_, existing| *existing != kind);
        for id in ids {
            self.direct.insert(id, kind);
        }
    }

    /// Replace the opencode entries. A model in both lists is served from go (subscription
    /// first); a model only in credits is served from credits.
    pub fn set_opencode(&mut self, go_ids: Vec<String>, credits_ids: Vec<String>) {
        self.opencode.clear();
        for id in credits_ids {
            self.opencode.insert(id, OpencodeBase::Credits);
        }
        for id in go_ids {
            self.opencode.insert(id, OpencodeBase::Go);
        }
    }

    /// Resolve a request's model id to the provider that should serve it, or `None` when no
    /// list and no heuristic claims it (the caller then rejects the request).
    #[must_use]
    pub fn resolve(&self, model: &str) -> Option<ProviderKind> {
        if model.starts_with(OPENCODE_PREFIX) {
            return Some(ProviderKind::Opencode);
        }
        if let Some(kind) = self.direct.get(model) {
            return Some(*kind);
        }
        heuristic_provider(model)
    }

    /// The opencode base for an unprefixed model id; go unless the catalog says credits-only.
    #[must_use]
    pub fn opencode_base(&self, model: &str) -> OpencodeBase {
        self.opencode
            .get(model)
            .copied()
            .unwrap_or(OpencodeBase::Go)
    }

    /// The advertised catalog for `/v1/models`: anthropic and codex ids bare, opencode ids
    /// carrying the `opencode/` prefix a client must send.
    #[must_use]
    pub fn advertised(&self) -> Vec<(String, ProviderKind)> {
        let mut out: Vec<(String, ProviderKind)> = self
            .direct
            .iter()
            .map(|(id, kind)| (id.clone(), *kind))
            .collect();
        out.extend(
            self.opencode
                .keys()
                .map(|id| (format!("{OPENCODE_PREFIX}{id}"), ProviderKind::Opencode)),
        );
        out
    }
}

/// Route an id no fetched list claims, by name shape. Broad on purpose: a new `gpt-*` or
/// `o<N>` family routes to codex without a code change; `claude-*` to anthropic.
fn heuristic_provider(model: &str) -> Option<ProviderKind> {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude-") || lower.starts_with("anthropic") {
        return Some(ProviderKind::Anthropic);
    }
    let codex = lower.starts_with("gpt-")
        || lower.starts_with("codex-")
        || lower
            .strip_prefix('o')
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()));
    codex.then_some(ProviderKind::Codex)
}

/// Model ids from an Anthropic `/v1/models` body (`{"data": [{"id": ...}]}`).
#[must_use]
pub fn parse_anthropic(body: &Value) -> Vec<String> {
    ids_from(body.get("data"), "id")
}

/// Model ids from a Codex `/codex/models` body (`{"models": [{"slug": ...}]}`).
#[must_use]
pub fn parse_codex(body: &Value) -> Vec<String> {
    ids_from(body.get("models"), "slug")
}

/// Model ids from an opencode `/models` body (`{"data": [{"id": ...}]}`).
#[must_use]
pub fn parse_opencode(body: &Value) -> Vec<String> {
    ids_from(body.get("data"), "id")
}

fn ids_from(array: Option<&Value>, field: &str) -> Vec<String> {
    array
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get(field).and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{ModelCatalog, OpencodeBase, parse_anthropic, parse_codex, parse_opencode};
    use crate::types::ProviderKind;
    use serde_json::json;

    #[test]
    fn parses_each_provider_body_shape() {
        assert_eq!(
            parse_anthropic(&json!({"data": [{"id": "claude-opus-5"}, {"id": "claude-sonnet-5"}]})),
            vec!["claude-opus-5", "claude-sonnet-5"]
        );
        assert_eq!(
            parse_codex(&json!({"models": [{"slug": "gpt-5.5"}, {"slug": "gpt-5.4"}]})),
            vec!["gpt-5.5", "gpt-5.4"]
        );
        assert_eq!(
            parse_opencode(&json!({"data": [{"id": "glm-5.2"}, {"id": "kimi-k3"}]})),
            vec!["glm-5.2", "kimi-k3"]
        );
    }

    #[test]
    fn resolves_by_fetched_list_then_prefix_then_heuristic() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-opus-5".into()]);
        catalog.set_direct(ProviderKind::Codex, vec!["gpt-5.5".into()]);
        catalog.set_opencode(vec!["glm-5.2".into()], vec!["claude-opus-5".into()]);

        // fetched lists win for bare ids
        assert_eq!(
            catalog.resolve("claude-opus-5"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(catalog.resolve("gpt-5.5"), Some(ProviderKind::Codex));
        // the opencode/ prefix always routes to opencode, even for an overlapping id
        assert_eq!(
            catalog.resolve("opencode/claude-opus-5"),
            Some(ProviderKind::Opencode)
        );
        // an id in no list still routes by shape (new families covered)
        assert_eq!(catalog.resolve("gpt-6"), Some(ProviderKind::Codex));
        assert_eq!(
            catalog.resolve("claude-opus-9"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(catalog.resolve("o5"), Some(ProviderKind::Codex));
        // a dropped alias / unknown id is claimed by nobody
        assert_eq!(catalog.resolve("opus"), None);
        assert_eq!(catalog.resolve("gemini-3"), None);
    }

    #[test]
    fn opencode_base_prefers_go_when_in_both() {
        let mut catalog = ModelCatalog::default();
        catalog.set_opencode(
            vec!["glm-5.2".into()],
            vec!["glm-5.2".into(), "grok-4.5".into()],
        );
        assert_eq!(catalog.opencode_base("glm-5.2"), OpencodeBase::Go);
        assert_eq!(catalog.opencode_base("grok-4.5"), OpencodeBase::Credits);
        // unknown opencode id defaults to go
        assert_eq!(catalog.opencode_base("mystery"), OpencodeBase::Go);
    }

    #[test]
    fn advertised_prefixes_opencode_ids_only() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-opus-5".into()]);
        catalog.set_opencode(vec!["glm-5.2".into()], vec![]);
        let advertised = catalog.advertised();
        assert!(advertised.contains(&("claude-opus-5".to_string(), ProviderKind::Anthropic)));
        assert!(advertised.contains(&("opencode/glm-5.2".to_string(), ProviderKind::Opencode)));
    }

    #[test]
    fn refetch_replaces_a_providers_entries() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-old".into()]);
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-new".into()]);
        assert_eq!(catalog.resolve("claude-new"), Some(ProviderKind::Anthropic));
        // the stale id is gone from the list (heuristic no longer claims a non-claude id)
        assert!(
            !catalog
                .advertised()
                .iter()
                .any(|(id, _)| id == "claude-old")
        );
    }
}
