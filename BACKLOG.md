# Backlog

Out-of-scope feature ideas, intentional mock/stub code, deferred refactors. The `plan-to-backlog.sh` hook auto-appends bullets from approved plans' `## Out of scope` sections.

Format:

```
- [ ] <YYYY-MM-DD> | from-plan:<plan-name> | <one-line idea>
- [ ] <YYYY-MM-DD> | manual                 | <one-line idea>
```

## Open

- [ ] 2026-08-03 | manual | **P2** — `typescript` is not a project dependency; there is no reproducible type-check

  Found during the Phase 2 rebase verification. `app/node_modules` was
  completely empty (0 entries) in the main checkout, so no frontend
  verification had been possible at all until `npm ci` was run. Worse:
  even after a clean install, `typescript` is absent from
  `app/package.json` — there is no `tsc` binary and no `typecheck`
  script, despite `app/tsconfig.json` setting `"strict": true` and
  `"noEmit": true`.

  The Phase 2 type-checks passed, but only against a compiler installed
  outside the lockfile — that result is not reproducible on another
  machine or in CI.

  `vite build` is not a substitute: esbuild strips types without
  checking them, so type errors ship silently today.

  Fix: `npm i -D typescript@<pin>` and add `"typecheck": "tsc --noEmit"`
  to `app/package.json` scripts. Left undone deliberately — it changes
  package.json + package-lock.json, which is out of scope for a
  worktree-cleanup pass.

- [ ] 2026-08-03 | manual | Command palette polish — cherry-pick `5413ef9` from `design-pass-01` when the palette outgrows ~23 commands

  Parked, not dropped. `design-pass-01` is 5 commits; 4 already landed on `main`
  under different SHAs (`7fbc7fe` docs/SVG, `b3c2965` logical properties,
  `cfd50e8` welcome screen, `56bd57d` --wmx-* tokens). `5413ef9` is the only
  real delta and it is pure UX polish, deferred on purpose:

  - fzy-style fuzzy scorer + `<mark>` highlighting + score ranking (replaces
    the `includes()` substring match at `app/src/CommandPalette.tsx:38`)
  - category grouping with sticky headers, derived from the dotted command-id
    prefix (`pane.*`, `ssh.*`) — no churn on the command definitions
  - per-category icons + right-aligned keybinding hints
  - Recent section (localStorage, last 5) when the query is empty
  - footer with nav/run hints + live result count
  - i18n: `cmd.cat.*` + palette hints/count across he/en/ar/ru

  Why deferred: the palette holds ~23 commands. Substring match is adequate at
  that size; a fuzzy scorer earns its keep at 100+. The one genuine bug the
  commit fixed (references to `--w-text-primary` / `--w-text-secondary`, which
  were never defined) is already gone from `main` — the `--wmx-*` token system
  superseded it, 0 references remain in `App.css`.

  Trigger to revisit: the command count roughly quadruples, or discoverability
  complaints come in. The categories + keybinding hints (~40 of the 246 lines)
  are the parts worth having early — cheap to rewrite fresh on `main` if only
  those are wanted.

  Cost when picked up: `CommandPalette.tsx` and the 4 i18n files apply cleanly
  (`main` has not touched them since the fork). The 109 lines in `App.css` will
  conflict — `main` grew that file by ~2355 lines, including the whole new
  design-token system. Resolve by hand.

  Keep branch `design-pass-01` alive as the archive (like
  `browser-dev-mode-tickets`). Do not merge it wholesale — 4/5 of it is already
  in `main` and a full rebase would replay landed work.

## Done
