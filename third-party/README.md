# Third-party materials

This directory holds the source material behind [THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md) and the third-party declarations shipped in build artifacts.

## Contents

| Path | Purpose |
| --- | --- |
| `inventory.json` | Production dependency union across the workspace plus copied source/assets, native tools and Rust crates: exact versions, sources, hashes and the notice hash. Its `scope` field documents what it does and does not cover. |
| `copied-components.json` | Source components copied into the repository (revision and upstream source). |
| `embedded-components.json` | Components embedded inside dependencies (parent package, revision and source). |
| `npm-overrides.json` | Overridden npm packages with the exact upstream notice snapshot used for each. |
| `rust-sources.json` | Rust crates linked into `crates/mode-cua-ax`: versions, licence identifiers, Cargo.lock checksums and licence snapshot references (the snapshots live beside the other licence texts in `scripts/license-texts/`). |
| `native-search/` | Native search tool sources and licence material (`sources.json`, `licenses/`). |
| `runtime/` | Node runtime licence texts and source references (`sources.json`). |
| `upstream/` | Byte-exact upstream notice snapshots referenced by the inventory (sha256-named files). |

## Generating and checking

- Regenerate the notice file and verify freshness: `node scripts/licenses.mjs notices`
- Strict check (fails on incomplete or stale material): `node scripts/licenses.mjs notices --strict`
- Rust ledger coverage for shipped targets (report until the M4 regeneration lands):
  `pnpm check:rust-ledger` — see `docs/plans/2026-10-18-m4-third-party-ledger.md`. It exits non-zero
  with `--strict` when a crate in a shipped target's resolve graph is missing from `rust-sources.json`.
- Generate a candidate Rust ledger from cargo metadata plus the registry cache (does **not** overwrite
  this directory by default): `pnpm exec tsx scripts/generate-rust-ledger.ts --out .plan/rust-ledger-candidate`.
  The candidate must be reviewed and regenerated through Linux CI before it is committed.

## Where the notices ship

- Build artifacts keep the applicable `LICENSE` / `NOTICE` / `COPYING` files alongside their other declarations.
- The CLI distribution includes `THIRD-PARTY-NOTICES.md` next to the agent bundle.
- The server packaging stage writes `runtime/THIRD-PARTY-NOTICES.md`, `LICENSE.node.txt` and `NODE-SOURCES.json`, and keeps per-component notices under `licenses/<component>/THIRD-PARTY-NOTICES.md`.

## Limits

The inventory is a conservative union across distributions; not every listed component is included on every platform, and it is not a per-installer SBOM or a certification that all licence obligations are met for every use. Package metadata licence identifiers are descriptive; the original terms always prevail.
