//! The model catalog: which models each provider currently serves, fetched live from the
//! upstreams and used as the source of truth for `/v1/models` and for routing.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{AllowLists, ConfiguredModel, ConfiguredProvider};
use crate::types::{ProviderId, ProviderKind};

/// What a model costs, in USD per million tokens. Every field is optional: an upstream
/// that publishes only some rates leaves the rest out rather than guessing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
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
/// limits, accepted input kinds, pricing, and whether the model reasons. All optional and
/// omitted from `/v1/models` when unknown, so an id-only client is unaffected.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
}

impl ModelMetadata {
    /// The metadata one upstream `/models` entry carries: `context_window`, `context_length` or
    /// vLLM's `max_model_len` (which omlx publishes) for the context window, `max_output_tokens`,
    /// `input_modalities`, `reasoning`, and `pricing` with the per-million keys. `None` when the
    /// entry carries none of them, so pass-through stays silent instead of inventing zeros.
    #[must_use]
    pub fn from_json(entry: &Value) -> Option<Self> {
        let context_window = entry
            .get("context_window")
            .or_else(|| entry.get("context_length"))
            .or_else(|| entry.get("max_model_len"))
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
        let reasoning = entry.get("reasoning").and_then(Value::as_bool);
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
            reasoning,
            pricing,
        };
        (metadata != Self::default()).then_some(metadata)
    }

    /// Field-wise merge: every field `overrides` publishes wins, `self` fills the gaps.
    /// A curated base plus a partial upstream body yields full coverage without either
    /// side inventing values.
    #[must_use]
    pub fn merged_with(self, overrides: ModelMetadata) -> Self {
        let pricing = match (self.pricing, overrides.pricing) {
            (Some(base), Some(rates)) => Some(base.merged_with(rates)),
            (base, rates) => rates.or(base),
        };
        Self {
            context_window: overrides.context_window.or(self.context_window),
            max_output_tokens: overrides.max_output_tokens.or(self.max_output_tokens),
            input_modalities: overrides.input_modalities.or(self.input_modalities),
            reasoning: overrides.reasoning.or(self.reasoning),
            pricing,
        }
    }
}

impl ModelPricing {
    /// Field-wise merge: every rate `overrides` publishes wins, `self` fills the gaps.
    #[must_use]
    pub fn merged_with(self, overrides: ModelPricing) -> Self {
        Self {
            input_per_million: overrides.input_per_million.or(self.input_per_million),
            output_per_million: overrides.output_per_million.or(self.output_per_million),
            cache_read_per_million: overrides
                .cache_read_per_million
                .or(self.cache_read_per_million),
            cache_write_per_million: overrides
                .cache_write_per_million
                .or(self.cache_write_per_million),
        }
    }
}

/// A JSON number, or a string holding one (some endpoints quote their rates).
fn number_from(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

/// Per-model metadata for one Provider's catalog. The anthropic upstream advertises only ids,
/// and commandcode publishes only `context_length`, so these numbers come from the vendor's
/// published model docs (see `docs/research/`) or from measurements recorded in
/// [`PROVIDER_MODALITIES`]; a model no entry claims is advertised without metadata rather than
/// with guessed ones. Ordered longest-prefix first, so `claude-fable-5-1` wins over the
/// `claude-fable` family default and dated aliases (`claude-opus-5-20260101`) match their
/// family.
///
/// `provider` decides one field only: modalities that were measured per Provider, which the
/// family tables below cannot express because they are keyed by the bare id.
fn curated_metadata(id: &str, provider: &ProviderId) -> Option<ModelMetadata> {
    /// (context window, max output, input $/M, output $/M, cache-read $/M, cache-write $/M).
    /// `None` cache-write: the vendor does not bill a separate write rate for the family.
    const GPT5_LIMITS: (u64, u64, f64, f64, f64, Option<f64>) =
        (400_000, 128_000, 1.25, 10.0, 0.125, None);
    let mut entries = claude_family_entries();
    entries.extend(gpt_and_vendor_family_entries(GPT5_LIMITS));
    let base = entries
        .iter()
        .find(|(prefix, _)| id.starts_with(prefix))
        .map(|(_, metadata)| metadata.clone());
    let Some(modalities) = provider_modalities(id, provider) else {
        return base;
    };
    let mut metadata = base.unwrap_or_default();
    metadata.input_modalities = Some(modalities.iter().map(|s| (*s).to_string()).collect());
    Some(metadata)
}

/// Modality claims measured against one Provider, for ids where the family tables above cannot
/// be right.
///
/// The same bare id can accept images on one Provider and refuse them on another. Measured
/// 2026-09-14 (`docs/research/commandcode-model-metadata.md`): `deepseek/deepseek-v4-flash` reads
/// a four-quadrant image through commandcode and answers `404 No endpoints found that support
/// image input` through `OpenRouter`, while `deepseek/deepseek-v4.1-flash` reads images through
/// both. A table keyed by the bare id alone cannot state that, so an entry names the Provider it
/// was measured against, or `None` when it was measured against every Provider pengepul was
/// pointed at.
///
/// The name in the middle is the **Provider id** — the config entry name from
/// `pengepul login --provider <name>`, or `anthropic` / `codex` / `grok`. Registering the same
/// upstream under a second name therefore gets no claim until it is measured under that name,
/// and a name that points somewhere else inherits the claim of the Provider it is named after.
///
/// Matched **exactly**, not by prefix: every id here is one model, and a prefix rule would leak
/// a claim onto its neighbours (`deepseek/deepseek-v4-flash` onto `-flash-fast`, which refuses
/// images).
///
/// Everything absent from this table keeps the family table's claim, including the claims nothing
/// has measured. An unmeasured `["text"]` is a claim pengepul has not refuted; a measured one that
/// is false — which is what this table exists for — is a bug reported by the clients that trust it.
/// The family tables' `["text","image"]` entries are the same inference from vendor documentation
/// rather than from a measurement against the Provider serving them, so a false positive remains
/// possible there: that direction costs more than a dropped image, because a client attaches an
/// image the upstream refuses (issue #8's `404`). Measuring those is not done yet.
///
/// `ranked_metadata` still lets an upstream's own `input_modalities` win, so a claim here holds
/// only while the Provider stays silent about modalities, as commandcode does.
const PROVIDER_MODALITIES: &[(&str, Option<&str>, &[&str])] = &[
    ("deepseek/deepseek-v4.1-flash", None, TEXT_IMAGE),
    (
        "deepseek/deepseek-v4-flash",
        Some("commandcode"),
        TEXT_IMAGE,
    ),
    // commandcode relays these to endpoints that read images; no family entry claims modalities
    // for them, so without this they advertise none and a client that has no catalog of its own
    // drops the image (issue #8). `xiaomi/mimo-v2.5` has no family entry at all and reaches this
    // table as the only metadata it gets; `google/gemini-3.8-flash` and `z-ai/glm-5.3-flash` keep
    // the `reasoning` their family entry already carried.
    ("google/gemini-3.8-flash", Some("commandcode"), TEXT_IMAGE),
    ("xiaomi/mimo-v2.5", Some("commandcode"), TEXT_IMAGE),
    ("z-ai/glm-5.3-flash", Some("commandcode"), TEXT_IMAGE),
];

/// The modalities measured for `id` against `provider`, when the table carries a claim for that
/// exact pair.
fn provider_modalities(id: &str, provider: &ProviderId) -> Option<&'static [&'static str]> {
    PROVIDER_MODALITIES
        .iter()
        .find(|(model, measured_on, _)| {
            *model == id && measured_on.is_none_or(|name| name == provider.id.as_ref())
        })
        .map(|(_, _, modalities)| *modalities)
}

/// The claude families the direct anthropic catalog serves. A data table; the line count
/// is the catalog, not logic.
#[allow(clippy::too_many_lines)]
fn claude_family_entries() -> Vec<(&'static str, ModelMetadata)> {
    let entries: &[(&str, ModelMetadata)] = &[
        // fable-5-1's cache read is 0.025x base input, not the standard 0.1x.
        (
            "claude-fable-5-1",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                10.0,
                50.0,
                0.25,
                Some(12.5),
            ),
        ),
        (
            "claude-fable",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                10.0,
                50.0,
                1.0,
                Some(12.5),
            ),
        ),
        (
            "claude-opus-5",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                5.0,
                25.0,
                0.5,
                Some(6.25),
            ),
        ),
        (
            "claude-opus-4-8",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                5.0,
                25.0,
                0.5,
                Some(6.25),
            ),
        ),
        (
            "claude-opus-4-7",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                5.0,
                25.0,
                0.5,
                Some(6.25),
            ),
        ),
        (
            "claude-opus-4-6",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                5.0,
                25.0,
                0.5,
                Some(6.25),
            ),
        ),
        (
            "claude-opus-4",
            meta(
                TEXT_IMAGE,
                Some(200_000),
                Some(64_000),
                5.0,
                25.0,
                0.5,
                Some(6.25),
            ),
        ),
        (
            "claude-sonnet-5",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                2.0,
                10.0,
                0.2,
                Some(2.5),
            ),
        ),
        (
            "claude-sonnet-4-6",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(128_000),
                3.0,
                15.0,
                0.3,
                Some(3.75),
            ),
        ),
        (
            "claude-sonnet-4",
            meta(
                TEXT_IMAGE,
                Some(200_000),
                Some(64_000),
                3.0,
                15.0,
                0.3,
                Some(3.75),
            ),
        ),
        (
            "claude-haiku-4-5",
            meta(
                TEXT_IMAGE,
                Some(200_000),
                Some(64_000),
                1.0,
                5.0,
                0.1,
                Some(1.25),
            ),
        ),
    ];
    entries.to_vec()
}

