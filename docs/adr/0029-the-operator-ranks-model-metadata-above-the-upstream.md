# 29. The operator ranks model metadata above the upstream

Status: Accepted

An OpenAI-compatible upstream such as omlx lists its models without saying whether they
reason, so a client offers no thinking level for a model that thinks, and pengepul cannot
learn the fact from the wire. The operator states it under `providers.<id>.models` in
`config.yaml`, and that statement wins over the upstream's `/models` body, which wins over
pengepul's curated table, field by field. All three tiers rank in one function,
`ranked_metadata`, so the order is read in one place; that puts the config statement inside
the fetch behind `UpstreamClient`, which is why `HttpUpstreamClient::fetch_models` reads the
provider's `models:` beside its `base-url`.

## Considered options

- **Apply the statement above the seam, at the catalog refresh.** The upstream client would
  carry only what the vendor says, and an app test could drive the override through a fake
  upstream. It lost because the ranking then lives in two places on two sides of the seam,
  and a reader has to find both to know which value wins.
- **Infer reasoning from an unsourced model id** (any id starting `Qwen3`, say). It lost
  because one family holds thinking and non-thinking variants (`Qwen3-VL-8B-Instruct` does
  not think), and an embedding model or a document converter shares the server's list. The
  curated table's `Qwen/Qwen3` entry is a prefix claim too, kept because a vendor source
  backs it for the ids that provider serves; omlx's ids have no such source.

## Consequences

A test double for `UpstreamClient` skips the ranking, so the override is tested on
`parse_openai` and on the real `HttpUpstreamClient` against a local upstream.
