# readme-subscription-pool

## Goal
Rewrite `README.md` around pengepul's real identity: an API relay you host that pools
your AI subscription accounts and serves them as one API reachable locally or over your
network, so your own harness runs on your Claude and ChatGPT/Codex plans instead of a
per-token API key. Highlight three
features (pool accounts, keep subscriptions working in openclaw and hermes, use as a
REST API for your own harness), with Claude highlighted first and Codex claimed for all
three.

## Non-goals
- No code, config schema, CLI, or behavior change. `README.md` is the only file edited.
- Do not explain the cloaking mechanism, the billing classifier, or "first-party"
  framing. The sell is benefit-only: describe what each feature does, not why the
  vendor would otherwise reject the traffic.
- Do not drop the openclaw/hermes working-config caveats (see AC-5, AC-6).
- Do not use em dashes anywhere in the file.

## Acceptance criteria
- AC-1: The README opens with a hero that frames pengepul as an API relay you host,
  pooling your subscriptions and reachable locally or over your network, running your
  own harness on your Claude and ChatGPT/Codex plans without a per-token API key, and
  does not name the classifier, "first-party", or "cloaking".
- AC-12: The README documents serving on a non-localhost host for remote reach, with a
  one-line caveat that the local API key is the only guard when exposed.
- AC-2: The three features are each stated plainly near the top: (1) pool several
  subscription accounts per provider, (2) keep subscriptions working in openclaw and
  hermes without an API key, (3) consume the pool as a REST API to build your own
  harness without an API key.
- AC-3: Claude is presented before Codex. Codex is documented for all three features:
  it is a poolable OAuth subscription, works in a compatible harness, and is reachable
  as a REST API via a `gpt-5`/`codex-*` model on the OpenAI-shaped routes.
- AC-4: `grep -iE 'opencode' README.md` returns nothing.
- AC-5: The openclaw section keeps its config block and the caveat that a bare
  `claude-…` model bypasses pengepul; its JSON block parses.
- AC-6: The hermes section keeps its config block and the caveat to use
  `provider: pengepul` (not `anthropic`) or requests reach `api.anthropic.com`.
- AC-7: A "build your own harness" section documents pointing an Anthropic-API client
  (Claude, `/v1/messages`) and an OpenAI-API client (Codex, `gpt-5` on
  `/v1/chat/completions` or `/v1/responses`) at pengepul with the local API key.
- AC-8: The README contains no em dash character.
- AC-9: A compact reference still covers routes with both auth-header forms, model
  aliases and the default model, the settable config keys (framed as settable, not a
  verbatim generated file), pool rotation/backoff/failover, token refresh, service, and
  logging.
- AC-10: `README.md` is between 150 and 200 lines.
- AC-11: No banned house-style words (per global CLAUDE.md); file is well-formed
  GitHub-flavored Markdown (fences balanced).

## Verification
grep -niE 'opencode' README.md; test $? -eq 1
grep -niE 'classifier|first-party|cloak' README.md | sed -n '1,5p'
grep -nF 'em dash check' /dev/null; python3 -c "import sys; s=open('README.md').read(); sys.exit(1 if '—' in s else 0)" && echo "no em dash"
python3 -c "import re,json; s=open('README.md').read(); [json.loads(b) for b in re.findall(r'```json\n(.*?)```', s, re.S)]; print('json parses')"
grep -niE 'provider: pengepul|bypass|gpt-5|/v1/messages' README.md
wc -l README.md
# Runtime item (user to run, not proven here): point an OpenAI-API client at pengepul
# with a gpt-5 model and confirm it serves from a live Codex subscription account.
