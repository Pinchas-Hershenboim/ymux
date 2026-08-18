// Client-side "is this font actually usable?" probe.
//
// Why not `document.fonts.check()`: in Chromium — and therefore in the
// WebView2 ymux runs on — `check()` returns true for essentially any
// family name that isn't in the FontFaceSet, because it validates the
// shorthand syntax rather than resolving the family. It reports a font we
// do not have as present, which is the exact failure this whole area is
// about. Measuring is the only thing that actually answers the question.
//
// The measurement: render a probe string in `"<family>", <generic>` and in
// `<generic>` alone. If the family resolves, the browser uses it and the two
// widths differ. If it does not, the fallback kicks in and the widths are
// identical. Repeated against three generics because a font can happen to be
// metrically identical to one of them.
//
// This complements the Rust-side registry enumeration rather than replacing
// it: the registry is authoritative for "what is installed", but it cannot
// see a family that arrived via a web-font stylesheet, and it stores face
// names that do not always spell the CSS family. Here we ask the renderer
// that will actually draw the terminal.

const PROBE_TEXT = "mmmmmmmmmmlliWWWWWWWWWW0O&1@%";
const BASELINES = ["monospace", "serif", "sans-serif"] as const;
const PROBE_SIZE = 72; // large enough that a 1px family difference shows up

/** CSS generics always resolve to something — never report them missing. */
const GENERICS = new Set([
  "monospace",
  "serif",
  "sans-serif",
  "system-ui",
  "ui-monospace",
  "ui-serif",
  "ui-sans-serif",
  "cursive",
  "fantasy",
  "math",
  "emoji",
  "fangsong",
]);

// One canvas for the life of the page. `undefined` = not yet attempted,
// `null` = 2d context unavailable (probing is then impossible).
let cachedCtx: CanvasRenderingContext2D | null | undefined;

function probeContext(): CanvasRenderingContext2D | null {
  if (cachedCtx !== undefined) return cachedCtx;
  cachedCtx =
    typeof document === "undefined"
      ? null
      : document.createElement("canvas").getContext("2d");
  return cachedCtx;
}

/**
 * A family name is only safe to interpolate into the `font` shorthand if it
 * cannot terminate the quoted string or the declaration. Anything with a
 * quote, backslash or semicolon is rejected outright — those cannot appear
 * in a real family name, and letting them through would silently corrupt the
 * measurement rather than answering the question.
 */
function quotable(family: string): string | null {
  const trimmed = family.trim();
  if (!trimmed || trimmed.length > 128) return null;
  if (/["'\\;{}]/.test(trimmed)) return null;
  return trimmed;
}

function widthOf(ctx: CanvasRenderingContext2D, fontSpec: string): number {
  ctx.font = `${PROBE_SIZE}px ${fontSpec}`;
  return ctx.measureText(PROBE_TEXT).width;
}

/**
 * True when the renderer can actually draw text in `family`.
 *
 * Returns true for CSS generics and — deliberately — for anything we cannot
 * measure (no canvas). An unverifiable font is reported available: claiming
 * a font is missing when we simply could not check is a worse lie than
 * staying quiet.
 */
export function isFontAvailable(family: string): boolean {
  const name = quotable(family);
  if (!name) return false;
  if (GENERICS.has(name.toLowerCase())) return true;

  const ctx = probeContext();
  if (!ctx) return true;

  for (const generic of BASELINES) {
    const fallbackWidth = widthOf(ctx, generic);
    const withFamily = widthOf(ctx, `"${name}", ${generic}`);
    // Any difference means the family won the cascade, i.e. it resolved.
    if (Math.abs(withFamily - fallbackWidth) > 0.01) return true;
  }
  return false;
}

/**
 * Same question, but first give a not-yet-loaded face a chance to arrive —
 * a family delivered by the web-font stylesheet is not "pending" until
 * something requests it, so `document.fonts.ready` alone never resolves it.
 * Used where the answer is shown to the user; the sync form is fine for
 * quick filtering.
 */
export async function isFontAvailableAsync(family: string): Promise<boolean> {
  const name = quotable(family);
  if (!name) return false;
  if (GENERICS.has(name.toLowerCase())) return true;

  const fonts = typeof document !== "undefined" ? document.fonts : undefined;
  if (fonts && typeof fonts.load === "function") {
    try {
      await fonts.load(`${PROBE_SIZE}px "${name}"`);
    } catch {
      // A malformed spec here just means "nothing to preload"; the
      // measurement below is still the real answer.
    }
  }
  return isFontAvailable(name);
}
