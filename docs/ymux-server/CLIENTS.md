# ymux-server client SDKs

Generated + hand-written clients for the `ymux-server` API. Types come from the
server contract (`sdk-gen/`); the clients are thin and idiomatic. Both track the
server version.

- **REST** is described by OpenAPI (`/api/openapi.json`, generated from the huma
  handlers). Surface: `/healthz`, `/api/version`, `/api/v2/files/*`,
  `/api/v2/logs/*`. (The Insights metrics API is desktop-internal and not part
  of the SDK — see [PHASE-77-DESIGN §6](../PHASE-77-DESIGN.md).)
- **WebSocket frames** are described by AsyncAPI (`/api/asyncapi.json`) +
  `frames.schema.json`. One flat JSON object per frame, discriminated by `type`,
  `frame_version` negotiated in the `hello` frame.

## TypeScript (`sdk/typescript`, `@ymux/sdk`)

```ts
import { YmuxClient, WorkspaceSocket } from "@ymux/sdk";

const client = new YmuxClient({ baseUrl: "http://127.0.0.1:7879", token });
await client.version();                       // capability negotiation
await client.uploadFile("notes/a.txt", bytes);
const { bytes, truncated } = await client.readFile("notes/a.txt");
await client.listLogClients();

// Stream a workspace session (browser WebSocket, or `ws` in node):
new WorkspaceSocket({
  baseUrl: "http://127.0.0.1:7879", token,
  workspaceId: "ws_default", sessionId,
  makeSocket: (u) => new WebSocket(u),
  onFrame: (f) => { if (f.type === "hook_request") { /* … */ } },
});
```

`WorkspaceSocket` also exposes `sendUserInput`, `sendHookDecision(reqId, "allow"|"deny")`,
`interrupt`, `unsubscribe`. Frame types are the generated `YmuxFrame` union —
narrow on `f.type`.

**Pairing + workspace (the mobile flow):**

```ts
// 1. redeem the one-shot from the desktop's QR → durable device credential.
const cred = await new YmuxClient({ baseUrl }).pairing.redeem(oneShotToken);
// cred: { device_id, long_term_token, default_workspace_id }

// 2. use the device token for the /api/v2 surface.
const phone = new YmuxClient({ baseUrl, token: cred.long_term_token });
const spaces = await phone.workspaces.list();                       // Workspace[]
const s = await phone.workspaces.sessions(cred.default_workspace_id).create({ kind: "claude_chat" });
const detail = await phone.workspaces.getSession(cred.default_workspace_id, s.session_id);
// then stream it with WorkspaceSocket (sessionId = s.session_id).
```

## Kotlin (`sdk/kotlin`, `dev.ymux.sdk`)

```kotlin
val client = YmuxClient("http://127.0.0.1:7879", token)
client.version()
client.uploadFile("notes/a.txt", bytes)
val (bytes, truncated) = client.readFile("notes/a.txt")

WorkspaceSocket.subscribe(
    baseUrl = "http://127.0.0.1:7879", token = token,
    workspaceId = "ws_default", sessionId = sessionId,
    handler = object : FrameHandler {
        override fun onFrame(frame: YmuxFrame) {
            when (frame) {
                is HookRequestFrame -> { /* … */ }
                else -> {}
            }
        }
    },
)
```

Frames deserialize into the sealed `YmuxFrame` via `YmuxJson.instance`
(`classDiscriminator = "type"`, unknown keys ignored for forward-compat).

**Pairing + workspace (the mobile flow):**

```kotlin
// 1. redeem the one-shot from the desktop's QR.
val cred = YmuxClient(baseUrl).pairing.redeem(oneShotToken)
// cred: PairingRedeemResponse(deviceId, longTermToken, defaultWorkspaceId)

// 2. use the device token for the /api/v2 surface.
val phone = YmuxClient(baseUrl, token = cred.longTermToken)
val spaces: List<Workspace> = phone.workspaces.list()
val s: SessionCreated = phone.workspaces.sessions(cred.defaultWorkspaceId)
    .create(CreateSessionRequest(kind = "claude_chat"))
val detail: Session = phone.workspaces.getSession(cred.defaultWorkspaceId, s.sessionId)
// then stream it with WorkspaceSocket.subscribe(sessionId = s.sessionId, …).
```

The `long_term_token` from `redeem` authenticates the whole `/api/v2/*` surface
(the server accepts the shared desktop token **or** a paired device token).

## Regenerating

Types are generated — do not edit `*.gen.ts` or `Frames.kt`/`Models.kt` by hand.
After a server contract change: `cd sdk-gen && npm run gen`. CI runs
`node sdk-gen/ci-check.mjs` to fail on drift. See [`sdk-gen/README.md`](../../sdk-gen/README.md).
