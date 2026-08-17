//! The model catalog: which models each provider currently serves, fetched live from the
//! upstreams and used as the source of truth for `/v1/models` and for routing.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::types::ProviderKind;

/// The models a single fetch returned: anthropic and codex each give one list.
#[derive(Debug, Clone)]
pub struct FetchedModels {
    pub ids: Vec<String>,
}

impl FetchedModels {
    #[must_use]
    pub fn new(ids: Vec<String>) -> Self {
        Self { ids }
    }
}

/// A snapshot of the models each provider serves. Empty for a provider whose fetch has never
/// succeeded, so it advertises nothing and its ids fall back to the prefix heuristic.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    /// model id -> the provider serving it. anthropic and codex id namespaces do not overlap.
    direct: BTreeMap<String, ProviderKind>,
}

impl ModelCatalog {
    /// Replace a provider's entries with a freshly fetched list.
    pub fn set_direct(&mut self, kind: ProviderKind, ids: Vec<String>) {
        self.direct.retain(|_, existing| *existing != kind);
        for id in ids {
            self.direct.insert(id, kind);
        }
    }

    /// Resolve a request's model id to the provider that should serve it, or `None` when no
    /// list and no heuristic claims it (the caller then rejects the request). An explicit
    /// `<provider>/<model>` prefix wins; a bare id falls to the fetched lists then the
    /// prefix heuristic.
    #[must_use]
    pub fn resolve(&self, model: &str) -> Option<ProviderKind> {
        if let Some(kind) = provider_prefix(model) {
            return Some(kind);
        }
        if let Some(kind) = self.direct.get(model) {
            return Some(*kind);
        }
        heuristic_provider(model)
    }

    /// The advertised catalog for `/v1/models`: every id carries its `<provider>/<model>`
    /// prefix so a client can address a provider unambiguously even when ids overlap.
    #[must_use]
    pub fn advertised(&self) -> Vec<(String, ProviderKind)> {
        self.direct
            .iter()
            .map(|(id, kind)| (format!("{}/{id}", kind.canonical_id()), *kind))
            .collect()
    }
}

/// Strip a leading `<provider>/` from a model id so the bare id goes upstream. Only known
/// provider prefixes are stripped; an unrelated id containing `/` is left intact.
#[must_use]
pub fn upstream_model(model: &str) -> &str {
    provider_prefix(model).map_or(model, |kind| &model[kind.canonical_id().len() + 1..])
}

/// The provider named by an explicit `<provider>/` prefix, if any.
fn provider_prefix(model: &str) -> Option<ProviderKind> {
    let prefix = model.split_once('/')?.0;
    match prefix {
        "anthropic" => Some(ProviderKind::Anthropic),
        "codex" => Some(ProviderKind::Codex),
        _ => None,
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
    use super::{ModelCatalog, parse_anthropic, parse_codex};
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
    }

    #[test]
    fn resolves_by_fetched_list_then_prefix_then_heuristic() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-opus-5".into()]);
        catalog.set_direct(ProviderKind::Codex, vec!["gpt-5.5".into()]);

        // fetched lists win for bare ids
        assert_eq!(
            catalog.resolve("claude-opus-5"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(catalog.resolve("gpt-5.5"), Some(ProviderKind::Codex));
        // an explicit <provider>/ prefix always wins
        assert_eq!(
            catalog.resolve("anthropic/claude-opus-5"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(catalog.resolve("codex/gpt-5.5"), Some(ProviderKind::Codex));
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
        // a removed provider's prefix is claimed by nobody
        assert_eq!(catalog.resolve("opencode/glm-5.2"), None);
    }

    #[test]
    fn advertised_prefixes_every_id_with_its_provider() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-opus-5".into()]);
        catalog.set_direct(ProviderKind::Codex, vec!["gpt-5.5".into()]);
        let advertised = catalog.advertised();
        assert!(advertised.contains(&(
            "anthropic/claude-opus-5".to_string(),
            ProviderKind::Anthropic
        )));
        assert!(advertised.contains(&("codex/gpt-5.5".to_string(), ProviderKind::Codex)));
    }

    #[test]
    fn upstream_model_strips_only_known_provider_prefixes() {
        assert_eq!(
            super::upstream_model("anthropic/claude-opus-5"),
            "claude-opus-5"
        );
        assert_eq!(super::upstream_model("codex/gpt-5.5"), "gpt-5.5");
        // bare ids and unrelated slashes pass through untouched
        assert_eq!(super::upstream_model("claude-opus-5"), "claude-opus-5");
        assert_eq!(super::upstream_model("vendor/weird-id"), "vendor/weird-id");
    }

    #[test]
    fn refetch_replaces_a_providers_entries() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-old".into()]);
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-new".into()]);
        assert_eq!(catalog.resolve("claude-new"), Some(ProviderKind::Anthropic));
        assert!(
            !catalog
                .advertised()
                .iter()
                .any(|(id, _)| id == "anthropic/claude-old")
        );
    }
}
