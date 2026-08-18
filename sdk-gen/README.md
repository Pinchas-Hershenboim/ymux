# sdk-gen — ymux-server client SDK generation

Generates the **TypeScript** and **Kotlin** client SDKs for `ymux-server` from
the server's own contract, so the SDKs can never silently drift from the API.

## Sources of truth

| Spec | Origin | Emitted by |
|---|---|---|
| `specs/openapi.json` | **generated** from the huma handlers | `ymux-server openapi` |
| `specs/asyncapi.json` | hand-authored WS contract | `internal/api/asyncapi.json` |
| `specs/frames.schema.json` | canonical WS frame JSON-Schema | `internal/api/frames.schema.json` |

The server also serves all three live at `/api/{openapi,asyncapi,frames.schema}.json`.

## Generators (no JVM required)

- **TypeScript** — `openapi-typescript` (REST types) + `json-schema-to-typescript`
  (frame union). Pinned in `package.json`.
- **Kotlin** — a small deterministic emitter (`gen-kotlin.mjs`) that produces
  idiomatic `kotlinx.serialization` types: a sealed `YmuxFrame` with
  `@SerialName` subclasses + `@Serializable` DTOs. (openapi-generator would give
  a richer client but needs a JVM; quicktype mishandles our discriminated
  unions — so we emit directly. Yossi's S4 brief allows "openapi-generator or
  custom".)

## Usage

```bash
cd sdk-gen
npm ci                 # pinned generators
npm run gen            # specs → sdk/typescript + sdk/kotlin
node ci-check.mjs      # drift-guard: regen + fail if committed output differs
```

`npm run gen` runs three steps: `specs` (collect/emit), `gen:ts`, `gen:kotlin`.
Only the `*.gen.ts` + generated Kotlin files and `specs/` are guarded; the
hand-written clients (`client.ts`, `ws.ts`, `YmuxClient.kt`, …) are not
regenerated.

## Contract tests

- **TypeScript** (`sdk/typescript/test/contract.mjs`) — builds a real
  `ymux-server`, drives the whole REST surface + a WS `hello` through the SDK.
  Runs here (needs Go + Node).
- **Kotlin** (`sdk/kotlin/.../ContractTest.kt`) — serialization round-trip of
  real wire payloads through the generated types. Runs in CI (needs a JDK).

## CI

Wire `node sdk-gen/ci-check.mjs` into CI to block PRs that change a handler or a
frame without regenerating the SDKs. Version-lock: the SDK `version` fields are
stamped to the server version on every `gen`.