/// The GPT and vendor families a configured OpenAI-compatible provider may serve
/// (commandcode's catalog among them), each with only vendor-sourced numbers.
fn gpt_and_vendor_family_entries(
    gpt5_limits: (u64, u64, f64, f64, f64, Option<f64>),
) -> Vec<(&'static str, ModelMetadata)> {
    let entries: &[(&str, ModelMetadata)] = &[
        // Commandcode families — sourced in docs/research/commandcode-model-metadata.md.
        // Longest prefix first; context is set where the vendor publishes it, else left
        // for the upstream's context_length to fill via the per-field merge.
        (
            "deepseek/deepseek-v4-pro",
            meta(
                TEXT_ONLY,
                Some(1_000_000),
                Some(384_000),
                1.32,
                3.96,
                0.044,
                None,
            ),
        ),
        (
            "deepseek/deepseek-v4-flash-vision-exp",
            meta(
                TEXT_IMAGE,
                Some(1_000_000),
                Some(384_000),
                0.44,
                1.32,
                0.014,
                None,
            ),
        ),
        (
            "deepseek/deepseek-v4",
            meta(
                TEXT_ONLY,
                Some(1_000_000),
                Some(384_000),
                0.44,
                1.32,
                0.014,
                None,
            ),
        ),
        (
            "moonshotai/Kimi-K2.7-Code-Highspeed",
            meta(TEXT_IMAGE, None, None, 1.90, 8.00, 0.38, None),
        ),
        (
            "moonshotai/Kimi-K2.7-Code",
            meta(TEXT_IMAGE, None, None, 0.95, 4.00, 0.19, None),
        ),
        (
            "moonshotai/Kimi-K3",
            meta(TEXT_IMAGE, Some(1_000_000), None, 3.0, 15.0, 0.30, None),
        ),
        ("moonshotai/Kimi", capability(None, None)),
        ("z-ai/glm", capability(None, None)),
        (
            "zai-org/GLM-5.3",
            capability(Some(1_000_000), Some(128_000)),
        ),
        (
            "zai-org/GLM-5.2",
            capability(Some(1_000_000), Some(128_000)),
        ),
        ("zai-org/GLM-5", capability(None, Some(128_000))),
        ("MiniMaxAI/MiniMax-M3", capability(Some(1_000_000), None)),
        ("MiniMaxAI/MiniMax-M", capability(None, None)),
        ("Qwen/Qwen3", capability(None, None)),
        (
            "xai/grok-4.6",
            meta(TEXT_IMAGE, Some(500_000), None, 2.0, 6.0, 0.50, None),
        ),
        (
            "xai/grok-4.5",
            meta(TEXT_IMAGE, Some(500_000), None, 2.0, 6.0, 0.30, None),
        ),
        (
            "grok-4.6",
            meta(TEXT_IMAGE, Some(500_000), None, 2.0, 6.0, 0.50, None),
        ),
        (
            "grok-4.5",
            meta(TEXT_IMAGE, Some(500_000), None, 2.0, 6.0, 0.30, None),
        ),
        // The Antigravity Gemini 4 family: one window and output cap published
        // by the upstream catalog for every effort tier, and no per-million
        // rate that a subscription bills at (docs/research/antigravity-provider.md).
        ("gemini-4-neon", capability(Some(1_048_576), Some(65_536))),
        ("google/gemini-3", capability(None, None)),
        (
            "gpt-5.6",
            meta(
                TEXT_IMAGE,
                Some(1_050_000),
                Some(128_000),
                4.0,
                20.0,
                0.4,
                None,
            ),
        ),
        ("gpt-5.5", meta_rated(TEXT_IMAGE, gpt5_limits)),
        ("gpt-5", meta_rated(TEXT_IMAGE, gpt5_limits)),
        ("codex-", meta_rated(TEXT_IMAGE, gpt5_limits)),
    ];
    entries.to_vec()
}
/// Input kinds for the family tables: most models take text and image; a few are
/// text-only. `capability` entries claim no modalities.
const TEXT_IMAGE: &[&str] = &["text", "image"];
const TEXT_ONLY: &[&str] = &["text"];

/// A text-and-image model from a `(window, output, input $/M, output $/M, cache-read $/M,
/// cache-write $/M)` tuple.
fn meta_rated(
    modalities: &[&str],
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
        modalities,
        Some(context_window),
        Some(max_output_tokens),
        input,
        output,
        cache_read,
        cache_write,
    )
}

/// A reasoning model with the given modalities, window, output cap and per-million
/// rates.
#[allow(clippy::too_many_arguments)]
fn meta(
    modalities: &[&str],
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: Option<f64>,
) -> ModelMetadata {
    ModelMetadata {
        context_window,
        max_output_tokens,
        input_modalities: Some(modalities.iter().map(|s| (*s).to_string()).collect()),
        reasoning: Some(true),
        pricing: Some(ModelPricing {
            input_per_million: Some(input),
            output_per_million: Some(output),
            cache_read_per_million: Some(cache_read),
            cache_write_per_million: cache_write,
        }),
    }
}

/// A reasoning-capable model with only the capability facts the vendor publishes — no
/// limits or rates. The upstream body fills what it knows (e.g. commandcode's
/// `context_length`) through the per-field merge; unknown fields stay omitted.
fn capability(context_window: Option<u64>, max_output_tokens: Option<u64>) -> ModelMetadata {
    ModelMetadata {
        context_window,
        max_output_tokens,
        input_modalities: None,
        reasoning: Some(true),
        pricing: None,
    }
}

/// The models a single fetch returned: anthropic and codex each give one list, plus any
/// per-model metadata those ids get from the curated table, the upstream body and, for a
/// configured provider, what the operator states (ranked by `ranked_metadata`).
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
    /// model id -> every built-in that lists it, in routing precedence. A bare id
    /// routes to the first; each copy is advertised under its own prefix, so a
    /// later fetch never takes an id from a built-in that listed it too.
    direct: BTreeMap<String, Vec<DirectModel>>,
    /// configured provider id -> the model ids its `/v1/models` returned. Only
    /// advertised; bare ids never route to generic providers (prefix-only).
    generic: BTreeMap<String, Vec<GenericModel>>,
    /// The operator's `allow:` lists. The lists above stay whole, so a bare id still resolves
    /// to the Provider that lists it and is refused by name rather than as unknown.
    allow: AllowLists,
    /// The built-ins whose list came from their upstream. Only such a list can
    /// say a maker does not serve an id; a floor or a static list cannot.
    live: std::collections::HashSet<ProviderKind>,
}

impl ModelCatalog {
    /// Replace a provider's entries with a freshly fetched list.
    pub fn set_direct(&mut self, kind: ProviderKind, models: FetchedModels) {
        // Grok's list is static, not fetched, so it can never speak for the
        // upstream; nor can an empty list, which names nothing it serves. Any
        // other built-in's list comes from its upstream.
        if kind == ProviderKind::Grok || models.ids.is_empty() {
            self.live.remove(&kind);
        } else {
            self.live.insert(kind);
        }
        for entries in self.direct.values_mut() {
            entries.retain(|existing| existing.kind != kind);
        }
        self.direct.retain(|_, entries| !entries.is_empty());
        for id in models.ids {
            let metadata = models.metadata.get(&id).cloned();
            let entries = self.direct.entry(id).or_default();
            entries.push(DirectModel { kind, metadata });
            entries.sort_by_key(|entry| bare_id_precedence(entry.kind));
        }
    }

    /// Whether any fetch has put a list for `kind` in the catalog.
    #[must_use]
    pub fn lists(&self, kind: ProviderKind) -> bool {
        self.direct
            .values()
            .any(|entries| entries.iter().any(|entry| entry.kind == kind))
    }

