# pengepul

pengepul pools several AI-vendor accounts behind one local API and spreads client requests across them. This glossary fixes the vocabulary an operator of that pool uses, so the same concept is not called three things across the README, the CLI and the source.

## Language

### Providers

**Provider**:
An upstream vendor family pengepul relays to — anthropic, codex, or a configured OpenAI-compatible endpoint (e.g. groq) — and the top-level axis by which accounts, credentials, models and admin output are partitioned.
_Avoid_: kind, provider id, backend, owned_by

**Upstream**:
The vendor side of the relay: the service pengepul calls out to on behalf of a client, as opposed to the client calling pengepul.
_Avoid_: backend, endpoint, provider (when the vendor connection, not the partition, is meant)

### Wire shapes

**Dialect**:
One of the three request and response shapes pengepul speaks — Anthropic Messages, OpenAI Chat Completions, OpenAI Responses.
_Avoid_: format, api, protocol, schema

**Inbound dialect**:
The dialect a client's request arrives in, chosen by the client's tooling and independent of which provider ends up serving it.
_Avoid_: client format, request type

**Translation**:
Rewriting a request from its inbound dialect into the dialect the chosen upstream speaks, and the reply back again.
_Avoid_: conversion, adapter, mapping, shim

### Accounts

**Account**:
One authorized identity at a provider, keyed by email (or by a derived label for static-key providers), holding the credential pengepul rotates through and the usage and failure record kept against it.
_Avoid_: credential, token, token file, snapshot

**Refresh**:
Replacement of an account's expiring access token using its refresh token, done by pengepul without operator involvement.
_Avoid_: renew, re-auth

**Reauth**:
The state an account enters when its refresh token is itself rejected, where nothing but a human re-running `pengepul login` restores it. Static-key accounts never enter Reauth — they have no refresh token — a rejected key only cools down and comes back to fail again until the operator replaces it.
_Avoid_: refresh exhausted, invalid_grant, dead token

**Cooldown**:
A period during which an account is passed over by rotation, entered on failure and cleared by the next success or by reloading accounts after a fresh login.
_Avoid_: backoff, lockout, unavailable, cooling down

**Rotation**:
The policy by which each request is handed the account after the one used last, skipping accounts on cooldown and holding no affinity to a client or session.
_Avoid_: round-robin, load balancing, sticky windows, account selection

**Failover**:
Re-serving one client request on a different account of the same provider after the upstream rejects it in a way another account could survive.
_Avoid_: retry, fallback, attempt budget

**Pool**:
The set of Accounts of one Provider behind the relay, spread across by Rotation.
_Avoid_: account list, fleet, grouping

**Usage counters**:
The running per-account totals — requests, successes, failures, tokens in/out/cache/reasoning — broken down per model for the successes, summed relay-wide in `status` and shown per account in `accounts`, and persisted to `usage.json` in the provider's auth directory so they survive restarts and upgrades. Counters recorded before per-model attribution existed stay only in the account totals. Cooldowns and failure streaks are not persisted: a fresh process always retries.
_Avoid_: stats file, metrics, telemetry

### Access and billing identity

**Local API key**:
A key a client must present to reach pengepul's own routes, distinct in direction and issuer from any credential pengepul presents upstream.
_Avoid_: api key (unqualified), bearer token

**Cloaking**:
Rewriting the system blocks and identifying headers of an outbound request so the classifier reads it as first-party vendor CLI traffic.
_Avoid_: masquerade, spoofing, impersonation, billing header

**Classifier**:
The vendor-side billing check that reads a request's system blocks and CLI identity to decide whether it came from an official vendor CLI or a third-party bridge, charging the latter as extra usage.
_Avoid_: billing classifier, detector, filter

## Relationships

