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
  **Refresh** live here, and every request outcome updates the Account's
  Usage counters, which it hands to the Credential store to persist. Every
  outcome passes through one private seam (`AccountState::settle`), so the
  attempt/outcome invariant holds by construction rather than by each
  recorder remembering it; counters read from disk are repaired at load,
  where nothing is in flight.
- **Credential store** (`tokens.rs`) — reads and writes what an Account
  leaves on disk under the auth dir: the one credential it holds, and the
  provider's **Usage counters** file (`usage.json`), both at `0600`, the
  latter written atomically (temp + rename); knows nothing of selection.
  (With default paths `config.yaml` and, transiently during a
  registration, `config.yaml.lock` sit there too. The only other
  long-lived file under the auth dir, `cloaking-versions.json`, is the
  Upstream module's own cache — `cloaking_versions.rs`.)
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
  and advertises every served model under its `<provider>/` prefix with the
  per-model metadata the client needs (context window, output cap, modalities,
  pricing).
- **Translation** (`translate.rs`, `streaming.rs`) — rewrites a body between
  Inbound and upstream **Dialect**, whole-document and one SSE event at a time;
  pure JSON, no I/O. Every pair the route table can produce has a translation,
  Messages↔Chat Completions included, so a configured endpoint serves a
  Messages client by translation rather than by refusal.
- **Config** (`config.rs`) — parses `config.yaml`, including the `providers:`
  section, which is the only Provider registry: there is no database table.
  It also writes it: `register_provider` adds one entry for `login
  --base-url`, and refuses an id already present with a different URL
  rather than overwriting it. The write is a read-modify-write on one
  shared file, so it takes a `<config>.lock` beside the config for the
  length of it; two registrations racing each other would otherwise lose
  one silently.
- **CLI + Runtime + Service** (`cli.rs`, `runtime.rs`, `service.rs`) — command
  parsing and dispatch (pure), the `CliRuntime` adapter that makes a verb touch
  the real world, and the per-user systemd/launchd unit — including the parser
  that turns the platform tool's status text into panel rows. `launch` is
  dispatch too: `launch_plan` turns a harness name into the binary, the
  arguments and the environment that point it at the relay, and the runtime
  `exec`s that plan — the whole of the per-harness knowledge is that one
  table. Its model picker splits the same way: `cli.rs` decides which rows
  exist and which the harness cannot be given (`model_choices`) and owns
  the search rule (`matching_choices`); the runtime fetches `/v1/models`
  and drives the keys, so what a test asserts is the list that was
  offered and the row that came back.
- **Render** (`render.rs`) — the panel language every verb prints with: the
  64-column box, the `Fact` row (`<label>  <value>`) and `fact_panel` that
  every rich *fact* surface is built from, the three-color palette, glyphs,
  number formats, and the `Style` (rich on a color TTY, plain otherwise)
  that `main.rs` decides once at the edge. Knows nothing of Pools,
  Accounts, or the admin payload.
- **Usage view** (`usage_view.rs`) — the admin payload turned into the relay
  total block for `status` (one block: pool summary lines and the relay-wide
  aggregate), pool panels with account rows, per-model lines and footers for
  `accounts`, and the 30-day sparkline for `usage`, in both styles. Pure over
  the payload and a `now` or a date the verb hands in.

## Seams

- **`UpstreamClient`** — the test seam: which vendor wire is spoken and whether
  any HTTP happens at all. `apply_cloaking` runs *below* it inside the HTTP
  client, so a test double sees the pre-cloak body and headers.
- **`ProviderKind`** — a closed enum (Anthropic, Codex, Generic) that switches
  the credential lifecycle, the OAuth flow, and whether Cloaking runs; the
  compiler is the checklist when a fourth kind arrives.
- **`CliRuntime`** — every side effect a CLI verb performs, so `cli.rs` stays
  pure argument handling.
- **`Style`** — decided once from the TTY and environment in `main.rs` and
  handed down; no renderer reads the environment, so tests drive either mode
  hermetically and piped output stays byte-stable for scripts.
- **Classifier-rewrite tables** — per-harness knowledge is a table entry, not an
  edit to the request path: openclaw's sections and tripping text, and the pi /
  opencode fingerprint rewrites.

## Invariants

- **A configured endpoint serves every dialect its models can carry.** It
  speaks only Chat Completions upstream, so a Messages request is translated
  onto it — request, whole response, and the SSE stream — rather than refused.
  Responses and `count_tokens` stay 501 there: no client asks for the first,
  and the second is anthropic's own endpoint.
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
  One CLI verb does: `login --base-url` registers a new Provider, and only
  a new one — an id already present with a different `base-url` is an
  error naming the URL it kept, so a mistyped flag cannot move a live
  Provider's traffic to another host. Changing an endpoint stays an edit.
- **Usage counters survive a restart; Cooldown does not.** Requests, successes,
  failures and tokens per Account — per model within an Account for the
  successes, and per local calendar day — are written to `usage.json` after
  every outcome, Refusals included and reloaded at startup; a fresh process always retries every
  Account. Daily buckets are trimmed to 90 days on write. Deleting the file
  is the only reset.
- **A counter counts outcomes, never attempts.** Every recorder settles
  through one private seam (`AccountState::settle`), which increments
  `requests` in the same call that increments `successes` or `failures`,
  cumulatively and in the day the outcome arrived. So
  `requests == successes + failures` holds by construction rather than by
  repair: no interleaving, no concurrent outcome and no future call site
  can put them out of step, and the seam never needs to know which
  attempt it is settling (ADR-0015). A request still in flight is counted
  nowhere, and an attempt lost to a crash is never counted at all: the
  relay counts what it observed. The seam does not refuse a second
  outcome — a double-recording path inflates `requests` rather than
  corrupting the balance, and a caller that must not record twice (the
  billing path) applies health without an outcome. Every path between
  selection and the response records a success, a failure, or a Refusal —
  including a client that hangs up mid-stream, whose outcome is paid by a
  `Drop` guard because the code after the stream loop never runs. A
  Refusal counts as a failed request but earns no Cooldown: a dialect the
  Provider cannot serve, or a 400 the client malformed, is not the
  Account's fault. Counters written by the earlier scheme, which counted
  at dispatch, are repaired once at load and never again: a gap can no
  longer be created, so the repair is a migration with an end.
- **A number the panels cannot attribute is not invented.** Every success
  opens a per-model counter, so the relay cannot create an unattributed
  gap: the model rows sum to their account by construction. Tokens
  recorded before per-model tracking existed were assigned by the
  operator, once, on the file — an operator supplying a fact the relay
  never captured, not the panel guessing one. The panel no longer names a
  remainder: a gap can now only come from restoring a pre-migration file,
  and it is silent by the operator's choice.
- **One word, one scope.** Two verbs never print the same label for different
  spans: `status` totals all time, `usage` names its `window` and its
  `all time` separately, and the all-time figure both print comes from one
  sum over one payload, so they cannot drift. A row reports a fact the panel
  does not already carry — it never restates another row or narrates another
  verb.
- **Every rich panel speaks one grammar.** Header is `<subject>` or
  `<subject> ─ <qualifier>`, never a colon, and a qualifier must add a
  fact the rows do not carry. Fact rows are `<label>  <value>` with the
  label column fitted per panel; the status glyph marks a state value
  only. The exception is deliberate: `accounts` also carries *list* rows
  (account and model tables) with their own fitted columns, and is the
  one panel built by hand rather than by `fact_panel`, because it needs a
  mid-panel separator between its list and its rollup. Plain output is a
  separate contract: it stays byte-stable for scripts and does not follow
  the panel language.
- **Cloaking follows Claude Code except where fidelity breaks the client.**
  The beta set is audited against the current CLI binary, but
  `redact-thinking` is never sent (it empties thinking text pengepul's clients
  display) and `web-fetch` stays for the native tool swap — ADR-0014.
- **A (Inbound dialect, Provider) pair the relay cannot serve is refused 501 at
  routing**, never sent upstream and never retried.
- **`launch` leaves nothing behind.** A harness is pointed at the relay by the
  environment and the arguments of one process, never by a file: the same
  binary started any other way still resolves its own accounts and its own
  models. That is what bounds the verb to harnesses which can be redirected
  per-process — openclaw and hermes can only be redirected in their config
  files, so they stay the README's manual step (ADR-0007 still holds: the
  client adapts, and `launch` only writes that adaptation into one process
  instead of onto disk). It is also why `launch claude --model` refuses a
  configured provider. The picker
  offers every advertised model to either harness, because every one of them
  can be run: the relay translates Messages onto a configured endpoint rather
  than refusing it. It opens nothing at all when stdout is not a terminal,
  where raw mode would seize a terminal nobody is watching.