    /// Advertise `models` for `kind` as a floor: shown, but not a list that can
    /// say the upstream does not serve an id.
    pub fn set_floor(&mut self, kind: ProviderKind, models: FetchedModels) {
        self.set_direct(kind, models);
        self.live.remove(&kind);
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

    /// Replace every Provider's allow list.
    pub fn set_allow(&mut self, allow: AllowLists) {
        self.allow = allow;
    }

    /// Whether `provider` may advertise and serve `model`, its id without the `<provider>/`
    /// prefix. A Provider with no allow list may serve any model.
    #[must_use]
    pub fn allows(&self, provider: &ProviderId, model: &str) -> bool {
        self.allow
            .get(provider.id.as_ref())
            .is_none_or(|allowed| allowed.contains(model))
    }

    /// The entries of `provider`'s allow list that its last fetched list does not contain.
    #[must_use]
    pub fn unmatched_allow(&self, provider: &ProviderId) -> Vec<&str> {
        let Some(allowed) = self.allow.get(provider.id.as_ref()) else {
            return Vec::new();
        };
        let listed = |model: &str| match provider.kind {
            ProviderKind::Generic => self
                .generic
                .get(provider.id.as_ref())
                .is_some_and(|models| models.iter().any(|listed| listed.id == model)),
            kind => self
                .direct
                .get(model)
                .is_some_and(|listed| listed.iter().any(|entry| entry.kind == kind)),
        };
        allowed
            .iter()
            .map(String::as_str)
            .filter(|model| !listed(model))
            .collect()
    }

    /// Resolve a request's model id to the provider that serves it.
    ///
    /// - A `<provider>/<model>` id goes to that provider exactly, configured
    ///   `providers:` entries included.
    /// - A bare id whose name says who makes the model ([`bare_origin`]) goes
    ///   to that maker's built-in and never falls through to another provider.
    ///   When another provider lists it, it is refused if the maker's pool has
    ///   no account (`origin_has_account`) or the maker's loaded list lacks it,
    ///   and the refusal names the prefixed ids that serve it. An id no one else
    ///   lists goes to its maker, which answers for it: a `:high` shorthand, a
    ///   model newer than the list, or the pool's own "no account".
    /// - Any other bare id keeps the fetched lists' routing, first built-in by
    ///   precedence, or `None` when no list names it.
    ///
    /// # Errors
    ///
    /// The message for a bare id its maker cannot serve.
    pub fn resolve(
        &self,
        model: &str,
        providers: &BTreeMap<String, ConfiguredProvider>,
        pool: MakerPool,
    ) -> Result<Option<ProviderId>, String> {
        if let Some(prefixed) = prefixed_provider(model, providers) {
            return Ok(Some(prefixed));
        }
        let Some(origin) = bare_origin(model, providers) else {
            return Ok(self
                .direct
                .get(model)
                .and_then(|entries| entries.first())
                .map(|entry| ProviderId::for_kind(entry.kind)));
        };
        let elsewhere = self.served_elsewhere(model, origin);
        let maker = origin.canonical_id();
        let why = match pool {
            MakerPool::Serving => None,
            MakerPool::AllDisabled => Some(format!("every {maker} account is disabled")),
            MakerPool::Empty => Some(format!("no {maker} account")),
        };
        if let Some(why) = why {
            let next = if elsewhere.is_empty() {
                match pool {
                    MakerPool::AllDisabled => {
                        format!(
                            "enable one with `pengepul accounts enable <account> --provider {maker}`"
                        )
                    }
                    _ => format!("run `pengepul login --provider {maker}`"),
                }
            } else {
                format!("use {}", elsewhere.join(" or "))
            };
            return Err(format!("{model} is not served by {maker}: {why}; {next}"));
        }
        if self.live.contains(&origin) && !self.maker_lists(origin, model) && !elsewhere.is_empty()
        {
            return Err(format!(
                "{model} is not served by {maker}; use {}",
                elsewhere.join(" or ")
            ));
        }
        Ok(Some(ProviderId::for_kind(origin)))
    }

    /// Whether `origin`'s list names `model`'s model: the same id, a dated
    /// release of it or its dateless alias, with or without a `[1m]` marker, in
    /// any case. A maker serves the alias of what it lists by date, so a
    /// listing elsewhere cannot take that alias from it.
    fn maker_lists(&self, origin: ProviderKind, model: &str) -> bool {
        let wanted = model_family(model);
        self.direct.iter().any(|(id, entries)| {
            entries.iter().any(|entry| entry.kind == origin) && model_family(id) == wanted
        })
    }

    /// The prefixed ids that serve `model` on a provider other than `origin`
    /// and that the operator's allow lists admit, matched in any case.
    fn served_elsewhere(&self, model: &str, origin: ProviderKind) -> Vec<String> {
        let mut ids = Vec::new();
        for (id, entries) in &self.direct {
            if !id.eq_ignore_ascii_case(model) {
                continue;
            }
            for entry in entries.iter().filter(|entry| entry.kind != origin) {
                if self.allows(&ProviderId::for_kind(entry.kind), id) {
                    ids.push(format!("{}/{id}", entry.kind.canonical_id()));
                }
            }
        }
        for (provider_id, models) in &self.generic {
            let provider = ProviderId::generic(provider_id.as_str());
            for listed in models
                .iter()
                .filter(|listed| listed.id.eq_ignore_ascii_case(model))
            {
                if self.allows(&provider, &listed.id) {
                    ids.push(format!("{provider_id}/{}", listed.id));
                }
            }
        }
        ids
    }

    /// [`Self::resolve`] with every maker's pool holding an account, for tests
    /// of the lists alone.
    #[cfg(test)]
    fn resolve_id(
        &self,
        model: &str,
        providers: &BTreeMap<String, ConfiguredProvider>,
    ) -> Option<ProviderId> {
        self.resolve(model, providers, MakerPool::Serving)
            .ok()
            .flatten()
    }

    /// The advertised catalog for `/v1/models`: every id carries its `<provider>/<model>`
    /// prefix so a client can address a provider unambiguously even when ids overlap.
    #[must_use]
    pub fn advertised(&self) -> Vec<AdvertisedModel> {
        let mut out = self
            .direct
            .iter()
            .flat_map(|(id, entries)| {
                entries.iter().map(move |model| AdvertisedModel {
                    id: format!("{}/{id}", model.kind.canonical_id()),
                    provider: ProviderId::for_kind(model.kind),
                    metadata: model.metadata.clone(),
                })
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
        out.retain(|model| {
            self.allows(&model.provider, upstream_model(&model.id, &model.provider))
        });
        out
    }
}

/// Strip a leading `<provider>/` from a model id so the bare id goes upstream. The prefix
/// must be the resolved provider's own id (`anthropic`, `codex`, or the `providers:` entry
/// name); an unrelated id containing `/` is left intact.
///
/// Nothing else is touched. A client-side shorthand such as pi's `:high` travels as part of
/// the name it was given: the relay cannot tell a colon-word the catalog does not list from a
/// vendor tag it has not learned, and a served id like `LongCat-2.0:free` shows the shape is
/// not the signal.
#[must_use]
pub fn upstream_model<'a>(model: &'a str, provider: &ProviderId) -> &'a str {
    let prefix = provider.id.as_ref();
    model
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(model)
}

/// Which built-in a bare id goes to when several list it: the built-ins in the
/// order they joined the relay, so a new provider never takes an id an older one
/// already served.
const fn bare_id_precedence(kind: ProviderKind) -> u8 {
    match kind {
        ProviderKind::Anthropic => 0,
        ProviderKind::Codex => 1,
        ProviderKind::Grok => 2,
        ProviderKind::Antigravity => 3,
        ProviderKind::Generic => 4,
    }
}

/// A model id with its case, a trailing `[1m]` context marker and a
/// `-YYYYMMDD` release date set aside: the model it names, whichever form.
fn model_family(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    let lower = lower.strip_suffix("[1m]").unwrap_or(&lower);
    match lower.rsplit_once('-') {
        Some((family, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => {
            family.to_string()
        }
        _ => lower.to_string(),
    }
}

/// What the pool of a bare id's maker can do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerPool {
    /// At least one account can be handed a request (cooling down counts).
    Serving,
    /// Every account it holds is Disabled.
    AllDisabled,
    /// It holds no account.
    Empty,
}

/// The provider a `<provider>/<model>` id names, if its prefix is one.
fn prefixed_provider(
    model: &str,
    providers: &BTreeMap<String, ConfiguredProvider>,
) -> Option<ProviderId> {
    let (prefix, _) = model.split_once('/')?;
    match prefix {
        "anthropic" => Some(ProviderId::anthropic()),
        "codex" => Some(ProviderId::codex()),
        "grok" => Some(ProviderId::grok()),
        "antigravity" => Some(ProviderId::antigravity()),
        other if providers.contains_key(other) => Some(ProviderId::generic(other)),
        _ => None,
    }
}

/// The built-in of the company that makes a bare id's model, from its name:
/// `claude-*` is anthropic's, `gpt-*`, `o<digit>*` and `codex-*` codex's,
/// `gemini-*` antigravity's, `grok-*` grok's. `None` for a prefixed id and for
/// a name no rule claims. Broad on purpose: a new family of a known maker
/// routes without a code change.
#[must_use]
pub fn bare_origin(
    model: &str,
    providers: &BTreeMap<String, ConfiguredProvider>,
) -> Option<ProviderKind> {
    if prefixed_provider(model, providers).is_some() {
        return None;
    }
    heuristic_provider(model)
}

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
    if codex {
        return Some(ProviderKind::Codex);
    }
    if lower.starts_with("grok-") {
        return Some(ProviderKind::Grok);
    }
    lower
        .starts_with("gemini-")
        .then_some(ProviderKind::Antigravity)
}

/// The grok catalog is static: grok build serves two models (its
/// `default_models.json`), so there is no upstream fetch to fail. The ids and
/// the 500k context come from grok build's own model list
/// (docs/research/grok-build-provider.md).
#[must_use]
pub fn grok_static_models() -> FetchedModels {
    let ids = vec!["grok-4.6".to_string(), "grok-4.5".to_string()];
    let provider = ProviderId::grok();
    let metadata = ids
        .iter()
        .filter_map(|id| curated_metadata(id, &provider).map(|meta| (id.clone(), meta)))
        .collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// The antigravity floor: what the pool advertises until a live list lands,
/// for an account with no project or after a failed first fetch. A successful
/// `fetchAvailableModels` replaces it, and nothing replaces a live list with it.
#[must_use]
pub fn antigravity_static_models() -> FetchedModels {
    // The bare `gemini-4-neon` is not an upstream id (`404 Requested entity was
    // not found`); the effort is part of the name.
    let ids = [
        "gemini-4-neon-low",
        "gemini-4-neon-medium",
        "gemini-4-neon-high",
        "gemini-4-neon-xhigh",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let provider = ProviderId::antigravity();
    let metadata = ids
        .iter()
        .filter_map(|id| curated_metadata(id, &provider).map(|meta| (id.clone(), meta)))
        .collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// Whether an id the Antigravity catalog lists is a chat model this relay serves.
/// The list also carries the IDE's tab-completion models (`tab_*`), internal
/// `chat_<n>` ids, a thinking alias `CLIProxyAPI` also skips, and image models,
/// which need another request type (a non-goal).
fn is_antigravity_chat_model(id: &str) -> bool {
    !(id.starts_with("tab_")
        || id.starts_with("chat_")
        || id.contains("image")
        || id == "gemini-2.5-flash-thinking")
}

/// Models from an Antigravity `fetchAvailableModels` body, which keys a map by
/// model id: `{"models": {"<id>": {"displayName", "maxTokens",
/// "maxOutputTokens"}}}`. The window and cap are renamed to the field names the
/// ranking reads, so the upstream's numbers win over the curated table.
#[must_use]
pub fn parse_antigravity(body: &Value, provider: &ProviderId) -> FetchedModels {
    let Some(models) = body.get("models").and_then(Value::as_object) else {
        return FetchedModels::new(Vec::new());
    };
    let mut ids = Vec::new();
    let mut metadata = BTreeMap::new();
    for (id, entry) in models
        .iter()
        .filter(|(id, _)| is_antigravity_chat_model(id))
    {
        ids.push(id.clone());
        let published = json!({
            "context_length": entry.get("maxTokens"),
            "max_output_tokens": entry.get("maxOutputTokens"),
        });
        if let Some(meta) =
            ranked_metadata(id, provider, ModelMetadata::from_json(&published), None)
        {
            metadata.insert(id.clone(), meta);
        }
    }
    ids.sort();
    FetchedModels::with_metadata(ids, metadata)
}

/// Models from an Anthropic `/v1/models` body (`{"data": [{"id": ...}]}`). The upstream
/// carries ids only, so metadata comes from the curated table where one claims the id.
#[must_use]
pub fn parse_anthropic(body: &Value, provider: &ProviderId) -> FetchedModels {
    let ids = listed_models(body.get("data"), "id")
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    let metadata = ids
        .iter()
        .filter_map(|id| curated_metadata(id, provider).map(|meta| (id.clone(), meta)))
        .collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// One model's metadata from every source, ranked field by field: the curated table is the
/// base, what the upstream entry publishes wins over it, and what the operator states under
/// `providers.<id>.models` wins over both. `None` when no source has anything.
fn ranked_metadata(
    id: &str,
    provider: &ProviderId,
    upstream: Option<ModelMetadata>,
    stated: Option<&ConfiguredModel>,
) -> Option<ModelMetadata> {
    let stated = stated.map(|model| ModelMetadata {
        context_window: model.context_window,
        max_output_tokens: model.max_output_tokens,
        reasoning: model.reasoning,
        ..ModelMetadata::default()
    });
    [curated_metadata(id, provider), upstream, stated]
        .into_iter()
        .flatten()
        .reduce(ModelMetadata::merged_with)
}

/// Models from a Codex `/codex/models` body (`{"models": [{"slug": ...}]}`). Per-entry
/// fields the upstream publishes override the curated table field by field; anything it
/// leaves out falls back to the curated entry.
#[must_use]
pub fn parse_codex(body: &Value, provider: &ProviderId) -> FetchedModels {
    let listed = listed_models(body.get("models"), "slug");
    let metadata = listed
        .iter()
        .filter_map(|(id, entry)| {
            ranked_metadata(id, provider, ModelMetadata::from_json(entry), None)
                .map(|meta| (id.clone(), meta))
        })
        .collect();
    let ids = listed.into_iter().map(|(id, _)| id).collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// Models from an OpenAI-style `/models` body (`{"data": [{"id": ...}]}`), the shape
/// OpenAI-compatible endpoints return, ranked per model with `stated`, the provider's
/// `models:` from config (see `ranked_metadata`). A statement about an id the body does not
/// list advertises nothing; a model no source describes stays bare.
#[must_use]
pub fn parse_openai(
    body: &Value,
    provider: &ProviderId,
    stated: &BTreeMap<String, ConfiguredModel>,
) -> FetchedModels {
    let listed = listed_models(body.get("data"), "id");
    let metadata = listed
        .iter()
        .filter_map(|(id, entry)| {
            ranked_metadata(
                id,
                provider,
                ModelMetadata::from_json(entry),
                stated.get(id),
            )
            .map(|meta| (id.clone(), meta))
        })
        .collect();
    let ids = listed.into_iter().map(|(id, _)| id).collect();
    FetchedModels::with_metadata(ids, metadata)
}

/// Every entry of a `/models` array that carries its id under `field`, paired with that id.
/// An entry without one is skipped on its own, so no other model is handed its metadata.
fn listed_models<'a>(array: Option<&'a Value>, field: &str) -> Vec<(String, &'a Value)> {
    array
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| Some((item.get(field)?.as_str()?.to_owned(), item)))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        FetchedModels, ModelCatalog, capability, parse_anthropic, parse_codex, parse_openai,
    };
    use crate::config::ConfiguredModel;
    use crate::types::{ProviderId, ProviderKind};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn parses_each_provider_body_shape() {
        assert_eq!(
            parse_anthropic(
                &json!({"data": [{"id": "claude-opus-5"}, {"id": "claude-sonnet-5"}]}),
                &ProviderId::anthropic()
            )
            .ids,
            vec!["claude-opus-5", "claude-sonnet-5"]
        );
        assert_eq!(
            parse_codex(
                &json!({"models": [{"slug": "gpt-5.5"}, {"slug": "gpt-5.4"}]}),
                &ProviderId::codex()
            )
            .ids,
            vec!["gpt-5.5", "gpt-5.4"]
        );
        assert_eq!(
            parse_openai(
                &json!({"data": [{"id": "llama-3.3-70b"}]}),
                &ProviderId::generic("commandcode"),
                &BTreeMap::new(),
            )
            .ids,
            vec!["llama-3.3-70b"]
        );
    }

    /// The seam the issue is about: what `/v1/models` publishes, after the catalog has been
    /// built from a commandcode-style body (ids plus `context_length`, no modalities). The
    /// advertised entry is what a client gates its image path on, and these three ids have no
    /// family entry claiming modalities, so the table is the only source for them.
    #[test]
    fn the_advertised_payload_carries_the_measured_modalities() {
        // shape taken from the live endpoint: 69 entries, each id/name/context_length only
        let body = json!({"data": [
            {"id": "deepseek/deepseek-v4.1-flash", "context_length": 1_000_000},
            {"id": "deepseek/deepseek-v4-flash", "context_length": 1_000_000},
            {"id": "deepseek/deepseek-v4-flash-fast", "context_length": 1_000_000},
            {"id": "z-ai/glm-5.3-flash", "context_length": 1_000_000},
            {"id": "google/gemini-3.8-flash", "context_length": 1_000_000},
            {"id": "xiaomi/mimo-v2.5", "context_length": 1_000_000},
        ]});
        let advertised = |provider: &str| {
            let id = ProviderId::generic(provider);
            let mut catalog = ModelCatalog::default();
            catalog.set_generic(provider, parse_openai(&body, &id, &BTreeMap::new()));
            catalog
                .advertised()
                .into_iter()
                .map(|model| {
                    (
                        model.id,
                        model
                            .metadata
                            .and_then(|meta| meta.input_modalities)
                            .unwrap_or_default(),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };

        let commandcode = advertised("commandcode");
        assert_eq!(
            commandcode["commandcode/deepseek/deepseek-v4.1-flash"],
            vec!["text".to_string(), "image".to_string()]
        );
        assert_eq!(
            commandcode["commandcode/deepseek/deepseek-v4-flash"],
            vec!["text".to_string(), "image".to_string()]
        );
        // Unchanged: measured text-only, and the parent of the Provider-scoped entry.
        assert_eq!(
            commandcode["commandcode/deepseek/deepseek-v4-flash-fast"],
            vec!["text".to_string()]
        );
        // All three ids the family tables never claimed, at the seam that ships.
        for id in [
            "commandcode/z-ai/glm-5.3-flash",
            "commandcode/google/gemini-3.8-flash",
            "commandcode/xiaomi/mimo-v2.5",
        ] {
            assert_eq!(
                commandcode[id],
                vec!["text".to_string(), "image".to_string()],
                "{id} lost its measured claim"
            );
        }

        let openrouter = advertised("openrouter");
        assert_eq!(
            openrouter["openrouter/deepseek/deepseek-v4-flash"],
            vec!["text".to_string()],
            "OpenRouter refuses image input for this id, and now the payload says so"
        );
        for id in [
            "openrouter/z-ai/glm-5.3-flash",
            "openrouter/google/gemini-3.8-flash",
            "openrouter/xiaomi/mimo-v2.5",
        ] {
            assert_eq!(
                openrouter[id],
                Vec::<String>::new(),
                "{id} claims modalities on a Provider it was never measured against"
            );
        }
    }

    /// The measured Provider splits, pinned so a later simplification of the table cannot make
    /// the id advertise one capability everywhere again. Measured 2026-09-14 with a
    /// four-quadrant image: commandcode reads it, `OpenRouter` answers
    /// `404 No endpoints found that support image input`.
    #[test]
    fn a_provider_scoped_claim_holds_only_on_the_provider_it_was_measured_on() {
        let body = |id: &str| json!({"data": [{"id": id}]});
        let modalities = |id: &str, provider: &ProviderId| {
            parse_openai(&body(id), provider, &BTreeMap::new())
                .metadata
                .remove(id)
                .and_then(|meta| meta.input_modalities)
        };
        let commandcode = ProviderId::generic("commandcode");
        let openrouter = ProviderId::generic("openrouter");

        // One bare id, two answers. A single provider-agnostic entry cannot express this.
        assert_eq!(
            modalities("deepseek/deepseek-v4-flash", &commandcode),
            Some(vec!["text".to_string(), "image".to_string()])
        );
        assert_eq!(
            modalities("deepseek/deepseek-v4-flash", &openrouter),
            Some(vec!["text".to_string()])
        );

        // Measured against both Providers, so the claim is not scoped.
        for provider in [&commandcode, &openrouter] {
            assert_eq!(
                modalities("deepseek/deepseek-v4.1-flash", provider),
                Some(vec!["text".to_string(), "image".to_string()]),
                "{} lost the claim",
                provider.id
            );
        }

        // The exact match is what keeps the claim off the siblings the same prefix covers.
        for text_only in [
            "deepseek/deepseek-v4-flash-fast",
            "deepseek/deepseek-v4-flash-0731",
            "deepseek/deepseek-v4-pro",
            "deepseek/deepseek-v4-pro-0813",
        ] {
            assert_eq!(
                modalities(text_only, &commandcode),
                Some(vec!["text".to_string()]),
                "{text_only} must stay text-only"
            );
        }

        // The three ids the table had no entry for at all: the claim is the only metadata they
        // get, and it is scoped to the Provider it was measured on.
        for id in [
            "google/gemini-3.8-flash",
            "xiaomi/mimo-v2.5",
            "z-ai/glm-5.3-flash",
        ] {
            assert_eq!(
                modalities(id, &commandcode),
                Some(vec!["text".to_string(), "image".to_string()]),
                "{id} must advertise the vision it was measured to have"
            );
            assert_eq!(
                modalities(id, &openrouter),
                None,
                "{id} must claim nothing on a Provider it was not measured against"
            );
        }
    }

    #[test]
    fn anthropic_ids_get_curated_metadata() {
        let fetched = parse_anthropic(
            &json!({"data": [
                {"id": "claude-fable-5-1"}, {"id": "claude-opus-5"}, {"id": "claude-opus-9"}
            ]}),
            &ProviderId::anthropic(),
        );
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
        let fetched_4x = parse_anthropic(
            &json!({"data": [
                {"id": "claude-opus-4-8"}, {"id": "claude-opus-4-5"},
                {"id": "claude-sonnet-4-6"}, {"id": "claude-sonnet-4-5"}
            ]}),
            &ProviderId::anthropic(),
        );
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
        let fetched = parse_openai(&body, &ProviderId::generic("commandcode"), &BTreeMap::new());
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
    fn a_vllm_style_max_model_len_is_the_context_window() {
        // omlx's live body: vLLM's field name, no context_length
        let body = json!({"data": [
            {"id": "Qwen3.8-27B-Uncensored-MLX", "object": "model",
             "owned_by": "omlx", "max_model_len": 262_144}
        ]});
        let fetched = parse_openai(&body, &ProviderId::generic("omlx"), &BTreeMap::new());
        let qwen = fetched
            .metadata
            .get("Qwen3.8-27B-Uncensored-MLX")
            .expect("qwen metadata");
        assert_eq!(qwen.context_window, Some(262_144));
    }

    #[test]
    fn codex_body_metadata_wins_over_the_curated_table() {
        let fetched = parse_codex(
            &json!({"models": [
                {"slug": "gpt-5.5", "context_window": 272_000}
            ]}),
            &ProviderId::codex(),
        );
        assert_eq!(
            fetched
                .metadata
                .get("gpt-5.5")
                .and_then(|m| m.context_window),
            Some(272_000)
        );
    }

    /// An entry the upstream lists without its id is skipped on its own. Filtering the ids
    /// first and zipping them against every entry would advertise each model after the gap
    /// with the metadata of the entry before it.
    #[test]
    fn an_entry_without_an_id_leaves_every_other_model_its_own_metadata() {
        let openai = parse_openai(
            &json!({"data": [
                {"object": "model", "context_length": 1_000},
                {"id": "alpha", "context_length": 2_000},
                {"id": "beta", "context_length": 3_000}
            ]}),
            &ProviderId::generic("omlx"),
            &BTreeMap::new(),
        );
        assert_eq!(openai.ids, vec!["alpha", "beta"]);
        assert_eq!(openai.metadata["alpha"].context_window, Some(2_000));
        assert_eq!(openai.metadata["beta"].context_window, Some(3_000));

        let codex = parse_codex(
            &json!({"models": [
                {"context_window": 1_000},
                {"slug": "alpha", "context_window": 2_000},
                {"slug": "beta", "context_window": 3_000}
            ]}),
            &ProviderId::codex(),
        );
        assert_eq!(codex.ids, vec!["alpha", "beta"]);
        assert_eq!(codex.metadata["alpha"].context_window, Some(2_000));
        assert_eq!(codex.metadata["beta"].context_window, Some(3_000));
    }

    #[test]
    fn the_operator_statement_ranks_above_the_upstream_and_the_curated_table() {
        // deepseek-v4-pro has a curated entry (reasoning, max output 384_000); the upstream
        // publishes its context; the operator overrides reasoning and the context.
        let stated = BTreeMap::from([
            (
                "deepseek/deepseek-v4-pro".to_string(),
                ConfiguredModel {
                    reasoning: Some(false),
                    context_window: Some(500_000),
                    max_output_tokens: None,
                },
            ),
            (
                "Qwen3.8-27B-Uncensored-MLX".to_string(),
                ConfiguredModel {
                    reasoning: Some(true),
                    ..ConfiguredModel::default()
                },
            ),
            (
                "not-served".to_string(),
                ConfiguredModel {
                    reasoning: Some(true),
                    ..ConfiguredModel::default()
                },
            ),
        ]);
        let fetched = parse_openai(
            &json!({"data": [
                {"id": "deepseek/deepseek-v4-pro", "context_length": 1_000_000},
                {"id": "Qwen3.8-27B-Uncensored-MLX", "max_model_len": 128_000}
            ]}),
            &ProviderId::generic("commandcode"),
            &stated,
        );

        let pro = &fetched.metadata["deepseek/deepseek-v4-pro"];
        assert_eq!(
            pro.reasoning,
            Some(false),
            "the statement beats the curated table"
        );
        assert_eq!(
            pro.context_window,
            Some(500_000),
            "the statement beats the upstream"
        );
        assert_eq!(
            pro.max_output_tokens,
            Some(384_000),
            "left out, the curated value stays"
        );
        let qwen = &fetched.metadata["Qwen3.8-27B-Uncensored-MLX"];
        assert_eq!(qwen.reasoning, Some(true));
        assert_eq!(
            qwen.context_window,
            Some(128_000),
            "left out, the upstream value stays"
        );
        // a statement about a model the upstream does not list advertises nothing
        assert_eq!(fetched.ids.len(), 2);
        assert!(!fetched.metadata.contains_key("not-served"));
    }

    #[test]
    fn reasoning_serializes_only_when_known() {
        let known = capability(None, None);
        let json = serde_json::to_value(&known).expect("serialize");
        assert_eq!(json["reasoning"], true);
        let mut unknown = known;
        unknown.reasoning = None;
        let json = serde_json::to_value(&unknown).expect("serialize");
        assert!(json.get("reasoning").is_none());
    }

    #[test]
    fn openai_metadata_merges_the_curated_base_with_upstream_overrides() {
        // commandcode-style body: context_length published, everything else silent
        let fetched = parse_openai(
            &json!({"data": [
                {"id": "deepseek/deepseek-v4-pro", "context_length": 1_000_000},
                {"id": "google/gemini-3.8-flash", "context_length": 1_000_000},
                {"id": "totally-unknown-model"}
            ]}),
            &ProviderId::generic("commandcode"),
            &BTreeMap::new(),
        );
        let pro = fetched
            .metadata
            .get("deepseek/deepseek-v4-pro")
            .expect("pro metadata");
        // upstream context wins, curated fills max output, reasoning and pricing
        assert_eq!(pro.context_window, Some(1_000_000));
        assert_eq!(pro.max_output_tokens, Some(384_000));
        assert_eq!(pro.reasoning, Some(true));
        let pricing = pro.pricing.as_ref().expect("deepseek pricing");
        assert_eq!(pricing.input_per_million, Some(1.32));
        // capability-only family: reasoning set, no invented limits
        let gemini = fetched
            .metadata
            .get("google/gemini-3.8-flash")
            .expect("gemini metadata");
        assert_eq!(gemini.reasoning, Some(true));
        assert_eq!(gemini.max_output_tokens, None);
        assert_eq!(gemini.pricing, None);
        // no curated entry and a silent upstream stays bare
        assert!(!fetched.metadata.contains_key("totally-unknown-model"));
    }

    #[test]
    fn commandcode_ids_the_relay_serves_resolve_to_metadata() {
        // ids advertised for commandcode. Families with vendor-sourced entries carry
        // reasoning; the unsourced ones (docs/research/commandcode-model-metadata.md)
        // are expected to stay bare.
        let sourced = [
            "deepseek/deepseek-v4-pro",
            "deepseek/deepseek-v4-flash",
            "deepseek/deepseek-v4-flash-fast",
            "deepseek/deepseek-v4-flash-vision-exp",
            "moonshotai/Kimi-K3",
            "moonshotai/Kimi-K2.7-Code",
            "moonshotai/Kimi-K2.7-Code-Highspeed",
            "moonshotai/Kimi-K2.6",
            "moonshotai/Kimi-K2.5",
            "z-ai/glm-5.3-flash",
            "zai-org/GLM-5.3",
            "zai-org/GLM-5.2",
            "zai-org/GLM-5.2-Fast",
            "zai-org/GLM-5.1",
            "zai-org/GLM-5",
            "MiniMaxAI/MiniMax-M3",
            "MiniMaxAI/MiniMax-M2.7",
            "MiniMaxAI/MiniMax-M2.5",
            "Qwen/Qwen3.8-Max-0902",
            "Qwen/Qwen3.8-Max",
            "Qwen/Qwen3.8-27B",
            "Qwen/Qwen3.8-Flash",
            "Qwen/Qwen3.7-Max",
            "Qwen/Qwen3.7-Plus",
            "Qwen/Qwen3.7-Flash",
            "Qwen/Qwen3.6-Max-Preview",
            "Qwen/Qwen3.6-Plus",
            "xai/grok-4.5",
            "xai/grok-4.6",
            "google/gemini-3.8-flash",
            "google/gemini-3.7-flash",
            "google/gemini-3.6-flash",
            "google/gemini-3.5-flash",
            "google/gemini-3.5-flash-lite",
            "google/gemini-3.1-flash-lite",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
        ];
        let body = json!({"data": sourced
            .iter()
            .map(|id| json!({"id": id, "context_length": 999_999}))
            .collect::<Vec<_>>()});
        let fetched = parse_openai(&body, &ProviderId::generic("commandcode"), &BTreeMap::new());
        for id in sourced {
            let metadata = fetched
                .metadata
                .get(id)
                .unwrap_or_else(|| panic!("{id} missing metadata"));
            assert_eq!(metadata.reasoning, Some(true), "{id} should reason");
        }
        // context always comes from the upstream's context_length, never the table
        let deepseek = fetched
            .metadata
            .get("deepseek/deepseek-v4-flash")
            .expect("flash");
        assert_eq!(deepseek.context_window, Some(999_999));
        let sol = fetched.metadata.get("gpt-5.6-sol").expect("sol");
        assert_eq!(sol.context_window, Some(999_999));
        assert_eq!(sol.max_output_tokens, Some(128_000));

        let unsourced = [
            "xiaomi/mimo-v2.5",
            "xiaomi/mimo-v2.5-pro",
            "stepfun/Step-3.7-Flash",
            "stepfun/Step-3.5-Flash",
            "tencent/hy3-paid",
            "tencent/hy4-preview",
            "meituan/LongCat-2.0:free",
            "meta/muse-spark-1.1",
            "meta/muse-spark-1.2",
            "meta/muse-spark-1.2-contributor",
            "meta/muse-spark-1.3",
            "meta/muse-spark-1.3-contributor",
            "nvidia/nemotron-3-ultra-550b-a55b",
            "thinkingmachines/inkling",
            "thinkingmachines/inkling-small",
            "poolside/laguna-s-2.1-free",
            "sakana/fugu-ultra",
        ];
        let body = json!({"data": unsourced
            .iter()
            .map(|id| json!({"id": id, "context_length": 999_999}))
            .collect::<Vec<_>>()});
        let fetched = parse_openai(&body, &ProviderId::generic("commandcode"), &BTreeMap::new());
        for id in unsourced {
            let metadata = fetched
                .metadata
                .get(id)
                .unwrap_or_else(|| panic!("{id} should still carry the upstream context"));
            // no sourced entry: only what the upstream published — context, no reasoning
            assert_eq!(metadata.reasoning, None, "{id} must not claim reasoning");
            assert_eq!(metadata.context_window, Some(999_999));
            assert_eq!(metadata.max_output_tokens, None);
            assert_eq!(metadata.pricing, None);
        }
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
        // R3: every bare `gemini-*` id is antigravity's, its maker's
        assert_eq!(
            catalog.resolve_id("gemini-3", &providers),
            Some(ProviderId::antigravity())
        );
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
                models: std::collections::BTreeMap::new(),
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

    /// A catalog holding three Providers' fetched lists, with allow lists on two of them.
    fn allowlisted_catalog() -> ModelCatalog {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(vec![
                "claude-opus-5-5".into(),
                "claude-sonnet-5-5".into(),
                "claude-haiku-5".into(),
            ]),
        );
        catalog.set_direct(
            ProviderKind::Codex,
            FetchedModels::new(vec!["gpt-5.5".into(), "gpt-5.4".into()]),
        );
        catalog.set_generic(
            "commandcode",
            FetchedModels::new(vec![
                "deepseek/deepseek-v4.1-flash".into(),
                "kimi-k3".into(),
            ]),
        );
        catalog.set_allow(BTreeMap::from([
            (
                "anthropic".to_string(),
                ["claude-opus-5-5", "claude-opus-9"]
                    .map(String::from)
                    .into(),
            ),
            (
                "commandcode".to_string(),
                ["deepseek/deepseek-v4.1-flash"].map(String::from).into(),
            ),
        ]));
        catalog
    }

    #[test]
    fn an_allow_list_narrows_what_its_provider_advertises_and_no_other() {
        let mut ids = allowlisted_catalog()
            .advertised()
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();
        ids.sort();

        // claude-opus-9 is allowed but not listed upstream, so it is not advertised; codex has
        // no allow list and advertises its whole list.
        assert_eq!(
            ids,
            [
                "anthropic/claude-opus-5-5",
                "codex/gpt-5.4",
                "codex/gpt-5.5",
                "commandcode/deepseek/deepseek-v4.1-flash",
            ]
        );
    }

    #[test]
    fn an_allow_list_matches_the_native_id_exactly() {
        let catalog = allowlisted_catalog();
        let commandcode = ProviderId::generic("commandcode");

        assert!(catalog.allows(&ProviderId::anthropic(), "claude-opus-5-5"));
        assert!(catalog.allows(&commandcode, "deepseek/deepseek-v4.1-flash"));
        for refused in [
            "claude-sonnet-5-5",
            "anthropic/claude-opus-5-5",
            "Claude-Opus-5-5",
            "claude-opus-5",
            "claude-opus-5-5 ",
        ] {
            assert!(
                !catalog.allows(&ProviderId::anthropic(), refused),
                "{refused} was allowed"
            );
        }
        assert!(!catalog.allows(&commandcode, "deepseek-v4.1-flash"));
        // No allow list: anything the Provider is asked for, listed or not.
        assert!(catalog.allows(&ProviderId::codex(), "gpt-5.5"));
        assert!(catalog.allows(&ProviderId::codex(), "gpt-7"));
    }

    #[test]
    fn unmatched_entries_are_the_allowed_ids_the_upstream_does_not_list() {
        let catalog = allowlisted_catalog();

        assert_eq!(
            catalog.unmatched_allow(&ProviderId::anthropic()),
            ["claude-opus-9"]
        );
        assert!(
            catalog
                .unmatched_allow(&ProviderId::generic("commandcode"))
                .is_empty()
        );
        assert!(catalog.unmatched_allow(&ProviderId::codex()).is_empty());
    }

    #[test]
    fn advertised_carries_each_models_metadata() {
        let mut catalog = ModelCatalog::default();
        catalog.set_direct(
            ProviderKind::Anthropic,
            parse_anthropic(
                &json!({"data": [
                    {"id": "claude-fable-5-1"}
                ]}),
                &ProviderId::anthropic(),
            ),
        );
        catalog.set_generic(
            "groq",
            parse_openai(
                &json!({"data": [
                    {"id": "llama-3.3-70b", "context_window": 131_072}
                ]}),
                &ProviderId::generic("commandcode"),
                &BTreeMap::new(),
            ),
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

    /// AC-1/AC-2: the prefix strip is the only rewrite. Everything after it is the name,
    /// including a colon-word on either side of a slash, because the relay cannot tell a
    /// client's shorthand from a vendor's tag.
    #[test]
    fn upstream_model_strips_the_provider_prefix_and_nothing_else() {
        let anthropic = ProviderId::anthropic();
        let generic = ProviderId::generic("commandcode");
        // AC-1: the prefix goes, for every provider.
        assert_eq!(
            super::upstream_model("anthropic/claude-opus-5", &anthropic),
            "claude-opus-5"
        );
        assert_eq!(
            super::upstream_model("codex/gpt-5.5", &ProviderId::codex()),
            "gpt-5.5"
        );
        assert_eq!(
            super::upstream_model("commandcode/some-model", &generic),
            "some-model"
        );
        // AC-2: a colon-word travels whole, whatever it says. No level is special any more:
        // every one of the seven the relay used to drop now survives, so a partial
        // re-introduction of the old allowlist fails here rather than shipping.
        for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
            let suffixed = format!("anthropic/claude-opus-5:{level}");
            assert_eq!(
                super::upstream_model(&suffixed, &anthropic),
                format!("claude-opus-5:{level}"),
                "{level} was stripped"
            );
        }
        assert_eq!(
            super::upstream_model("commandcode/meituan/LongCat-2.0:free", &generic),
            "meituan/LongCat-2.0:free"
        );
        assert_eq!(super::upstream_model("qwen:7b", &generic), "qwen:7b");
        assert_eq!(super::upstream_model(":high", &anthropic), ":high");
        assert_eq!(
            super::upstream_model("anthropic/x:high:high", &anthropic),
            "x:high:high"
        );
        // A colon before a slash, inside the name, is part of the name too.
        assert_eq!(
            super::upstream_model("anthropic/x:high/y", &anthropic),
            "x:high/y"
        );
        // A prefix that is not this provider's is part of the name.
        assert_eq!(
            super::upstream_model("openrouter/anthropic/claude-opus-5", &generic),
            "openrouter/anthropic/claude-opus-5"
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

#[test]
fn grok_models_route_bare_prefixed_and_by_shape() {
    let mut catalog = ModelCatalog::default();
    catalog.set_direct(ProviderKind::Grok, grok_static_models());
    let providers = BTreeMap::from([(
        "xai".to_string(),
        crate::config::ConfiguredProvider {
            base_url: "https://api.x.ai/v1".to_string(),
            models: std::collections::BTreeMap::new(),
        },
    )]);

    // bare ids come from the static list
    assert_eq!(
        catalog.resolve_id("grok-4.6", &providers),
        Some(ProviderId::grok())
    );
    // the provider's own prefix routes the same pool
    assert_eq!(
        catalog.resolve_id("grok/grok-4.6", &providers),
        Some(ProviderId::grok())
    );
    // unknown grok families still route by name shape
    assert_eq!(
        catalog.resolve_id("grok-9", &providers),
        Some(ProviderId::for_kind(ProviderKind::Grok))
    );
    // and `xai/` stays claimed by the configured console endpoint
    assert_eq!(
        catalog.resolve_id("xai/grok-4.6", &providers),
        Some(ProviderId::generic("xai"))
    );
}

#[test]
fn grok_static_models_carry_context_metadata_and_advertise_prefixed() {
    let static_models = grok_static_models();
    assert_eq!(
        static_models.ids,
        vec!["grok-4.6".to_string(), "grok-4.5".to_string()]
    );
    for id in &static_models.ids {
        let metadata = static_models.metadata.get(id).expect("curated metadata");
        assert_eq!(metadata.context_window, Some(500_000), "{id}");
    }

    let mut catalog = ModelCatalog::default();
    catalog.set_direct(ProviderKind::Grok, static_models);
    let mut advertised: Vec<(String, Option<u64>)> = catalog
        .advertised()
        .into_iter()
        .filter(|model| model.provider.kind == ProviderKind::Grok)
        .map(|model| {
            (
                model.id,
                model.metadata.and_then(|meta| meta.context_window),
            )
        })
        .collect();
    advertised.sort();
    assert_eq!(
        advertised,
        vec![
            ("grok/grok-4.5".to_string(), Some(500_000)),
            ("grok/grok-4.6".to_string(), Some(500_000)),
        ]
    );
}

#[test]
fn antigravity_models_route_bare_prefixed_and_by_shape() {
    let mut catalog = ModelCatalog::default();
    catalog.set_direct(ProviderKind::Antigravity, antigravity_static_models());
    let providers = BTreeMap::new();

    // The floor: the four effort tiers the upstream serves.
    for id in [
        "gemini-4-neon-low",
        "gemini-4-neon-medium",
        "gemini-4-neon-high",
        "gemini-4-neon-xhigh",
    ] {
        assert_eq!(
            catalog.resolve_id(id, &providers),
            Some(ProviderId::antigravity()),
            "{id}"
        );
        assert_eq!(
            catalog.resolve_id(&format!("antigravity/{id}"), &providers),
            Some(ProviderId::antigravity()),
            "antigravity/{id}"
        );
    }
    // A Gemini 4 family id the static list does not name still routes by shape.
    assert_eq!(
        catalog.resolve_id("gemini-4-hydrogen-high", &providers),
        Some(ProviderId::for_kind(ProviderKind::Antigravity))
    );
    // R3: a Gemini 3 id is antigravity's too; a configured endpoint that
    // serves it is reached by its prefix.
    assert_eq!(
        catalog.resolve_id("gemini-3-flash", &providers),
        Some(ProviderId::antigravity())
    );
}

#[test]
fn antigravity_static_models_floor_advertises_prefixed_with_limits() {
    let static_models = antigravity_static_models();
    assert_eq!(
        static_models.ids,
        vec![
            "gemini-4-neon-low".to_string(),
            "gemini-4-neon-medium".to_string(),
            "gemini-4-neon-high".to_string(),
            "gemini-4-neon-xhigh".to_string(),
        ]
    );
    for id in &static_models.ids {
        let metadata = static_models.metadata.get(id).expect("curated metadata");
        assert_eq!(metadata.context_window, Some(1_048_576), "{id}");
        assert_eq!(metadata.max_output_tokens, Some(65_536), "{id}");
    }

    let mut catalog = ModelCatalog::default();
    catalog.set_direct(ProviderKind::Antigravity, static_models);
    let advertised: Vec<String> = catalog
        .advertised()
        .into_iter()
        .filter(|model| model.provider.kind == ProviderKind::Antigravity)
        .map(|model| model.id)
        .collect();
    assert!(advertised.contains(&"antigravity/gemini-4-neon-medium".to_string()));
    assert!(advertised.contains(&"antigravity/gemini-4-neon-xhigh".to_string()));
}

#[test]
fn parse_antigravity_reads_the_keyed_map_and_its_limits() {
    let fetched = parse_antigravity(
        &json!({"models": {
            "gemini-4-neon-high": {"displayName": "Gemini 4 Neon",
                                  "maxTokens": 1_048_576, "maxOutputTokens": 65_536},
            "claude-sonnet-4-6": {"displayName": "Claude Sonnet 4.6"}
        }}),
        &ProviderId::antigravity(),
    );
    assert_eq!(
        fetched.ids,
        vec![
            "claude-sonnet-4-6".to_string(),
            "gemini-4-neon-high".to_string()
        ]
    );
    let neon = fetched.metadata.get("gemini-4-neon-high").expect("neon");
    assert_eq!(neon.context_window, Some(1_048_576));
    assert_eq!(neon.max_output_tokens, Some(65_536));
    // An entry that publishes nothing keeps only what the curated table knows.
    let claude = fetched.metadata.get("claude-sonnet-4-6").expect("claude");
    assert!(
        claude.context_window.is_some(),
        "the curated window fills in"
    );
}

#[test]
fn parse_antigravity_without_a_models_map_is_empty() {
    assert!(
        parse_antigravity(&json!({"error": {"code": 401}}), &ProviderId::antigravity())
            .ids
            .is_empty()
    );
}

/// F6 and S3: an id of no known maker that both the anthropic and the
/// antigravity catalogs list goes to anthropic, the earlier built-in, whichever
/// was fetched last — including on a later refresh that fetches anthropic
/// again. The maker rule does not decide it: `mythos-preview` names no maker.
#[test]
fn an_id_of_no_known_maker_follows_built_in_precedence_across_refreshes() {
    let providers = BTreeMap::new();
    let anthropic = || FetchedModels::new(vec!["mythos-preview".to_string()]);
    let antigravity = || FetchedModels::new(vec!["mythos-preview".to_string()]);
    for anthropic_first in [true, false] {
        let mut catalog = ModelCatalog::default();
        if anthropic_first {
            catalog.set_direct(ProviderKind::Anthropic, anthropic());
            catalog.set_direct(ProviderKind::Antigravity, antigravity());
        } else {
            catalog.set_direct(ProviderKind::Antigravity, antigravity());
            catalog.set_direct(ProviderKind::Anthropic, anthropic());
        }
        // The next refresh cycle fetches anthropic again.
        catalog.set_direct(ProviderKind::Anthropic, anthropic());
        assert_eq!(
            catalog.resolve_id("mythos-preview", &providers),
            Some(ProviderId::anthropic()),
            "anthropic first: {anthropic_first}"
        );
    }
}

/// F6: an id both the anthropic and the antigravity catalogs list stays with
/// anthropic whichever was fetched last; the antigravity copy is reachable only
/// by its prefix, and both stay advertised.
#[test]
fn an_antigravity_catalog_never_takes_an_id_from_anthropic() {
    let providers = BTreeMap::new();
    for anthropic_first in [true, false] {
        let mut catalog = ModelCatalog::default();
        let anthropic = FetchedModels::new(vec!["claude-sonnet-4-6".to_string()]);
        let antigravity = FetchedModels::new(vec![
            "claude-sonnet-4-6".to_string(),
            "gemini-4-neon-low".to_string(),
        ]);
        if anthropic_first {
            catalog.set_direct(ProviderKind::Anthropic, anthropic);
            catalog.set_direct(ProviderKind::Antigravity, antigravity);
        } else {
            catalog.set_direct(ProviderKind::Antigravity, antigravity);
            catalog.set_direct(ProviderKind::Anthropic, anthropic);
        }
        assert_eq!(
            catalog.resolve_id("claude-sonnet-4-6", &providers),
            Some(ProviderId::anthropic()),
            "anthropic first: {anthropic_first}"
        );
        assert_eq!(
            catalog.resolve_id("antigravity/claude-sonnet-4-6", &providers),
            Some(ProviderId::antigravity())
        );
        let advertised: Vec<String> = catalog.advertised().into_iter().map(|m| m.id).collect();
        assert!(
            advertised.contains(&"anthropic/claude-sonnet-4-6".to_string()),
            "{advertised:?}"
        );
        assert!(
            advertised.contains(&"antigravity/claude-sonnet-4-6".to_string()),
            "{advertised:?}"
        );
        // A refetch of anthropic alone leaves antigravity's copy listed.
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(vec!["claude-sonnet-4-6".to_string()]),
        );
        assert!(
            catalog
                .advertised()
                .iter()
                .any(|m| m.id == "antigravity/claude-sonnet-4-6")
        );
    }
}

/// F5/L5: the floor names only ids the upstream serves, and the internal and
/// non-chat ids of a live list are not advertised.
#[test]
fn the_antigravity_catalog_advertises_only_served_chat_models() {
    assert!(
        !antigravity_static_models()
            .ids
            .contains(&"gemini-4-neon".to_string()),
        "the bare id answers 404 upstream"
    );
    let fetched = parse_antigravity(
        &json!({"models": {
            "gemini-4-neon-medium": {}, "tab_flash_lite_preview": {},
            "tab_jump_flash_lite_preview": {}, "chat_20706": {},
            "gemini-3.1-flash-image": {}, "gemini-2.5-flash-thinking": {}
        }}),
        &ProviderId::antigravity(),
    );
    assert_eq!(fetched.ids, vec!["gemini-4-neon-medium".to_string()]);
}

/// The catalog the origin-rule tests share: antigravity's live list names a
/// Claude id, a GPT-OSS id, a Gemini id and an id of no known maker.
#[cfg(test)]
fn origin_catalog(anthropic: Option<&[&str]>, codex: Option<&[&str]>) -> ModelCatalog {
    let mut catalog = ModelCatalog::default();
    catalog.set_direct(
        ProviderKind::Antigravity,
        FetchedModels::new(
            [
                "claude-sonnet-4-6",
                "gpt-oss-120b-medium",
                "gemini-4-neon-medium",
                "llama-x",
            ]
            .map(str::to_string)
            .to_vec(),
        ),
    );
    if let Some(ids) = anthropic {
        catalog.set_direct(
            ProviderKind::Anthropic,
            FetchedModels::new(ids.iter().map(|id| (*id).to_string()).collect()),
        );
    }
    if let Some(ids) = codex {
        catalog.set_direct(
            ProviderKind::Codex,
            FetchedModels::new(ids.iter().map(|id| (*id).to_string()).collect()),
        );
    }
    catalog
}

/// R3: a bare `claude-*` id goes to anthropic, its maker: with anthropic's
/// list listing it, and with anthropic's list not loaded at all. When the
/// loaded list lacks it and another provider serves it, the answer names that
/// prefixed id; with no anthropic account, too.
#[test]
fn a_bare_claude_id_goes_to_anthropic_or_names_who_serves_it() {
    let providers = BTreeMap::new();
    let listed = origin_catalog(Some(&["claude-sonnet-4-6"]), None);
    assert_eq!(
        listed.resolve("claude-sonnet-4-6", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::anthropic()))
    );
    let unloaded = origin_catalog(None, None);
    let empty = origin_catalog(Some(&[]), None);
    assert_eq!(
        empty.resolve("claude-sonnet-4-6", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::anthropic()))
    );
    assert_eq!(
        unloaded.resolve("claude-sonnet-4-6", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::anthropic()))
    );
    let elsewhere = origin_catalog(Some(&["claude-opus-5"]), None);
    let refused = elsewhere
        .resolve("claude-sonnet-4-6", &providers, MakerPool::Serving)
        .expect_err("anthropic does not list it");
    assert!(refused.contains("not served by anthropic"), "{refused}");
    assert!(
        refused.contains("antigravity/claude-sonnet-4-6"),
        "{refused}"
    );
    let no_account = unloaded
        .resolve("claude-sonnet-4-6", &providers, MakerPool::Empty)
        .expect_err("no anthropic account");
    assert!(
        no_account.contains("antigravity/claude-sonnet-4-6"),
        "{no_account}"
    );
}

/// R3: an id its maker's list does not name, and no one else lists, still goes
/// to its maker — a client shorthand (`:high`) or a model newer than the last
/// refresh — rather than being refused.
#[test]
fn a_bare_id_no_one_else_lists_still_goes_to_its_maker() {
    let providers = BTreeMap::new();
    let catalog = origin_catalog(Some(&["claude-opus-5"]), Some(&["gpt-5.5"]));
    assert_eq!(
        catalog.resolve("claude-opus-5:high", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::anthropic()))
    );
    assert_eq!(
        catalog.resolve("gpt-5.7", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::codex()))
    );
}

/// R3: a bare `gpt-oss-*` id is an `OpenAI` model, so it goes to codex, never to
/// antigravity, whose list names it. With or without a codex account, the
/// answer says which prefixed id serves it.
#[test]
fn a_bare_gpt_oss_id_goes_to_codex_or_names_antigravity() {
    let providers = BTreeMap::new();
    let catalog = origin_catalog(None, Some(&["gpt-5.5"]));
    let with_account = catalog
        .resolve("gpt-oss-120b-medium", &providers, MakerPool::Serving)
        .expect_err("codex does not serve it");
    assert_eq!(
        with_account,
        "gpt-oss-120b-medium is not served by codex; use antigravity/gpt-oss-120b-medium"
    );
    let without_account = catalog
        .resolve("gpt-oss-120b-medium", &providers, MakerPool::Empty)
        .expect_err("no codex account");
    assert!(without_account.contains("codex"), "{without_account}");
    assert!(
        without_account.contains("antigravity/gpt-oss-120b-medium"),
        "{without_account}"
    );
    // The prefixed id is served where it says.
    assert_eq!(
        catalog.resolve(
            "antigravity/gpt-oss-120b-medium",
            &providers,
            MakerPool::Empty
        ),
        Ok(Some(ProviderId::antigravity()))
    );
}

/// R3: every bare `gemini-*` id is antigravity's, listed or not.
#[test]
fn a_bare_gemini_id_goes_to_antigravity() {
    let providers = BTreeMap::new();
    let catalog = origin_catalog(None, None);
    for id in ["gemini-4-neon-medium", "gemini-3-flash"] {
        assert_eq!(
            catalog.resolve(id, &providers, MakerPool::Serving),
            Ok(Some(ProviderId::antigravity())),
            "{id}"
        );
    }
    // C1: with no antigravity account, the refusal says which login is missing.
    let refused = catalog
        .resolve("gemini-3-flash", &providers, MakerPool::Empty)
        .expect_err("no antigravity account");
    assert!(
        refused.contains("pengepul login --provider antigravity"),
        "{refused}"
    );
}

/// R3: a prefixed id goes to that provider exactly, whoever makes the model.
#[test]
fn a_prefixed_id_goes_to_its_prefix() {
    let providers = BTreeMap::new();
    let catalog = origin_catalog(Some(&["claude-sonnet-4-6"]), None);
    assert_eq!(
        catalog.resolve(
            "antigravity/claude-sonnet-4-6",
            &providers,
            MakerPool::Empty
        ),
        Ok(Some(ProviderId::antigravity()))
    );
    assert_eq!(
        catalog.resolve("anthropic/claude-sonnet-4-6", &providers, MakerPool::Empty),
        Ok(Some(ProviderId::anthropic()))
    );
}

/// R3: an id no maker rule claims keeps the live-catalog routing, and one no
/// list names stays unknown.
#[test]
fn an_id_of_no_known_maker_keeps_catalog_routing() {
    let providers = BTreeMap::new();
    let catalog = origin_catalog(None, None);
    assert_eq!(
        catalog.resolve("llama-x", &providers, MakerPool::Empty),
        Ok(Some(ProviderId::antigravity()))
    );
    assert_eq!(
        catalog.resolve("deepseek-x", &providers, MakerPool::Serving),
        Ok(None)
    );
}

/// A catalog with a configured `opencode` listing `ids` bare.
#[cfg(test)]
fn with_configured(mut catalog: ModelCatalog, ids: &[&str]) -> ModelCatalog {
    catalog.set_generic(
        "opencode",
        FetchedModels::new(ids.iter().map(|id| (*id).to_string()).collect()),
    );
    catalog
}

/// S1: anthropic lists a pre-4.6 model only by its dated id, and serves the
/// dateless alias too. A configured provider that lists the alias does not take
/// it from anthropic; nor does a dated id anthropic lists dateless, a `[1m]`
/// marker, or a case the list does not use (N3).
#[test]
fn a_maker_serves_the_dated_and_dateless_forms_of_what_it_lists() {
    let providers = BTreeMap::new();
    let catalog = with_configured(
        origin_catalog(Some(&["claude-haiku-4-5-20251001", "claude-opus-5"]), None),
        &[
            "claude-haiku-4-5",
            "claude-opus-5-20260101",
            "claude-haiku-4-5[1m]",
            "Claude-Haiku-4-5",
        ],
    );
    for id in [
        "claude-haiku-4-5",
        "claude-opus-5-20260101",
        "claude-haiku-4-5[1m]",
        "Claude-Haiku-4-5",
    ] {
        assert_eq!(
            catalog.resolve(id, &providers, MakerPool::Serving),
            Ok(Some(ProviderId::anthropic())),
            "{id}"
        );
    }
}

/// S3: a configured provider's listing is one the refusal names; with it the
/// only other lister, the 404 says `opencode/<id>`.
#[test]
fn the_refusal_names_a_configured_lister() {
    let providers = BTreeMap::new();
    let catalog = with_configured(
        origin_catalog(Some(&["claude-opus-5"]), None),
        &["claude-x-1"],
    );
    let refused = catalog
        .resolve("claude-x-1", &providers, MakerPool::Serving)
        .expect_err("anthropic does not list it");
    assert_eq!(
        refused,
        "claude-x-1 is not served by anthropic; use opencode/claude-x-1"
    );
}

/// S2: a prefixed id the operator's allow list refuses is not offered; with no
/// allowed lister left, the id goes to its maker as if no one else listed it.
#[test]
fn the_refusal_offers_only_ids_the_allow_lists_admit() {
    let providers = BTreeMap::new();
    let mut catalog = with_configured(
        origin_catalog(Some(&["claude-opus-5"]), None),
        &["claude-x-1"],
    );
    catalog.set_allow(BTreeMap::from([(
        "opencode".to_string(),
        std::collections::BTreeSet::from(["llama-3.3-70b".to_string()]),
    )]));
    assert_eq!(
        catalog.resolve("claude-x-1", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::anthropic()))
    );
    let refused = catalog
        .resolve("claude-x-1", &providers, MakerPool::Empty)
        .expect_err("no anthropic account");
    assert!(!refused.contains("opencode/"), "{refused}");
}

/// S3: the antigravity floor is not a list the upstream gave, so it cannot say
/// antigravity lacks an id; a configured lister of a Gemini id does not take
/// it while only the floor is in place.
#[test]
fn the_floor_never_says_the_maker_lacks_an_id() {
    let providers = BTreeMap::new();
    let mut catalog = with_configured(ModelCatalog::default(), &["gemini-3-flash"]);
    catalog.set_floor(ProviderKind::Antigravity, antigravity_static_models());
    assert_eq!(
        catalog.resolve("gemini-3-flash", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::antigravity()))
    );
}

/// S3: grok's static list is not fetched, so it cannot say grok lacks an id.
#[test]
fn grok_s_static_list_never_says_grok_lacks_an_id() {
    let providers = BTreeMap::new();
    let mut catalog = with_configured(ModelCatalog::default(), &["grok-code"]);
    catalog.set_direct(ProviderKind::Grok, grok_static_models());
    assert_eq!(
        catalog.resolve("grok-code", &providers, MakerPool::Serving),
        Ok(Some(ProviderId::grok()))
    );
}

/// C1 and N4: a bare id whose maker's pool cannot serve is a refusal that
/// says why — no account, or every account disabled — and what to do, even
/// when no other provider lists it.
#[test]
fn a_maker_pool_that_cannot_serve_is_named_in_the_refusal() {
    let providers = BTreeMap::new();
    let catalog = origin_catalog(None, None);
    assert_eq!(
        catalog.resolve("claude-opus-5", &providers, MakerPool::Empty),
        Err(
            "claude-opus-5 is not served by anthropic: no anthropic account; \
             run `pengepul login --provider anthropic`"
                .to_string()
        )
    );
    let disabled = catalog
        .resolve("claude-opus-5", &providers, MakerPool::AllDisabled)
        .expect_err("every anthropic account disabled");
    assert!(
        disabled.contains("every anthropic account is disabled"),
        "{disabled}"
    );
    // With another lister, the refusal names it instead of the login.
    let elsewhere = catalog
        .resolve("claude-sonnet-4-6", &providers, MakerPool::Empty)
        .expect_err("no anthropic account");
    assert_eq!(
        elsewhere,
        "claude-sonnet-4-6 is not served by anthropic: no anthropic account; \
         use antigravity/claude-sonnet-4-6"
    );
}
