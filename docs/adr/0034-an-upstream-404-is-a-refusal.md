# 34. An upstream 404 is a refusal

Status: Accepted

## Context

`record_provider_failure` booked a 400 or 402 as a Refusal and sent every other
status through `classify_status`. That function names 401, 403, 429 and 5xx and
gives everything else `network`, so an upstream 404 became a `network` failure:
a failure streak, a `lastError`, and a Cooldown whenever the Pool had a sibling.

A 404 says nothing about the account. Cloud Code answers it for a Gemini id the
account's catalog does not hold, and anthropic, codex, grok and configured
endpoints answer it for a model or path they do not serve. The same request from
every account in the Pool gets the same answer. Booking it as a failure parked a
healthy account per typo or retired id, and once the bare-id origin rule sent an
unlisted `gemini-*` id to Cloud Code, a bare id a client mistyped was enough.

## Decision

An upstream 404 is a Refusal on every provider's path, as a 400 is: the request
counts as a failed request, so `requests` still equals `successes + failures`, and
the account's health is untouched: no Cooldown, no failure streak, no `lastError`.
The client gets the upstream's 404 as it came. A 404 is not retried on another
account (`should_retry_upstream_status` never listed it).

## Consequences

- A typo in a model id costs one upstream round trip and nothing else.
- An account an upstream answers 404 for every request (a revoked workspace that
  answers 404 rather than 403) is no longer cooled by the relay. Its requests fail
  with the upstream's 404, which names the cause.
