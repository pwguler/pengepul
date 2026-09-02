# workspace-split

## Goal

Restructure the repository into a Cargo workspace with the Rust relay under `core/` and a
Next.js + Prisma application under `web/`, and land the settled database schema, without
changing a single byte of relay behaviour.

## Non-goals

- No change to relay behaviour, routes, response shapes, or the published binary name.
  The binary stays `pengepul` and the release asset names stay identical.
- No Rust code reads or writes the database in this work item. The schema exists; core
  does not yet know about it.
- No UI pages beyond what proves the app builds and serves. No console screens.
- No migration is run against anything but a local development database. The command is
  named in this spec and stopped there; running it elsewhere is the operator's.
- No dependency added to the Rust crate.

## Acceptance criteria

- AC-1: The root `Cargo.toml` is a virtual manifest with `members = ["core"]`, and
  `core/Cargo.toml` declares `name = "pengepul"` at the version the tree had before the move.
  - Verify: `cargo metadata --no-deps --format-version 1 | grep -o '"name":"pengepul"'`
- AC-2: `cargo test --locked` from the repo root passes with the same count as before the
  move (176 at time of writing), and `cargo fmt --check` and
  `cargo clippy --all-targets --all-features --locked -- -D warnings` are clean.
- AC-3: `target/` resolves to the workspace root, so `release.yml`'s
  `tar czf ... -C target/<triple>/release pengepul` needs no edit.
  - Verify: `cargo build --locked --release && test -x target/release/pengepul`
- AC-4: `cargo pkgid --locked -p pengepul` prints the version in `core/Cargo.toml`, and
  `release.yml`'s tag-check step uses that `-p` form.
- AC-5: `scripts/install.sh`'s build-from-source line names the package, so
  `cargo install --git <repo> pengepul --locked` resolves against the workspace.
- AC-6: The Rust move is a pure rename: `git diff -M --stat main` reports renames and the
  two manifest edits, and no content change under `core/src/` or `core/tests/`.
- AC-7: `web/prisma/schema.prisma` declares `Key`, `KeySpend`, `UsageEvent`, `RequestLog`,
  `Provider` and `Model` with the fields, uniques and indexes named in Data below, and
  `pnpm prisma validate` passes. Operator settings are not in the database; they stay in
  `config.yaml`.
- AC-8: `web/` installs and builds: `pnpm install && pnpm build` succeeds and the dev server
  serves a page.
- AC-9: `.gitignore` excludes `web/node_modules`, `web/.next`, and the SQLite database and
  its `-wal`/`-shm` sidecars.

## Data

The schema this work item lands. `Key.keyHash` is SHA-256 of the presented key; the full key
is never stored. Rates on `UsageEvent` are snapshots, so re-pricing a `Model` never moves a
past row.

```prisma
model Key {
  id               String    @id @default(cuid())
  name             String
  owner            String
  keyHash          String    @unique
  keyPrefix        String
  monthlyBudgetUsd Float?
  createdAt        DateTime  @default(now())
  revokedAt        DateTime?
  @@index([owner])
}

model KeySpend {
  keyId     String
  month     String   // "2026-08"
  costUsd   Float    @default(0)
  updatedAt DateTime @updatedAt
  @@id([keyId, month])
}

model UsageEvent {
  id                String   @id @default(cuid())
  keyId             String
  provider          String
  model             String
  inputTokens       Int      @default(0)
  outputTokens      Int      @default(0)
  cacheCreateTokens Int      @default(0)
  cacheReadTokens   Int      @default(0)
  reasoningTokens   Int      @default(0)
  costUsd           Float?
  rateInput         Float?
  rateOutput        Float?
  rateCacheWrite    Float?
  rateCacheRead     Float?
  startedAt         DateTime
  durationMs        Int
  statusCode        Int
  ok                Boolean
  streamed          Boolean  @default(false)
  @@index([keyId, startedAt])
  @@index([startedAt])
}

model RequestLog {
  usageEventId String   @id
  prompt       String
  completion   String
  truncated    Boolean  @default(false)
  createdAt    DateTime @default(now())
  @@index([createdAt])
}

model Provider {
  id        String   @id          // routing prefix: "groq"
  baseUrl   String
  label     String?
  createdAt DateTime @default(now())
}

model Model {
  id              String   @id @default(cuid())
  providerId      String
  modelId         String
  inputPer1M      Float?
  outputPer1M     Float?
  cacheWritePer1M Float?
  cacheReadPer1M  Float?
  @@unique([providerId, modelId])
}
```

## Verification

```sh
cargo metadata --no-deps --format-version 1 | grep -q '"name":"pengepul"'
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --locked --release && test -x target/release/pengepul
cargo pkgid --locked -p pengepul
git diff -M --stat main
cd web && pnpm install && pnpm prisma validate && pnpm build
```

Migration is generated but **not applied** beyond a local development database:
`cd web && pnpm prisma migrate dev --name init` is the operator's to run, and
`pnpm prisma migrate deploy` is how it reaches the box. The relay itself never migrates.

`web/` is additive: the Rust crate has no build-time or runtime dependency on it, and
`cargo build --locked` succeeds in a checkout with `web/` deleted.
