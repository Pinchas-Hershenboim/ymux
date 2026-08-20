// Tests for the visual->logical clipboard transform.
// Run: cd app && node --experimental-strip-types --test src/copyBidi.test.ts
//
// The forward direction (logical -> visual) is reorderRtlForDisplay in bidi.ts,
// covered by bidi.test.ts. These tests are about the INVERSE: given what a
// visual-order pane put in the buffer, do we hand the clipboard something that
// reads correctly when pasted?

import test from "node:test";
import assert from "node:assert/strict";
import { visualToLogical } from "./copyBidi.ts";
import { reorderRtlForDisplay } from "./bidi.ts";

/** The shapes bidi.test.ts already cares about, plus the ones that bite. */
const ROUND_TRIP = [
  "מה קורה אחי?",
  "שרת רץ על port 4200 — האם זה עובד?",
  "(שלום) עולם",
  "claude-fable-5 · ~ צבר·full",
  "אוקי",
  "יש 42 קבצים בתיקייה",
  "> מה קורה אחי?",
];

test("un-reverses what the display reorder produced", () => {
  for (const logical of ROUND_TRIP) {
    const visual = reorderRtlForDisplay(logical);
    assert.equal(
      visualToLogical(visual),
      logical,
      `round trip failed for ${JSON.stringify(logical)}`,
    );
  }
});

test("text with no RTL is returned untouched", () => {
  // The overwhelmingly common case — every copy in an English session goes
  // through here, so identity has to be exact, not merely equivalent.
  for (const s of ["", "hello world", "PS C:\Users\me> git status", "1234"]) {
    assert.equal(visualToLogical(s), s);
  }
});

test("digits inside a Hebrew run keep their own order", () => {
  // "42" must not come back as "24". This is the thing a naive
  // reverse-the-whole-string implementation gets wrong.
  const logical = "יש 42 קבצים בתיקייה";
  assert.equal(visualToLogical(reorderRtlForDisplay(logical)), logical);
  assert.ok(visualToLogical(reorderRtlForDisplay(logical)).includes("42"));
});

test("mirrored brackets come back the right way round", () => {
  const logical = "(שלום) עולם";
  const visual = reorderRtlForDisplay(logical);
  // The forward pass really did mirror them — otherwise this test proves nothing.
  assert.notEqual(visual, logical);
  assert.equal(visualToLogical(visual), logical);
});

test("niqqud stays attached to its base letter", () => {
  // UAX #9 W1 gives a combining mark its base letter's level, so a reversal
  // moves the mark AHEAD of its base. The round trip has to put it back.
  const logical = "שָׁלוֹם";
  const visual = reorderRtlForDisplay(logical);
  assert.notEqual(visual, logical);
  assert.equal(visualToLogical(visual), logical);
});

test("an astral character is not torn in half", () => {
  // Array.from, not split(""). A surrogate pair inside a reversed run would
  // otherwise come back as two lone surrogates.
  const logical = "א🌵ב";
  const out = visualToLogical(reorderRtlForDisplay(logical));
  assert.equal(out, logical);
  assert.ok(!/[\uD800-\uDFFF]/.test(out.replace(/[\u{10000}-\u{10FFFF}]/gu, "")));
});

test("line structure survives, including CRLF", () => {
  const logical = "מה קורה אחי?\r\nשלום עולם\nאוקי";
  assert.equal(visualToLogical(reorderRtlForDisplay(logical)), logical);
});

test("each line resolves its own direction", () => {
  // A Latin-first line and a Hebrew-first line in one selection must not be
  // forced to share a paragraph direction.
  const logical = "claude-fable-5 · ~ צבר·full\nמה קורה אחי?";
  assert.equal(visualToLogical(reorderRtlForDisplay(logical)), logical);
});

test("KNOWN AMBIGUITY: adjacent digits and Latin at an RTL edge", () => {
  // Not a bug we can fix — the two logical readings below have the SAME visual
  // form, so the information is destroyed before we ever see it. Pinned so the
  // day someone "fixes" it, they find out here that there is nothing to fix.
  const a = "שלום עולם 123 abc";
  const b = "שלום עולם abc 123";
  const va = reorderRtlForDisplay(a);
  const vb = reorderRtlForDisplay(b);
  assert.equal(va, vb, "precondition: the two readings collide in visual form");

  const out = visualToLogical(va);
  assert.ok(
    out === a || out === b,
    `expected one of the two readings, got ${JSON.stringify(out)}`,
  );
});
