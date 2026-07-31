// Phase 80.1 unit tests for fmPaths.
// Run: node --experimental-strip-types --test src/fmPaths.test.ts   (node >= 22.6)
// (Excluded from the app tsconfig -- this is a node test, not browser code.)
import { test, beforeEach } from "node:test";
import assert from "node:assert/strict";

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

const { loadFmPaths, saveFmPaths, pruneFmPaths } = await import("./fmPaths.ts");

const KEY = "winmux.fm.paths.v1";

beforeEach(() => {
  store = {};
  failWrites = false;
});

// --- round trip -----------------------------------------------------------

test("save then load returns both columns", () => {
  saveFmPaths("ws1", { local: "C:\\work", remote: "/srv/app" });
  assert.deepEqual(loadFmPaths("ws1"), { local: "C:\\work", remote: "/srv/app" });
});

test("load returns {} for an unknown workspace", () => {
  assert.deepEqual(loadFmPaths("nope"), {});
});

test("workspaces do not see each other's paths", () => {
  saveFmPaths("ws1", { local: "C:\\a" });
  saveFmPaths("ws2", { local: "D:\\b" });
  assert.equal(loadFmPaths("ws1").local, "C:\\a");
  assert.equal(loadFmPaths("ws2").local, "D:\\b");
});

test("save ignores an empty workspace id", () => {
  saveFmPaths("", { local: "C:\\a" });
  assert.equal(store[KEY], undefined);
});

// --- merge semantics (the clobber bug this exists to prevent) -------------

test("saving only local keeps a previously remembered remote", () => {
  saveFmPaths("ws1", { local: "C:\\a", remote: "/srv/app" });
  saveFmPaths("ws1", { local: "C:\\b" });
  assert.deepEqual(loadFmPaths("ws1"), { local: "C:\\b", remote: "/srv/app" });
});

test("an empty-string field is 'no opinion', not 'forget it'", () => {
  saveFmPaths("ws1", { local: "C:\\a", remote: "/srv/app" });
  saveFmPaths("ws1", { local: "C:\\a", remote: "" });
  assert.equal(loadFmPaths("ws1").remote, "/srv/app");
});

test("saving nothing at all is a no-op", () => {
  saveFmPaths("ws1", { local: "C:\\a" });
  const before = store[KEY];
  saveFmPaths("ws1", {});
  assert.equal(store[KEY], before);
});

test("re-saving identical paths does not rewrite storage", () => {
  saveFmPaths("ws1", { local: "C:\\a", remote: "/srv" });
  const before = store[KEY];
  failWrites = true; // any write attempt would be swallowed, so assert on value
  saveFmPaths("ws1", { local: "C:\\a", remote: "/srv" });
  assert.equal(store[KEY], before);
});

// --- shape gates ----------------------------------------------------------

test("a UNC local path is dropped on read (no unattended SMB handshake)", () => {
  store[KEY] = JSON.stringify({ ws1: { local: "\\\\attacker.tld\\share" } });
  assert.deepEqual(loadFmPaths("ws1"), {});
});

test("relative and forward-slash-rooted local paths are dropped", () => {
  store[KEY] = JSON.stringify({
    a: { local: "..\\..\\etc" },
    b: { local: "//attacker.tld/share" },
    c: { local: "work" },
  });
  assert.deepEqual(loadFmPaths("a"), {});
  assert.deepEqual(loadFmPaths("b"), {});
  assert.deepEqual(loadFmPaths("c"), {});
});

test("drive-absolute local paths survive, with either slash", () => {
  store[KEY] = JSON.stringify({
    a: { local: "C:\\work" },
    b: { local: "d:/work" },
  });
  assert.equal(loadFmPaths("a").local, "C:\\work");
  assert.equal(loadFmPaths("b").local, "d:/work");
});

test("a relative remote path is dropped, POSIX-absolute survives", () => {
  store[KEY] = JSON.stringify({
    a: { remote: "srv/app" },
    b: { remote: "/srv/app" },
  });
  assert.deepEqual(loadFmPaths("a"), {});
  assert.equal(loadFmPaths("b").remote, "/srv/app");
});

test("one bad column does not take the good one with it", () => {
  store[KEY] = JSON.stringify({ ws1: { local: "\\\\evil\\share", remote: "/srv" } });
  assert.deepEqual(loadFmPaths("ws1"), { remote: "/srv" });
});

// --- malformed storage ----------------------------------------------------

test("corrupt JSON reads as empty instead of throwing", () => {
  store[KEY] = "{not json";
  assert.deepEqual(loadFmPaths("ws1"), {});
});

test("non-object payloads and entries read as empty", () => {
  for (const raw of ["null", '"str"', "7", '{"ws1": 5}', '{"ws1": null}']) {
    store[KEY] = raw;
    assert.deepEqual(loadFmPaths("ws1"), {}, `payload ${raw}`);
  }
});

test("wrong field types are skipped", () => {
  store[KEY] = JSON.stringify({ ws1: { local: 42, remote: ["/srv"] } });
  assert.deepEqual(loadFmPaths("ws1"), {});
});

test("a write that throws (quota / private mode) does not propagate", () => {
  failWrites = true;
  assert.doesNotThrow(() => saveFmPaths("ws1", { local: "C:\\a" }));
  assert.deepEqual(loadFmPaths("ws1"), {});
});

// --- prune ----------------------------------------------------------------

test("prune drops entries for workspaces that no longer exist", () => {
  saveFmPaths("live", { local: "C:\\a" });
  saveFmPaths("deleted", { remote: "/srv/customer" });
  pruneFmPaths(new Set(["live"]));
  assert.equal(loadFmPaths("live").local, "C:\\a");
  assert.deepEqual(loadFmPaths("deleted"), {});
});

test("prune with nothing to drop leaves storage untouched", () => {
  saveFmPaths("live", { local: "C:\\a" });
  const before = store[KEY];
  pruneFmPaths(new Set(["live", "other"]));
  assert.equal(store[KEY], before);
});