- pengepul has two built-in **Providers** — anthropic, codex — plus one configured **Provider** per `providers:` config entry.
- One **Provider** has zero or more **Accounts**; one **Account** belongs to exactly one **Provider**.
- Within one **Provider** an **Account** is keyed by exactly one email (anthropic/codex) or by a label derived from the key (static-key providers), and keys are unique.
- One **Account** holds exactly one credential: an access-token/refresh-token pair for anthropic and codex, or one static API key for a configured OpenAI-compatible **Provider**.
- One **Account** has at most one **Cooldown** in effect, with one duration policy for ordinary failures and a longer one for **Reauth**.
- One model id resolves to exactly one **Provider**.
- One client request is served by one **Account** at a time, and **Failover** only moves it between **Accounts** of the same **Provider**.
- **Cloaking** applies to requests bound for the anthropic and codex **Upstreams**; configured OpenAI-compatible endpoints are never cloaked. The **Local API key** applies to requests arriving from a client.
- One pengepul endpoint accepts exactly one **Inbound dialect**; one **Provider** accepts exactly one **Dialect** upstream.
- Any **Inbound dialect** may be served by anthropic or codex, and **Translation** is what closes the gap. A configured OpenAI-compatible endpoint accepts only the Chat Completions dialect and answers 501 for the others. count_tokens is anthropic-only and answers 501 elsewhere.

## Example dialogue

> **Dev:** "Half my calls came back 503 for about a minute, then cleared on their own. Did an **account** die?"
>
> **Operator:** "No. One **account** hit a **cooldown** after a 429, so **rotation** skipped it. Two of the three were already busy failing, so there was nothing left to hand the request to."
>
> **Dev:** "But it recovered without me. So why does `pengepul accounts` still show one as unavailable?"
>
> **Operator:** "That one is on the long **cooldown** — the **reauth** one. Same mechanism you just waited out, different cause and a very different duration. `pengepul accounts` prints both as unavailable, so the tell is `lastError` reading `refresh token ...; re-run login`, and a cooldown measured in hours rather than seconds. The short **cooldown** doubles per consecutive failure and caps out in minutes, and any success clears it. **Reauth** means the **upstream** rejected the refresh token itself — **refresh** can't fix it, so that **account** sits out for a day and comes back only to fail again."
>
> **Dev:** "So the 503 and the unavailable line are unrelated?"
>
> **Operator:** "Same mechanism, different cause. Both are a **cooldown**. Only one of them is something you can wait out. Run `pengepul login --provider anthropic` for that email, then `pengepul accounts --reload` — the **cooldown** clears when the new credential is picked up."
>
> **Dev:** "And the request that got the 503 — did it try the other two first?"
>
> **Operator:** "**Failover** tried, yes. It re-serves on a different **account** of the same **provider**, but only for statuses another **account** could plausibly survive. Every **account** was on **cooldown**, so it ran out of candidates and returned what it had."

## Flagged ambiguities

- "provider" names both the vendor and a configured entry for that vendor. Resolved: **Provider** means the vendor. For a configured OpenAI-compatible endpoint, the config entry is the Provider; for anthropic and codex there is no config entry because they are built in.
- "claude" appears as an alias for anthropic in stored credentials. Resolved: **anthropic** is the only spelling an operator uses or types.
- "account", "credential" and "token" all name the same file under the auth directory across the README, the CLI and the source. Resolved: **Account** is the domain noun — the identity, its credential, and its record. Credential is the secret inside an account. Token is a wire artifact and never means the account.
- Accounts are keyed by email. Resolved: read the field as the account key, not as an address.
- "backoff", "lockout" and "unavailable" appear across the README, the CLI and the admin output for one mechanism. Resolved: there is one **Cooldown** with two duration policies. Say "failure cooldown" and "reauth cooldown" when the durations must be distinguished.
- "API key" covers two unrelated secrets: the keys clients present to pengepul, and the credentials pengepul presents upstream. Resolved: **Local API key** is what clients present; the upstream credential is what pengepul presents upstream. They point in opposite directions on the wire.
- "cloaking" and "masquerade" are used interchangeably. Resolved: **Cloaking** is the domain term.
- "refresh" names both the secret an account holds and the act of replacing its expiring access token. Resolved: **Refresh** is the act. The secret is the refresh token.
- "round-robin with sticky windows" survives in `docs/`, contradicting the README. Resolved: **Rotation** has no stickiness. Selection resumes after the last-used account and advances on every request, with no client or session key anywhere.
- "pool" and "pooling" name both the whole relay and one provider's accounts. Resolved: **Pool** is one Provider's set of Accounts; the process serving every pool is the relay.
- The CLI printed cooldown accounts as "unavailable". Resolved: operator-facing output says "on cooldown" with the remaining time; "unavailable" is on the avoid list.
