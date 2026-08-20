// @ts-ignore - bidi-js has no type defs
import bidiFactory from "bidi-js";

const bidi: any = bidiFactory();

const RTL_RE = /[֐-ࣿיִ-ﻼ]/;
/** Exported so the visual->logical pass can protect escapes exactly the way
 *  this file does — one definition of "what an escape looks like". */
export const ANSI_RE = /\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\x07\x1B]*(?:\x07|\x1B\\))/g;

function reorderLine(line: string): string {
  if (!RTL_RE.test(line)) return line;
  // 2026-08-19: the base direction used to be hard-coded "ltr", which put the
  // NEUTRALS at the wrong end of a Hebrew line — a trailing "?" or "!!!"
  // resolved against an LTR paragraph and rendered at the RIGHT edge, i.e. at
  // the START of the Hebrew sentence. Yossi hit this as "characters show at
  // the beginning of the line instead of the end".
  //
  // Hard-coding "rtl" instead is equally wrong, and worse: Claude's status
  // line is mostly Latin with one Hebrew word, and an RTL paragraph mirrors
  // the whole run sequence — the layout flip seen in auto_per_line mode.
  //
  // Let bidi-js apply UAX #9 P2/P3 instead (first strong character wins),
  // which is right in all three shapes we care about:
  //   "מה קורה אחי?"                    -> first strong is Hebrew -> rtl
  //   "> מה קורה אחי?"                  -> ">" is neutral, so still rtl
  //   "claude-fable-5 · ~ 🌵 צבר·full"  -> first strong is Latin  -> ltr
  //
  // KNOWN LIMIT: `line` is a run between ANSI escapes, not always a whole
  // visual line, so a heavily-coloured line can be split into fragments that
  // each resolve their own base. Strictly better than any fixed direction, but
  // the real fix is reassembling full lines before reordering — see the caret /
  // partial-repaint follow-up in docs/DECISIONS.md.
  const levels = bidi.getEmbeddingLevels(line);
  const flips = bidi.getReorderSegments(line, levels);
  const mirrors = bidi.getMirroredCharactersMap(line, levels);

  const arr = line.split("");
  if (mirrors && mirrors.size) {
    mirrors.forEach((repl: string, idx: number) => {
      arr[idx] = repl;
    });
  }
  for (const [start, end] of flips) {
    const sub = arr.slice(start, end + 1).reverse();
    for (let i = 0; i < sub.length; i++) arr[start + i] = sub[i];
  }
  return arr.join("");
}

function reorderTextSegment(seg: string): string {
  if (!RTL_RE.test(seg)) return seg;
  const parts = seg.split(/(\r\n|\r|\n)/);
  return parts
    .map((p) => (/^(?:\r\n|\r|\n)$/.test(p) ? p : reorderLine(p)))
    .join("");
}

export function reorderRtlForDisplay(chunk: string): string {
  if (!RTL_RE.test(chunk)) return chunk;
  const out: string[] = [];
  let last = 0;
  for (const m of chunk.matchAll(ANSI_RE)) {
    const idx = m.index!;
    if (idx > last) out.push(reorderTextSegment(chunk.slice(last, idx)));
    out.push(m[0]);
    last = idx + m[0].length;
  }
  if (last < chunk.length) out.push(reorderTextSegment(chunk.slice(last)));
  return out.join("");
}
