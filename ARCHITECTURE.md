# Architecture

## What this is

pengepul is a local relay. It pools subscription **Accounts** per **Provider** —
anthropic, codex, and OpenAI-compatible endpoints configured in `config.yaml` —
behind one REST surface, so a harness runs on a subscription instead of a
per-token key. It authenticates each caller by a **Local API key**, **Cloaks**
anthropic- and codex-bound requests so the vendor **Classifier** reads them as
first-party CLI traffic, and **Translates** between the dialect a client speaks
and the dialect each Provider speaks upstream. It is a single crate with no
database: account health, rotation and the provider list all live in memory or
in files.

## Modules

- **Relay** (`app.rs`) — the fixed route table, the request path, the
  `UpstreamClient` trait and its HTTP implementation, and **Failover** across the
  Accounts of one Provider. One route accepts exactly one Inbound dialect and no
  route keys off client identity.
- **Account** (`accounts.rs`) — `AccountManager`: holds every Account of one
  Provider and picks who serves next; **Rotation**, **Cooldown** and due
  **Refresh** live here.
- **Credential store** (`tokens.rs`) — reads and writes the one credential an
  Account holds, under the auth dir at `0600`; knows nothing of selection.
- **OAuth** (`oauth.rs`) — mints and Refreshes the anthropic and codex
  credential, and is the only place a rejected refresh token becomes **Reauth**.
- **Cloaking sanitizer** (`masquerade.rs`) — strips a harness's bot-identity
  system sections, `PascalCase`s its tool names, and rewrites the literal phrases
  that trip the Classifier (openclaw sections and tool names; pi and opencode
  fingerprints). Returns the reverse map that restores tool names on the reply.
- **Upstream + vendor identity** (`upstream.rs`) — owns every outbound vendor
  call behind one trait, and `apply_cloaking` injects the billing-header block,
  the "You are Claude Code" prefix, the account metadata and the identifying
  headers. `cloaking_versions.rs` supplies the vendor CLI versions it learns at
  runtime.
- **Model catalog** (`models.rs`) — resolves a model id to exactly one Provider,
  and advertises configured endpoints under a `<provider>/` prefix.
- **Translation** (`translate.rs`, `streaming.rs`) — rewrites a body between
  Inbound and upstream **Dialect**, whole-document and one SSE event at a time;
  pure JSON, no I/O.
- **Config** (`config.rs`) — parses `config.yaml`, including the `providers:`
  section, which is the only Provider registry: there is no database table.
- **CLI + Runtime + Service** (`cli.rs`, `runtime.rs`, `service.rs`) — command
  parsing (pure), the `CliRuntime` adapter that makes a verb touch the real world,
  and the per-user systemd/launchd unit.

## Seams

- **`UpstreamClient`** — the test seam: which vendor wire is spoken and whether
  any HTTP happens at all. `apply_cloaking` runs *below* it inside the HTTP
  client, so a test double sees the pre-cloak body and headers.
- **`ProviderKind`** — a closed enum (Anthropic, Codex, Generic) that switches
  the credential lifecycle, the OAuth flow, and whether Cloaking runs; the
  compiler is the checklist when a fourth kind arrives.
- **`CliRuntime`** — every side effect a CLI verb performs, so `cli.rs` stays
  pure argument handling.
- **Classifier-rewrite tables** — per-harness knowledge is a table entry, not an
  edit to the request path: openclaw's sections and tripping text, and the pi /
  opencode fingerprint rewrites.

## Invariants

- **Cloaking runs in two layers.** The sanitizer (`masquerade_request`) runs on
  the `/messages` route only; the vendor-identity inject (`apply_cloaking`) runs
  inside the Upstream client for anthropic on every dialect. A Chat- or
  Responses-shaped request to an anthropic model therefore gets the identity
  headers but not the sanitizer — a known gap. codex is header-only; a configured
  OpenAI-compatible endpoint is never Cloaked; `count_tokens` gets identifying
  headers but no body Cloaking.
- **Tool-name rewrites are bijective within a request and restored before the
  reply reaches the client**, on the Messages route where the sanitizer applied
  them.
- **Nothing observes the Classifier.** Every table entry is bisected offline
  against live traffic; an under-strip surfaces only as an upstream 400 with no
  log line, so the rules bias toward failing loud over deleting operator content.
- **One model id resolves to exactly one Provider**, and an id nobody claims is
  refused 400 before any Account is touched. A bare id never routes to a
  configured endpoint — only an explicit `<provider>/` prefix does.
- **Failover only moves a request between Accounts of the same Provider**,
  resolved once before the attempt loop.
- **Every route authenticates before it parses a body.** `/health` is the only
  unauthenticated route.
- **One Account holds exactly one credential**, on disk at `0600` and never
  elsewhere. **Cooldown** clears only on success, a completed Refresh, or a
  reload that sees a changed credential.
- **The Provider registry is the `config.yaml` `providers:` section**, read at
  startup; there is no database and nothing on the serving path writes it.
- **A (Inbound dialect, Provider) pair the relay cannot serve is refused 501 at
  routing**, never sent upstream and never retried.
