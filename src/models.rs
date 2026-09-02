//! The model catalog: which models each provider currently serves, fetched live from the
//! upstreams and used as the source of truth for `/v1/models` and for routing.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::config::ConfiguredProvider;
use crate::types::{ProviderId, ProviderKind};

/// What a model costs, in USD per million tokens. Every field is optional: an upstream
/// that publishes only some rates leaves the rest out rather than guessing.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelPricing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_per_million: Option<f64>,
}

/// Per-model facts a client needs to size requests and costs: context limits, output
/// limits, accepted input kinds and pricing. All optional and omitted from `/v1/models`
/// when unknown, so an id-only client is unaffected.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
}

impl ModelMetadata {
    /// The metadata one upstream `/v1/models` entry carries, under the field names
    /// pengepul publishes (`context_window`, `context_length`, `max_output_tokens`,
    /// `input_modalities`, `pricing` with the per-million keys). `None` when the entry
    /// carries none of them, so pass-through stays silent instead of inventing zeros.
    #[must_use]
    pub fn from_json(entry: &Value) -> Option<Self> {
        let context_window = entry
            .get("context_window")
            .or_else(|| entry.get("context_length"))
            .and_then(Value::as_u64);
        let max_output_tokens = entry.get("max_output_tokens").and_then(Value::as_u64);
        let input_modalities =
            entry
                .get("input_modalities")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                });
        let pricing = entry
            .get("pricing")
            .and_then(Value::as_object)
            .map(|rates| ModelPricing {
                input_per_million: rates.get("input_per_million").and_then(number_from),
                output_per_million: rates.get("output_per_million").and_then(number_from),
                cache_read_per_million: rates.get("cache_read_per_million").and_then(number_from),
                cache_write_per_million: rates.get("cache_write_per_million").and_then(number_from),
            });
        let metadata = Self {
            context_window,
            max_output_tokens,
            input_modalities,
            pricing,
        };
        (metadata != Self::default()).then_some(metadata)
    }
}

