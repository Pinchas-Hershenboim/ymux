// Unit tests for the logical -> visual reorder pipeline (rtl_mode
// "bidi_reorder"). Run:
//   cd app && node --experimental-strip-types --test src/bidi.test.ts
// (Excluded from the app tsconfig -- node tests, not browser code.)
//
// The bug these were written for (2026-08-19): reorderLine() hard-coded the
// paragraph base direction to "ltr", so neutrals at the end of a Hebrew line
// resolved against an LTR paragraph and rendered at the line START.
import { test } from "node:test";
import assert from "node:assert/strict";
import { reorderRtlForDisplay } from "./bidi.ts";

// Reordering is a permutation of the input plus bidi mirroring, so the safest
// assertions are structural: where a character LANDS, not a transcribed
// string literal (which is unreadable and easy to get wrong in a bidi test).
const idx = (s: string, ch: string) => s.indexOf(ch);

test("pure-Latin text is returned untouched", () => {
  const s = "npm run build -- --no-bundle";
  assert.equal(reorderRtlForDisplay(s), s);
});

test("text with no RTL character is returned by identity", () => {
  const s = "[file] /home/runner/.env.prod (2.1 kB)";
  assert.equal(reorderRtlForDisplay(s), s);
});

// Orientation note, because it is easy to get backwards: the OUTPUT of this
// pipeline is VISUAL order, painted left-to-right by the WebGL renderer. So
// string index 0 is the LEFTMOST glyph — which, in a right-to-left sentence,
// is its END. Sentence-final punctuation therefore belongs at index 0, and
// finding it at the last index is the bug Yossi reported ("characters show at
// the beginning of the line instead of the end").
test("sentence-final '?' on a Hebrew line renders at the visual END (leftmost)", () => {
  const out = reorderRtlForDisplay("מה קורה אחי?");
  assert.equal(idx(out, "?"), 0, "'?' landed at the right edge — the start of a Hebrew sentence");
});

test("sentence-final '!!!' on a Hebrew line renders at the visual END", () => {
  const out = reorderRtlForDisplay("בא נראה איך זה עובד!!!");
  assert.equal(out[0], "!", "'!' landed at the right edge — the start of a Hebrew sentence");
});

test("a Latin-first line keeps its LTR paragraph — layout is NOT mirrored", () => {
  // Claude's status line: mostly Latin, one Hebrew word. UAX #9 P2/P3 resolves
  // the base from the first strong character (Latin), so the run order must be
  // preserved. Forcing rtl here is what mirrored the status bar.
  const src = "claude-fable-5 ~ צבר full";
  const out = reorderRtlForDisplay(src);
  assert.ok(out.startsWith("claude-fable-5"), "Latin prefix moved — paragraph was treated as RTL");
  assert.ok(out.endsWith("full"), "Latin suffix moved — paragraph was treated as RTL");
});

test("reorder is a permutation — no character is lost or duplicated", () => {
  const src = "שרת רץ על port 4200 — האם זה עובד?";
  const out = reorderRtlForDisplay(src);
  assert.equal(out.length, src.length);
  const sortKey = (s: string) => s.split("").sort().join("");
  // Mirrored pairs are substituted, not dropped; this line has none.
  assert.equal(sortKey(out), sortKey(src));
});

test("ANSI escape sequences are passed through verbatim", () => {
  const out = reorderRtlForDisplay("\x1b[31mשלום\x1b[0m");
  assert.ok(out.startsWith("\x1b[31m"), "leading SGR was altered");
  assert.ok(out.endsWith("\x1b[0m"), "trailing SGR was altered");
});

test("an ANSI-only chunk is untouched", () => {
  const s = "\x1b[2J\x1b[H\x1b[?25l";
  assert.equal(reorderRtlForDisplay(s), s);
});

test("line structure survives — newlines stay put", () => {
  const out = reorderRtlForDisplay("שלום\nעולם\n");
  assert.equal(out.split("\n").length, 3);
  assert.ok(out.endsWith("\n"));
});

test("CRLF is preserved as CRLF", () => {
  const out = reorderRtlForDisplay("שלום\r\nעולם");
  assert.ok(out.includes("\r\n"), "CRLF was split or reordered");
});

test("a Latin-first mixed line still reorders its Hebrew run", () => {
  // Claude's chrome often puts a Latin glyph first on an otherwise Hebrew
  // line; bidi-js's own auto-detection would call the paragraph LTR here,
  // which is why the direction comes from detectDirection() instead.
  const src = "> מה קורה אחי?";
  const out = reorderRtlForDisplay(src);
  assert.notEqual(out, src, "Hebrew run was left in logical order");
  assert.equal(out.length, src.length);
});
