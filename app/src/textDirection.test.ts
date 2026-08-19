// v0.4.4 (RTL Approach C) unit tests for detectDirection.
// v0.4.4-beta.3 (Approach C+) tests for classifyRow + detectRowDirections.
// Run: node --experimental-strip-types --test src/textDirection.test.ts
// (Excluded from the app tsconfig -- it is a node test, not browser code.)
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  detectDirection,
  detectRowDirections,
  classifyRow,
  nextTuiOwnsBidi,
  strongCounts,
  RTL_DOMINANCE,
} from "./textDirection.ts";

const LTR = "ltr";
const RTL = "rtl";

const cases: Array<[string, "ltr" | "rtl", string]> = [
  // Yossi's core five
  ["1. Hello world", LTR, "pure Latin list item"],
  ["1. שלום עולם", RTL, "pure Hebrew list item"],
  ["1. שלום world", RTL, "mixed -> RTL"],
  ["/opt/wa/.shared.env", LTR, "pure ASCII path"],
  ["שרת רץ על port 4200", RTL, "mixed Hebrew + latin/digits -> RTL"],
  // Edge cases from the brief
  ["", LTR, "empty -> LTR default"],
  ["12345", LTR, "digits only -> LTR"],
  ["-> <- up down", LTR, "arrows/symbols only -> LTR"],
  // More coverage
  ["   ", LTR, "whitespace only -> LTR"],
  ["!@#$%^&*()", LTR, "punctuation only -> LTR"],
  ["שלום", RTL, "single Hebrew word"],
  ["a", LTR, "single Latin char"],
  ["ש", RTL, "single Hebrew char"],
  ["مرحبا بالعالم", RTL, "pure Arabic"],
  ["run مرحبا now", RTL, "mixed Arabic + Latin -> RTL"],
  ["4200", LTR, "port number"],
  ["$ ls -la /home", LTR, "shell prompt + command"],
  ["הפורט 4200 פתוח", RTL, "Hebrew wrapping a number"],
  ["ERROR: קובץ לא נמצא", RTL, "Latin word then Hebrew -> RTL"],
  ["git commit -m 'תיקון'", RTL, "Latin command with Hebrew arg -> RTL"],
  ["100% done", LTR, "digits + symbol + Latin -> LTR"],
  ["שלום\tworld", RTL, "tab-separated mixed -> RTL"],
  ["cafe", LTR, "Latin -> LTR"],
];

for (const [input, expected, label] of cases) {
  test(`detectDirection(${JSON.stringify(input)}) -> ${expected} -- ${label}`, () => {
    assert.equal(detectDirection(input), expected);
  });
}

// -- RTL_DOMINANCE (2026-08-19): a TUI status bar must not flip -------------
//
// The row that motivated the rule. Claude Code paints its footer as ONE
// terminal row: an update banner, the cwd, the model, and a context gauge.
// Under the old "any Hebrew wins" rule the three letters of `צבר` turned the
// whole row RTL and UAX #9 L2 mirrored every Latin run in it.
const CLAUDE_STATUS_ROW =
  "Update available! Run winget upgrade Anthropic.ClaudeCode" +
  "   ~ צבר·full · claude-fable-5[1m]  0k/1M 0% [░░░░░░░░]";

test("dominance: Claude's status row stays LTR (layout is positional)", () => {
  const { rtl, ltr } = strongCounts(CLAUDE_STATUS_ROW);
  assert.equal(rtl, 3); // צבר
  assert.ok(ltr > rtl * RTL_DOMINANCE, `expected ${ltr} > ${rtl * RTL_DOMINANCE}`);
  assert.equal(detectDirection(CLAUDE_STATUS_ROW), LTR);
});

test("dominance: the same row's Hebrew alone is still RTL", () => {
  // Proof the rule is about the ROW, not about צבר losing its direction.
  assert.equal(detectDirection("צבר"), RTL);
});

