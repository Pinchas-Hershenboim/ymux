// Phase 80 unit tests for sessionRestore.
// Run: node --experimental-strip-types --test src/sessionRestore.test.ts   (node >= 22.6)
// (Excluded from the app tsconfig -- this is a node test, not browser code.)
import { test, beforeEach } from "node:test";
import assert from "node:assert/strict";

// Minimal localStorage stub, installed BEFORE the module under test runs any
// of its functions (the module only touches storage inside calls, so a plain
// global assignment here is enough).
let store: Record<string, string> = {};
let failWrites = false;
(globalThis as unknown as { localStorage: unknown }).localStorage = {
  getItem: (k: string) => (k in store ? store[k] : null),
  setItem: (k: string, v: string) => {
    if (failWrites) throw new Error("QuotaExceededError");
    store[k] = v;
  },
  removeItem: (k: string) => {
    delete store[k];
  },
};

const {
  rememberPaneSession,
  forgetPaneSession,
  getPaneSession,
  prunePaneSessions,
} = await import("./sessionRestore.ts");

const KEY = "winmux.paneSessions.v1";
const DAY = 24 * 60 * 60 * 1000;

beforeEach(() => {
  store = {};
  failWrites = false;
});

// --- round trip -----------------------------------------------------------

test("remember then get returns the stored session name", () => {
  rememberPaneSession("p1", "winmux-alpha");
  assert.equal(getPaneSession("p1"), "winmux-alpha");
});

test("get returns null for an unknown pane", () => {
  assert.equal(getPaneSession("nope"), null);
});

test("remember overwrites the previous name for the same pane", () => {
  rememberPaneSession("p1", "old");
  rememberPaneSession("p1", "new");
  assert.equal(getPaneSession("p1"), "new");
});

test("remember ignores empty pane id or session name", () => {
  rememberPaneSession("", "winmux-x");
  rememberPaneSession("p1", "");
  assert.equal(store[KEY], undefined);
});

test("forget drops only the named pane", () => {
  rememberPaneSession("p1", "a");
  rememberPaneSession("p2", "b");
  forgetPaneSession("p1");
  assert.equal(getPaneSession("p1"), null);
  assert.equal(getPaneSession("p2"), "b");
});

// --- TTL ------------------------------------------------------------------

test("entries older than 14 days are dropped on read", () => {
  store[KEY] = JSON.stringify({
    fresh: { tmux: "a", ts: Date.now() - 13 * DAY },
    stale: { tmux: "b", ts: Date.now() - 15 * DAY },
  });
  assert.equal(getPaneSession("fresh"), "a");
  assert.equal(getPaneSession("stale"), null);
});

// --- malformed storage ----------------------------------------------------

test("corrupt JSON reads as empty instead of throwing", () => {
  store[KEY] = "{not json";
  assert.equal(getPaneSession("p1"), null);
});

test("non-object payloads read as empty", () => {
  for (const raw of ["null", '"a string"', "42", "[1,2,3]"]) {
    store[KEY] = raw;
    assert.equal(getPaneSession("p1"), null, `payload ${raw}`);
  }
});

test("entries with the wrong field types are skipped", () => {
  store[KEY] = JSON.stringify({
    a: { tmux: 42, ts: Date.now() },
    b: { tmux: "ok", ts: "not a number" },
    c: null,
    d: { tmux: "", ts: Date.now() },
    e: { tmux: "good", ts: Date.now() },
  });
  assert.equal(getPaneSession("a"), null);
  assert.equal(getPaneSession("b"), null);
  assert.equal(getPaneSession("c"), null);
  assert.equal(getPaneSession("d"), null);
  assert.equal(getPaneSession("e"), "good");
});

test("a write that throws (quota / private mode) does not propagate", () => {
  failWrites = true;
  assert.doesNotThrow(() => rememberPaneSession("p1", "a"));
  assert.equal(getPaneSession("p1"), null);
});

// --- prune ----------------------------------------------------------------

test("prune drops hints for panes that no longer exist", () => {
  rememberPaneSession("live", "a");
  rememberPaneSession("dead", "b");
  prunePaneSessions(new Set(["live"]));
  assert.equal(getPaneSession("live"), "a");
  assert.equal(getPaneSession("dead"), null);
});

test("prune with nothing to drop leaves storage untouched", () => {
  rememberPaneSession("live", "a");
  const before = store[KEY];
  prunePaneSessions(new Set(["live", "other"]));
  assert.equal(store[KEY], before);
});

test("prune with an empty live set clears every hint", () => {
  rememberPaneSession("p1", "a");
  rememberPaneSession("p2", "b");
  prunePaneSessions(new Set());
  assert.equal(getPaneSession("p1"), null);
  assert.equal(getPaneSession("p2"), null);
});

// --- allPaneSessions (diagnostics) ---------------------------------------

test("allPaneSessions returns every remembered mapping", async () => {
  const { allPaneSessions } = await import("./sessionRestore.ts");
  rememberPaneSession("p1", "a");
  rememberPaneSession("p2", "b");
  assert.deepEqual(allPaneSessions(), { p1: "a", p2: "b" });
});

test("allPaneSessions is empty when nothing is remembered", async () => {
  const { allPaneSessions } = await import("./sessionRestore.ts");
  assert.deepEqual(allPaneSessions(), {});
});
