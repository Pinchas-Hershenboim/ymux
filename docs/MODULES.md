# Modules → moved to the vault

This file's job — per-file responsibilities, "where does X live" — is now
[`docs/vault/`](./vault/INDEX.md), and unlike this file the vault is enforced:
`scripts/vault-check.mjs` hashes every source file a vault page claims to cover and
`ci-windows.yml` fails when the prose and the code drift apart.

**Start at [`docs/vault/INDEX.md`](./vault/INDEX.md).**

## Why it moved

This page was the same idea, written once in 2025 and never checked. By 2026-08-23 it
claimed `lib.rs (~1760 lines)`, `rpc_server.rs (~487)` and `remote_bootstrap.rs (~285)`
against real values of **12,475 / 2,559 / 83**. It covered 11 frontend files out of 153,
none of the eight `ymux-*` crates, and none of the Go server.

Nothing was wrong with the writing. What was missing was anything that would notice when
it stopped being true — which is the whole point of the gate that replaced it.

Its still-accurate content lives on in `docs/vault/backend-*.md` and
`docs/vault/frontend-*.md`. The rest is in `git log`, where a wrong description cannot
mislead anyone into thinking it describes the present.
