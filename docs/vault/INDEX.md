---
vault: index
---

# The vault

Hand-written explanations of this codebase, meant to be read **instead of** the
source. The tree is ~90k lines; this directory is ~3k. If you are about to open a
`.rs` or `.tsx` file to find out what something does, read the vault file that
covers it first.

`.vault-lock.json` records a sha256 of every covered source file.
`scripts/vault-check.mjs` compares those hashes against the tree and fails CI when
they drift, so a vault file cannot quietly become a lie the way `docs/MODULES.md`
did. Details in `docs/CONTRIBUTING.md` § Updating the vault.

## Routing table

*(populated as each area lands — see `docs/vault/*.md` for what exists now)*

## What the vault is not

- **Not design docs.** Why the system is shaped this way lives in
  `docs/ARCHITECTURE.md`, `docs/PROTOCOLS.md`, `docs/DECISIONS.md`. The vault
  covers what the code *does*, right now, at these paths.
- **Not exhaustive.** Each file ends with a "read the source when" section naming
  the questions it deliberately does not answer. Trust that list.
- **Not a substitute for `git log`.** History lookups still go through
  `PROGRESS.txt` and `git log --all --grep`, per CLAUDE.md.
