//! The model catalog: which models each provider currently serves, fetched live from the
//! upstreams and used as the source of truth for `/v1/models` and for routing.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config::ConfiguredProvider;
use crate::types::{ProviderId, ProviderKind};

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
    /// configured provider id -> the model ids its `/v1/models` returned. Only
    /// advertised; bare ids never route to generic providers (prefix-only).
    generic: BTreeMap<String, Vec<String>>,
}

impl ModelCatalog {
    /// Replace a provider's entries with a freshly fetched list.
    pub fn set_direct(&mut self, kind: ProviderKind, ids: Vec<String>) {
        self.direct.retain(|_, existing| *existing != kind);
        for id in ids {
            self.direct.insert(id, kind);
        }
    }

    /// Replace a configured provider's advertised entries with a fresh fetch.
    pub fn set_generic(&mut self, provider_id: &str, ids: Vec<String>) {
        self.generic.insert(provider_id.to_string(), ids);
    }

    /// Resolve a request's model id to the provider that should serve it, or `None` when no
    /// list and no heuristic claims it (the caller then rejects the request). An explicit
    /// `<provider>/<model>` prefix wins — including configured `providers:` entries, which
    /// resolve to a `Generic` `ProviderId` named by the prefix; a bare id falls to the fetched
    /// lists then the prefix heuristic.
    #[must_use]
    pub fn resolve_id(
        &self,
        model: &str,
        providers: &BTreeMap<String, ConfiguredProvider>,
    ) -> Option<ProviderId> {
        if let Some((prefix, _)) = model.split_once('/') {
            match prefix {
                "anthropic" => return Some(ProviderId::anthropic()),
                "codex" => return Some(ProviderId::codex()),
                other if providers.contains_key(other) => {
                    return Some(ProviderId::generic(other));
                }
                _ => {}
            }
        }
        self.direct
            .get(model)
            .copied()
            .map(ProviderId::for_kind)
            .or_else(|| heuristic_provider(model).map(ProviderId::for_kind))
    }

    /// The advertised catalog for `/v1/models`: every id carries its `<provider>/<model>`
    /// prefix so a client can address a provider unambiguously even when ids overlap.
    #[must_use]
    pub fn advertised(&self) -> Vec<(String, ProviderId)> {
        let mut out = self
            .direct
            .iter()
            .map(|(id, kind)| {
                (
                    format!("{}/{id}", kind.canonical_id()),
                    ProviderId::for_kind(*kind),
                )
            })
            .collect::<Vec<_>>();
        for (provider_id, ids) in &self.generic {
            for id in ids {
                out.push((
                    format!("{provider_id}/{id}"),
                    ProviderId::generic(provider_id.clone()),
                ));
            }
        }
        out
    }
}

/// Strip a leading `<provider>/` from a model id so the bare id goes upstream. The prefix
/// must be the resolved provider's own id (`anthropic`, `codex`, or the `providers:` entry
/// name); an unrelated id containing `/` is left intact.
#[must_use]
pub fn upstream_model<'a>(model: &'a str, provider: &ProviderId) -> &'a str {
    let prefix = provider.id.as_ref();
    model
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(model)
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

/// Model ids from an OpenAI-style `/models` body (`{"data": [{"id": ...}]}`), the
/// shape OpenAI-compatible endpoints return.
#[must_use]
pub fn parse_openai(body: &Value) -> Vec<String> {
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
    use super::{ModelCatalog, parse_anthropic, parse_codex};
    use crate::types::{ProviderId, ProviderKind};
    use serde_json::json;
    use std::collections::BTreeMap;

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
        let providers = BTreeMap::new();

        // fetched lists win for bare ids
        assert_eq!(
            catalog.resolve_id("claude-opus-5", &providers),
            Some(ProviderId::anthropic())
        );
        assert_eq!(
            catalog.resolve_id("gpt-5.5", &providers),
            Some(ProviderId::codex())
        );
        // an explicit <provider>/ prefix always wins
        assert_eq!(
            catalog.resolve_id("anthropic/claude-opus-5", &providers),
            Some(ProviderId::anthropic())
        );
        assert_eq!(
            catalog.resolve_id("codex/gpt-5.5", &providers),
            Some(ProviderId::codex())
        );
        // an id in no list still routes by shape (new families covered)
        assert_eq!(
            catalog.resolve_id("gpt-6", &providers),
            Some(ProviderId::codex())
        );
        assert_eq!(
            catalog.resolve_id("claude-opus-9", &providers),
            Some(ProviderId::anthropic())
        );
        assert_eq!(
            catalog.resolve_id("o5", &providers),
            Some(ProviderId::codex())
        );
        // a dropped alias / unknown id is claimed by nobody
        assert_eq!(catalog.resolve_id("opus", &providers), None);
        assert_eq!(catalog.resolve_id("gemini-3", &providers), None);
        // a removed provider's prefix is claimed by nobody
        assert_eq!(catalog.resolve_id("opencode/glm-5.2", &providers), None);
    }

    #[test]
    fn resolves_configured_provider_prefixes_to_a_generic_id() {
        let catalog = ModelCatalog::default();
        let providers = BTreeMap::from([(
            "groq".to_string(),
            crate::config::ConfiguredProvider {
                base_url: "https://api.groq.com/openai/v1".to_string(),
            },
        )]);

        assert_eq!(
            catalog.resolve_id("groq/llama-3.3-70b", &providers),
            Some(ProviderId::generic("groq"))
        );
        // a configured prefix without a matching entry is claimed by nobody
        assert_eq!(catalog.resolve_id("mistral/large", &providers), None);
        // bare ids never route to configured providers, even for their models
        assert_eq!(catalog.resolve_id("llama-3.3-70b", &providers), None);
    }

    #[test]
    fn advertised_prefixes_every_id_with_its_provider() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-opus-5".into()]);
        catalog.set_direct(ProviderKind::Codex, vec!["gpt-5.5".into()]);
        let advertised = catalog.advertised();
        assert!(advertised.contains(&(
            "anthropic/claude-opus-5".to_string(),
            ProviderId::anthropic()
        )));
        assert!(advertised.contains(&("codex/gpt-5.5".to_string(), ProviderId::codex())));
    }

    #[test]
    fn upstream_model_strips_the_resolved_provider_prefix() {
        assert_eq!(
            super::upstream_model("anthropic/claude-opus-5", &ProviderId::anthropic()),
            "claude-opus-5"
        );
        assert_eq!(
            super::upstream_model("codex/gpt-5.5", &ProviderId::codex()),
            "gpt-5.5"
        );
        assert_eq!(
            super::upstream_model("groq/llama-3.3-70b", &ProviderId::generic("groq")),
            "llama-3.3-70b"
        );
        // bare ids and unrelated slashes pass through untouched
        assert_eq!(
            super::upstream_model("claude-opus-5", &ProviderId::anthropic()),
            "claude-opus-5"
        );
        assert_eq!(
            super::upstream_model("vendor/weird-id", &ProviderId::anthropic()),
            "vendor/weird-id"
        );
    }

    #[test]
    fn refetch_replaces_a_providers_entries() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-old".into()]);
        catalog.set_direct(ProviderKind::Anthropic, vec!["claude-new".into()]);
        let providers = BTreeMap::new();
        assert_eq!(
            catalog.resolve_id("claude-new", &providers),
            Some(ProviderId::anthropic())
        );
        assert!(
            !catalog
                .advertised()
                .iter()
                .any(|(id, _)| id == "anthropic/claude-old")
        );
    }
}