test("dominance: a Hebrew note on a long path still wins", () => {
  // 4 Hebrew vs 14 Latin -- the near miss the factor is calibrated against.
  assert.equal(detectDirection("2. /opt/wa/.shared.env - הערה"), RTL);
});

test("dominance: neutrals count for neither side", () => {
  // A progress bar, a box border and a pile of digits must not outvote
  // Hebrew, or every framed Hebrew message would flip to LTR.
  const { rtl, ltr } = strongCounts("│ שלום │ [███░░] 12345 │");
  assert.equal(ltr, 0);
  assert.equal(rtl, 4);
  assert.equal(detectDirection("│ שלום │ [███░░] 12345 │"), RTL);
});

test("dominance: exactly at the threshold, Hebrew wins", () => {
  // 2 Hebrew, 8 Latin -> 2*4 >= 8. The tie goes to RTL on purpose.
  assert.equal(detectDirection("abcdefgh של"), RTL);
  // One more Latin char and it tips.
  assert.equal(detectDirection("abcdefghi של"), LTR);
});

test("dominance: block vote uses the whole block, not one row", () => {
  // Claude's bordered input box sitting under a long Latin help line: the
  // block holds Hebrew, but nowhere near enough of it to mirror the border.
  const rows = [
    "┌" + "─".repeat(60) + "┐",
    "│ Using claude-fable-5 (from .claude/settings.json) - /model to change │",
    "│ של                                                                 │",
    "└" + "─".repeat(60) + "┘",
  ];
  assert.deepEqual(detectRowDirections(rows), [LTR, LTR, LTR, LTR]);
});

test("dominance: a Hebrew box still flips entire", () => {
  const rows = [
    "┌──────┐",
    "│ שלום │",
    "└──────┘",
  ];
  assert.deepEqual(detectRowDirections(rows), [RTL, RTL, RTL]);
});


// -- Approach C+ (block-aware) tests -----------------------------------------

// classifyRow sanity checks
test("classifyRow: ASCII markdown separator is border", () => {
  assert.equal(classifyRow("|----|-----|"), "border");
  assert.equal(classifyRow("|------|-------|"), "border");
  assert.equal(classifyRow("+------+------+"), "border");
});

test("classifyRow: Unicode box-drawing line is border", () => {
  assert.equal(classifyRow("┌──────┐"), "border");
  assert.equal(classifyRow("│ code │"), "border");
  assert.equal(classifyRow("└──────┘"), "border");
});

test("classifyRow: fence opener/closer", () => {
  assert.equal(classifyRow("```"), "fence");
  assert.equal(classifyRow("```ts"), "fence");
  assert.equal(classifyRow("  ```javascript"), "fence");
});

test("classifyRow: table body row with letters is content, not border", () => {
  assert.equal(classifyRow("| Name | Value |"), "content");
  assert.equal(classifyRow("| שם | ערך |"), "content");
});

test("classifyRow: pure text is content", () => {
  assert.equal(classifyRow("שלום עולם"), "content");
  assert.equal(classifyRow("hello world"), "content");
});

// The 6 mandated block-aware cases from the brief.

test("Test 1: pure Hebrew table -> all rows RTL", () => {
  const rows = [
    "| שם | ערך |",
    "|----|-----|",
    "| א  | 1   |",
  ];
  assert.deepEqual(detectRowDirections(rows), ["rtl", "rtl", "rtl"]);
});

test("Test 2: pure ASCII table -> all rows LTR", () => {
  const rows = [
    "| Name | Value |",
    "|------|-------|",
    "| a    | 1     |",
  ];
  assert.deepEqual(detectRowDirections(rows), ["ltr", "ltr", "ltr"]);
});

test("Test 3: mixed table (Hebrew in one cell) -> all rows RTL", () => {
  const rows = [
    "| Name | ערך |",
    "|------|-----|",
    "| a    | 1   |",
  ];
  assert.deepEqual(detectRowDirections(rows), ["rtl", "rtl", "rtl"]);
});

