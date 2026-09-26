# disable-an-account

## Goal

The operator can take an Account out of its Pool and put it back, and can make an Account on
Cooldown serve again at once: `pengepul accounts disable <id>` and
`pengepul accounts enable <id>`.

The operator was doing the first by hand: renaming a credential to `<id>.json.dead-<ts>` and
restarting, which the relay then treated as a removed credential (ADR-0026). There was no way
at all to do the second short of a restart.

## Non-goals

- **No timed disable.** Disabled lasts until `enable` or a fresh `pengepul login`; nothing
  expires it.
- **No `force` verb and no pin.** Rotation's policy is unchanged; `enable` does not make an
  account preferred.
- **No new file.** The credential's own name is the state: `<id>.json` is enabled,
  `<id>.json.disabled` is Disabled. There is no `disabled.json`.
- **No change to `.dead-*` files or any other name.** The loader reads `*.json` and
  `*.json.disabled`; everything else in a provider directory stays ignored.
- **`enable` does not repair a credential.** A Reauth account still needs `pengepul login`; a
  removed credential cannot be enabled.
- **No change to Rotation, Failover or the Cooldown policies** beyond Disabled accounts never
  being handed a request.
- **No confirmation prompt,** including for the last enabled account of a Pool.
- **`--reload` and `--verbose` do not combine with `disable` or `enable`;** the CLI refuses
  the pair rather than ignore the flag.

## Acceptance criteria

- AC-1: `disable` renames the account's credential from `<id>.json` to `<id>.json.disabled`
  in its provider directory, and from then on neither Rotation, conversation affinity nor
  Failover hands it a request. A request already in flight on it completes.
- AC-2: `enable` on a Disabled account renames the file back and returns it to Rotation.
- AC-3: `enable` on an account on Cooldown clears the Cooldown and resets the failure streak,
  so it is selectable on the next request and its next failure earns the base Cooldown.
- AC-4: `enable` on an available account prints `already available`; `disable` on a Disabled
  one prints `already disabled`. Both exit 0 and touch nothing.
- AC-5: `enable` on an account in Reauth says the account still needs the login that restores
  it — `pengepul login --provider <provider>`, with `--key <key>` for a configured Provider —
  whether it renamed the account back, cleared a Cooldown, or found nothing to clear (a Pool
  of one earns no Cooldown), and says "cleared" only when a Cooldown was active. `enable` or
  `disable` on an account whose credential is gone exits non-zero naming the same login. An
  id no Pool holds exits non-zero, saying `pengepul accounts` lists them; a `--provider` no
  Pool has exits non-zero naming the pools there are.
- AC-6: The provider is inferred from the id. An id held by two Pools exits non-zero naming
  both, and `--provider <provider>` selects one.
- AC-7: Disabling the last enabled account of a Pool succeeds and prints a warning that the
  provider's requests fail until an account is enabled.
- AC-8: The state survives a restart: a relay started with `<id>.json.disabled` on disk lists
  the account as Disabled and never hands it a request.
- AC-9: `pengepul login` for a Disabled account writes `<id>.json` and removes
  `<id>.json.disabled`, leaving the account enabled.
- AC-10: `accounts --reload` reconciles every Pool with its directory: an account whose
  `<id>.json` was renamed to `<id>.json.disabled` by hand becomes Disabled, and one whose
  credential is gone entirely becomes the removed-credential record ADR-0026 describes.
- AC-11: A Disabled account keeps its counters and its `lastSuccessAt`, is listed by
  `accounts` and counted by `status` and `usage` like any other account. The payload carries
  `disabled: true` and `available: false`; the account row prints `disabled` in both styles, and
  the Pool header's available count excludes it.
- AC-12: A Pool whose only enabled account sits beside Disabled ones earns no Cooldown for it
  (ADR-0027).
- AC-13: A Refresh cannot recreate `<id>.json` for an account that was disabled: the rename and
  the Refresh's write are serialized on the Pool.
- AC-14: `POST /admin/accounts/disable` and `POST /admin/accounts/enable` take
  `{"account": "<id>", "provider": "<provider>"?}`, require the local API key, and answer with
  the outcome the CLI prints.

## Verification

```
cargo test --locked disable
cargo test --locked enable
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
pengepul accounts disable <id>; pengepul accounts; pengepul service restart; pengepul accounts; pengepul accounts enable <id>
```