/// A JSON number, or a string holding one (some endpoints quote their rates).
fn number_from(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

/// Per-model metadata for the direct anthropic and codex catalogs. The anthropic upstream
/// advertises only ids, so these numbers come from the vendor's published model docs
/// (see `docs/research/claude-model-metadata.md`); a model no entry claims is advertised
/// without metadata rather than with guessed ones. Ordered longest-prefix first, so
/// `claude-fable-5-1` wins over the `claude-fable` family default and dated aliases
/// (`claude-opus-5-20260101`) match their family.
fn curated_metadata(id: &str) -> Option<ModelMetadata> {
    /// (context window, max output, input $/M, output $/M, cache-read $/M, cache-write $/M).
    /// `None` cache-write: the vendor does not bill a separate write rate for the family.
    const GPT5_LIMITS: (u64, u64, f64, f64, f64, Option<f64>) =
        (400_000, 128_000, 1.25, 10.0, 0.125, None);
    let entries: &[(&str, ModelMetadata)] = &[
        // fable-5-1's cache read is 0.025x base input, not the standard 0.1x.
        (
            "claude-fable-5-1",
            meta(1_000_000, 128_000, 10.0, 50.0, 0.25, Some(12.5)),
        ),
        (
            "claude-fable",
            meta(1_000_000, 128_000, 10.0, 50.0, 1.0, Some(12.5)),
        ),
        (
            "claude-opus-5",
            meta(1_000_000, 128_000, 5.0, 25.0, 0.5, Some(6.25)),
        ),
        (
            "claude-opus-4-8",
            meta(1_000_000, 128_000, 5.0, 25.0, 0.5, Some(6.25)),
        ),
        (
            "claude-opus-4-7",
            meta(1_000_000, 128_000, 5.0, 25.0, 0.5, Some(6.25)),
        ),
        (
            "claude-opus-4-6",
            meta(1_000_000, 128_000, 5.0, 25.0, 0.5, Some(6.25)),
        ),
        (
            "claude-opus-4",
            meta(200_000, 64_000, 5.0, 25.0, 0.5, Some(6.25)),
        ),
        (
            "claude-sonnet-5",
            meta(1_000_000, 128_000, 2.0, 10.0, 0.2, Some(2.5)),
        ),
        (
            "claude-sonnet-4-6",
            meta(1_000_000, 128_000, 3.0, 15.0, 0.3, Some(3.75)),
        ),
        (
            "claude-sonnet-4",
            meta(200_000, 64_000, 3.0, 15.0, 0.3, Some(3.75)),
        ),
        (
            "claude-haiku-4-5",
            meta(200_000, 64_000, 1.0, 5.0, 0.1, Some(1.25)),
        ),
        ("gpt-5.5", meta_rated(GPT5_LIMITS)),
        ("gpt-5", meta_rated(GPT5_LIMITS)),
        ("codex-", meta_rated(GPT5_LIMITS)),
    ];
    entries
        .iter()
        .find(|(prefix, _)| id.starts_with(prefix))
        .map(|(_, metadata)| metadata.clone())
}

/// A text-and-image model from a `(window, output, input $/M, output $/M, cache-read $/M,
/// cache-write $/M)` tuple.
fn meta_rated(
    (context_window, max_output_tokens, input, output, cache_read, cache_write): (
        u64,
        u64,
        f64,
        f64,
        f64,
        Option<f64>,
    ),
) -> ModelMetadata {
    meta(
        context_window,
        max_output_tokens,
        input,
        output,
        cache_read,
        cache_write,
    )
}

/// A text-and-image model with the given window, output cap and per-million rates.
fn meta(
    context_window: u64,
    max_output_tokens: u64,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: Option<f64>,
) -> ModelMetadata {
    ModelMetadata {
        context_window: Some(context_window),
        max_output_tokens: Some(max_output_tokens),
        input_modalities: Some(vec!["text".to_string(), "image".to_string()]),
        pricing: Some(ModelPricing {
            input_per_million: Some(input),
            output_per_million: Some(output),
            cache_read_per_million: Some(cache_read),
            cache_write_per_million: cache_write,
        }),
    }
}

/// The models a single fetch returned: anthropic and codex each give one list, plus any
/// per-model metadata the upstream body or the curated table carries for those ids.
#[derive(Debug, Clone)]
pub struct FetchedModels {
    pub ids: Vec<String>,
    pub metadata: BTreeMap<String, ModelMetadata>,
}

impl FetchedModels {
    #[must_use]
    pub fn new(ids: Vec<String>) -> Self {
        Self {
            ids,
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_metadata(ids: Vec<String>, metadata: BTreeMap<String, ModelMetadata>) -> Self {
        Self { ids, metadata }
    }
}

/// One model in the advertised catalog: its `<provider>/<model>` id, the provider that
/// serves it, and any per-model metadata known for it.
#[derive(Debug, Clone)]
pub struct AdvertisedModel {
    pub id: String,
    pub provider: ProviderId,
    pub metadata: Option<ModelMetadata>,
}

/// One direct (anthropic or codex) catalog entry.
#[derive(Debug, Clone)]
struct DirectModel {
    kind: ProviderKind,
    metadata: Option<ModelMetadata>,
}

/// One configured-provider catalog entry.
#[derive(Debug, Clone)]
struct GenericModel {
    id: String,
    metadata: Option<ModelMetadata>,
}

/// A snapshot of the models each provider serves. Empty for a provider whose fetch has never
/// succeeded, so it advertises nothing and its ids fall back to the prefix heuristic.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    /// model id -> the provider serving it. anthropic and codex id namespaces do not overlap.
    direct: BTreeMap<String, DirectModel>,
    /// configured provider id -> the model ids its `/v1/models` returned. Only
    /// advertised; bare ids never route to generic providers (prefix-only).
    generic: BTreeMap<String, Vec<GenericModel>>,
}

impl ModelCatalog {
    /// Replace a provider's entries with a freshly fetched list.
    pub fn set_direct(&mut self, kind: ProviderKind, models: FetchedModels) {
        self.direct.retain(|_, existing| existing.kind != kind);
        for id in models.ids {
            let metadata = models.metadata.get(&id).cloned();
            self.direct.insert(id, DirectModel { kind, metadata });
        }
    }

    /// Replace a configured provider's advertised entries with a fresh fetch.
    pub fn set_generic(&mut self, provider_id: &str, models: FetchedModels) {
        let entries = models
            .ids
            .into_iter()
            .map(|id| GenericModel {
                metadata: models.metadata.get(&id).cloned(),
                id,
            })
            .collect();
        self.generic.insert(provider_id.to_string(), entries);
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
            .map(|entry| ProviderId::for_kind(entry.kind))
            .or_else(|| heuristic_provider(model).map(ProviderId::for_kind))
    }

    /// The advertised catalog for `/v1/models`: every id carries its `<provider>/<model>`
    /// prefix so a client can address a provider unambiguously even when ids overlap.
    #[must_use]
    pub fn advertised(&self) -> Vec<AdvertisedModel> {
        let mut out = self
            .direct
            .iter()
            .map(|(id, model)| AdvertisedModel {
                id: format!("{}/{id}", model.kind.canonical_id()),
                provider: ProviderId::for_kind(model.kind),
                metadata: model.metadata.clone(),
            })
            .collect::<Vec<_>>();
        for (provider_id, models) in &self.generic {
            for model in models {
                out.push(AdvertisedModel {
                    id: format!("{provider_id}/{}", model.id),
                    provider: ProviderId::generic(provider_id.clone()),
                    metadata: model.metadata.clone(),
                });
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

/// Models from an Anthropic `/v1/models` body (`{"data": [{"id": ...}]}`). The upstream
/// carries ids only, so metadata comes from the curated table where one claims the id.
#[must_use]
pub fn parse_anthropic(body: &Value) -> FetchedModels {
    let ids = ids_from(body.get("data"), "id");
    let metadata = ids
        .iter()
        .filter_map(|id| curated_metadata(id).map(|meta| (id.clone(), meta)))
        .collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// Models from a Codex `/codex/models` body (`{"models": [{"slug": ...}]}`). Per-entry
/// fields the upstream publishes are kept; anything it leaves out falls to the curated
/// table.
#[must_use]
pub fn parse_codex(body: &Value) -> FetchedModels {
    let entries = body
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids = entries
        .iter()
        .filter_map(|entry| entry.get("slug").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let metadata = ids
        .iter()
        .zip(entries.iter())
        .filter_map(|(id, entry)| {
            ModelMetadata::from_json(entry)
                .or_else(|| curated_metadata(id))
                .map(|meta| (id.clone(), meta))
        })
        .collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// Models from an OpenAI-style `/models` body (`{"data": [{"id": ...}]}`), the shape
/// OpenAI-compatible endpoints return. Whatever per-model metadata an endpoint publishes
/// passes through; one that publishes none stays metadata-free.
#[must_use]
pub fn parse_openai(body: &Value) -> FetchedModels {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids = entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let metadata = ids
        .iter()
        .zip(entries.iter())
        .filter_map(|(id, entry)| ModelMetadata::from_json(entry).map(|meta| (id.clone(), meta)))
        .collect();
    FetchedModels::with_metadata(ids, metadata)
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
    use super::{FetchedModels, ModelCatalog, parse_anthropic, parse_codex, parse_openai};
    use crate::types::{ProviderId, ProviderKind};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn parses_each_provider_body_shape() {
        assert_eq!(
            parse_anthropic(&json!({"data": [{"id": "claude-opus-5"}, {"id": "claude-sonnet-5"}]}))
                .ids,
            vec!["claude-opus-5", "claude-sonnet-5"]
        );
        assert_eq!(
            parse_codex(&json!({"models": [{"slug": "gpt-5.5"}, {"slug": "gpt-5.4"}]})).ids,
            vec!["gpt-5.5", "gpt-5.4"]
        );
        assert_eq!(
            parse_openai(&json!({"data": [{"id": "llama-3.3-70b"}]})).ids,
            vec!["llama-3.3-70b"]
        );
    }

    #[test]
    fn anthropic_ids_get_curated_metadata() {
        let fetched = parse_anthropic(&json!({"data": [
            {"id": "claude-fable-5-1"}, {"id": "claude-opus-5"}, {"id": "claude-opus-9"}
        ]}));
        assert_eq!(
            fetched.ids,
            vec!["claude-fable-5-1", "claude-opus-5", "claude-opus-9"]
        );
        let fable_meta = fetched
            .metadata
            .get("claude-fable-5-1")
            .expect("fable metadata");
        assert_eq!(fable_meta.context_window, Some(1_000_000));
        assert_eq!(fable_meta.max_output_tokens, Some(128_000));
        assert_eq!(
            fable_meta.input_modalities.as_deref(),
            Some(["text".to_string(), "image".to_string()].as_slice())
        );
        let pricing = fable_meta.pricing.as_ref().expect("fable pricing");
        assert_eq!(pricing.input_per_million, Some(10.0));
        assert_eq!(pricing.output_per_million, Some(50.0));
        // fable-5-1's cache read is 0.025x base input; every other family pays 0.1x
        assert_eq!(pricing.cache_read_per_million, Some(0.25));
        assert_eq!(pricing.cache_write_per_million, Some(12.5));
        // opus-5: 1M/128K at $5/$25 — the pre-4.1 opus rates do not carry forward
        let opus = fetched
            .metadata
            .get("claude-opus-5")
            .expect("opus metadata");
        assert_eq!(opus.context_window, Some(1_000_000));
        assert_eq!(opus.max_output_tokens, Some(128_000));
        let opus_pricing = opus.pricing.as_ref().expect("opus pricing");
        assert_eq!(opus_pricing.input_per_million, Some(5.0));
        assert_eq!(opus_pricing.output_per_million, Some(25.0));
        assert_eq!(opus_pricing.cache_read_per_million, Some(0.5));
        assert_eq!(opus_pricing.cache_write_per_million, Some(6.25));
        // the 4.x models the upstream still serves are covered too
        let fetched_4x = parse_anthropic(&json!({"data": [
            {"id": "claude-opus-4-8"}, {"id": "claude-opus-4-5"},
            {"id": "claude-sonnet-4-6"}, {"id": "claude-sonnet-4-5"}
        ]}));
        let window = |id: &str| fetched_4x.metadata.get(id).and_then(|m| m.context_window);
        assert_eq!(window("claude-opus-4-8"), Some(1_000_000));
        assert_eq!(window("claude-opus-4-5"), Some(200_000));
        assert_eq!(window("claude-sonnet-4-6"), Some(1_000_000));
        assert_eq!(window("claude-sonnet-4-5"), Some(200_000));
        // a model no curated entry claims stays metadata-free, not guessed
        assert!(!fetched.metadata.contains_key("claude-opus-9"));
    }

    #[test]
    fn generic_metadata_passes_through_only_when_upstream_publishes_it() {
        let body = json!({"data": [
            {"id": "big", "context_length": 131_072, "max_output_tokens": 8_192,
             "input_modalities": ["text"],
             "pricing": {"input_per_million": 0.5, "output_per_million": "1.5"}},
            {"id": "plain"}
        ]});
        let fetched = parse_openai(&body);
        assert_eq!(fetched.ids, vec!["big", "plain"]);
        let big = fetched.metadata.get("big").expect("big metadata");
        assert_eq!(big.context_window, Some(131_072));
        assert_eq!(big.max_output_tokens, Some(8_192));
        assert_eq!(
            big.input_modalities.as_deref(),
            Some(["text".to_string()].as_slice())
        );
        assert_eq!(
            big.pricing.as_ref().and_then(|p| p.output_per_million),
            Some(1.5)
        );
        // an endpoint that publishes nothing stays silent
        assert!(!fetched.metadata.contains_key("plain"));
    }

    #[test]
    fn codex_body_metadata_wins_over_the_curated_table() {
        let fetched = parse_codex(&json!({"models": [
            {"slug": "gpt-5.5", "context_window": 272_000}
        ]}));
        assert_eq!(
            fetched
                .metadata
                .get("gpt-5.5")
                .and_then(|m| m.context_window),
            Some(272_000)
        );
    }

    #[test]
    fn resolves_by_fetched_list_then_prefix_then_heuristic() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(vec!["claude-opus-5".into()]),
        );
        catalog.set_direct(
            ProviderKind::Codex,
            FetchedModels::new(vec!["gpt-5.5".into()]),
        );
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
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(vec!["claude-opus-5".into()]),
        );
        catalog.set_direct(
            ProviderKind::Codex,
            FetchedModels::new(vec!["gpt-5.5".into()]),
        );
        let advertised = catalog.advertised();
        assert!(advertised.iter().any(|m| m.id == "anthropic/claude-opus-5"
            && m.provider == ProviderId::anthropic()));
        assert!(
            advertised
                .iter()
                .any(|m| m.id == "codex/gpt-5.5" && m.provider == ProviderId::codex())
        );
    }

    #[test]
    fn advertised_carries_each_models_metadata() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(
            ProviderKind::Anthropic,
            parse_anthropic(&json!({"data": [
                {"id": "claude-fable-5-1"}
            ]})),
        );
        catalog.set_generic(
            "groq",
            parse_openai(&json!({"data": [
                {"id": "llama-3.3-70b", "context_window": 131_072}
            ]})),
        );
        let advertised = catalog.advertised();
        let fable = advertised
            .iter()
            .find(|m| m.id == "anthropic/claude-fable-5-1")
            .expect("fable advertised");
        assert_eq!(
            fable.metadata.as_ref().and_then(|m| m.context_window),
            Some(1_000_000)
        );
        let llama = advertised
            .iter()
            .find(|m| m.id == "groq/llama-3.3-70b")
            .expect("llama advertised");
        assert_eq!(
            llama.metadata.as_ref().and_then(|m| m.context_window),
            Some(131_072)
        );
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
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(vec!["claude-old".into()]),
        );
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(vec!["claude-new".into()]),
        );
        let providers = BTreeMap::new();
        assert_eq!(
            catalog.resolve_id("claude-new", &providers),
            Some(ProviderId::anthropic())
        );
        assert!(
            !catalog
                .advertised()
                .iter()
                .any(|model| model.id == "anthropic/claude-old")
        );
    }
}