test("Test 4: Hebrew paragraph -> ASCII box -> Hebrew paragraph, all RTL (box inherits)", () => {
  const rows = [
    "שלום זה טקסט",
    "┌──────┐",
    "│ code │",
    "└──────┘",
    "שלום עולם",
  ];
  assert.deepEqual(detectRowDirections(rows), ["rtl", "rtl", "rtl", "rtl", "rtl"]);
});

test("Test 5: pure ASCII code fence -> all rows LTR", () => {
  const rows = [
    "```",
    "const x = 5;",
    "```",
  ];
  assert.deepEqual(detectRowDirections(rows), ["ltr", "ltr", "ltr"]);
});

test("Test 6: mixed code fence with Hebrew comment -> all rows RTL", () => {
  const rows = [
    "```",
    "// הערה בעברית",
    "const x = 5;",
    "```",
  ];
  assert.deepEqual(detectRowDirections(rows), ["rtl", "rtl", "rtl", "rtl"]);
});

// Extra sanity: ASCII box between two LTR paragraphs stays LTR.
test("bonus: ASCII box between LTR paragraphs -> all LTR", () => {
  const rows = [
    "some english prose",
    "┌──────┐",
    "│ code │",
    "└──────┘",
    "more english prose",
  ];
  assert.deepEqual(detectRowDirections(rows), ["ltr", "ltr", "ltr", "ltr", "ltr"]);
});

// Extra sanity: empty input.
test("bonus: empty rows array", () => {
  assert.deepEqual(detectRowDirections([]), []);
});

// Extra sanity: single Hebrew standalone line still works.
test("bonus: single standalone Hebrew line -> RTL", () => {
  assert.deepEqual(detectRowDirections(["שלום עולם"]), ["rtl"]);
});

// -- nextTuiOwnsBidi (Claude visual-order RTL) tests --------------------------
// The title-driven state machine: ON on a "claude" title, OFF on an empty
// title, hold on anything else (shell paths, Claude's auto topic titles).

test("tuiOwnsBidi: 'claude' startup title turns on", () => {
  assert.equal(nextTuiOwnsBidi(false, "claude"), true);
});

test("tuiOwnsBidi: 'claude · resume' variant turns on", () => {
  assert.equal(nextTuiOwnsBidi(false, "claude · resume"), true);
});

test("tuiOwnsBidi: case-insensitive match", () => {
  assert.equal(nextTuiOwnsBidi(false, "Claude Code"), true);
});

test("tuiOwnsBidi: empty title (claude exit cleanup) turns off", () => {
  assert.equal(nextTuiOwnsBidi(true, ""), false);
});

test("tuiOwnsBidi: whitespace-only title also turns off", () => {
  assert.equal(nextTuiOwnsBidi(true, "   "), false);
});

test("tuiOwnsBidi: auto topic title mid-session holds ON", () => {
  assert.equal(nextTuiOwnsBidi(true, "fixing the RTL bug"), true);
});

test("tuiOwnsBidi: Hebrew topic title mid-session holds ON", () => {
  assert.equal(nextTuiOwnsBidi(true, "תיקון באג RTL"), true);
});

test("tuiOwnsBidi: shell path title while off stays off", () => {
  assert.equal(
    // String.raw: in a plain "" literal `\v` is a vertical tab and `\W \S \p`
    // are dropped, so this asserted against a mangled string, not a path.
    nextTuiOwnsBidi(false, String.raw`C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe`),
    false,
  );
});

test("tuiOwnsBidi: unrelated title while off stays off", () => {
  assert.equal(nextTuiOwnsBidi(false, "vim - notes.md"), false);
});

test("tuiOwnsBidi: full lifecycle start -> topic -> exit", () => {
  let s = false;
  s = nextTuiOwnsBidi(s, "claude");            // startup
  assert.equal(s, true);
  s = nextTuiOwnsBidi(s, "צ׳אט על באגים");      // auto topic rename
  assert.equal(s, true);
  s = nextTuiOwnsBidi(s, "");                  // clean exit
  assert.equal(s, false);
});
