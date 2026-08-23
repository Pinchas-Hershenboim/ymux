// The wire shape of `GET /analytics` (see server/internal/insights/analytics.go)
// plus the one thing you can do with it outside the panel: flatten it into a
// plain-text report you can paste into Claude — or an email, or an incident
// ticket — and ask what happened.
//
// This module is deliberately PURE and DOM-free: no Solid, no i18n, no
// clipboard. The caller supplies the already-translated intro line. That is
// what makes `insightsReport.test.ts` runnable as a plain node test, and the
// column alignment is exactly the kind of thing that is quietly wrong forever
// if nothing asserts it.

import { fmtBytes, fmtBps, fmtSpan } from "./insightsFmt.ts";

export interface AnPoint {
  t: number;
  n: number;
  cpu: number;
  cpu_max: number;
  mem_pct: number;
  load: number;
  rx_bps: number;
  tx_bps: number;
}
export interface AnTotals {
  samples: number;
  first_ts: number;
  last_ts: number;
  cpu_avg: number;
  cpu_max: number;
  busy_pct: number;
  mem_pct_avg: number;
  mem_pct_max: number;
  mem_used_max: number;
  mem_total: number;
  swap_max: number;
  load_avg: number;
  load_max: number;
  rx_avg_bps: number;
  tx_avg_bps: number;
  rx_bytes: number;
  tx_bytes: number;
}
export interface AnPeriod {
  t: number;
  n: number;
  cpu_avg: number;
  cpu_max: number;
  mem_pct_avg: number;
  load_avg: number;
}
export interface AnDisk {
  mount: string;
  used_avg: number;
  used_last: number;
  total: number;
  pct_last: number;
  growth_bytes: number;
  n: number;
}
export interface AnContainer {
  name: string;
  cpu_avg: number;
  cpu_max: number;
  mem_avg: number;
  mem_max: number;
  uptime_pct: number;
  n: number;
}
export interface AnReport {
  bucketed?: boolean;
  /** Set by the local-workspace shim: there is no metrics store to roll up. */
  unavailable?: string;
  since: number;
  until: number;
  bucket_s: number;
  period_s: number;
  totals: AnTotals;
  series: AnPoint[];
  by_period: AnPeriod[];
  by_disk: AnDisk[];
  by_container: AnContainer[];
}

export interface ReportStrings {
  /** The ask, already translated — what the reader should DO with this. */
  intro: string;
  /** Workspace/server name, or empty. */
  name: string;
  /** Human label of the selected range, e.g. "24h". */
  rangeLabel: string;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/**
 * `YYYY-MM-DD HH:MM` in the LOCAL time of the machine rendering this — which
 * is the desktop, not the server. The report header says so out loud, because
 * a reader who assumes otherwise will mis-date every spike in it.
 *
 * Hand-rolled rather than Intl: this string is machine-read as much as
 * human-read, so it must not change shape with the UI language.
 */
export function stamp(unix: number): string {
  const d = new Date(unix * 1000);
  return (
    `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ` +
    `${pad2(d.getHours())}:${pad2(d.getMinutes())}`
  );
}

/** Fixed-width text table. Numeric columns right-align, the first column left. */
export function textTable(headers: string[], rows: string[][]): string {
  const widths = headers.map((h, i) =>
    Math.max(h.length, ...rows.map((r) => (r[i] ?? "").length)),
  );
  const line = (cells: string[]) =>
    cells
      .map((c, i) => (i === 0 ? c.padEnd(widths[i]) : c.padStart(widths[i])))
      .join("  ")
      .trimEnd();
  return [line(headers), ...rows.map(line)].join("\n");
}

const kbps = (bps: number) => (bps / 1024).toFixed(1);

// Deliberately NOT insightsFmt's fmtPct. That one drops the decimal above 10%
// so columns of live numbers stop jittering between refreshes — a display
// choice, and the right one on screen. Here the number is handed to something
// that will reason about it, and 23% instead of 23.4% is precision thrown
// away for nothing. One decimal, always, so the column still aligns.
const pct = (v: number) => `${v.toFixed(1)}%`;

/**
 * Flatten a report into pasteable plain text.
 *
 * Choices worth keeping: absolute timestamps rather than "3h ago" (the paste
 * outlives the moment it was copied); the full bucketed series, because the
 * shape over time is the whole question and 120 rows is a few KB; and an
 * explicit note about what is NOT in here, so nobody has to guess whether
 * pasting it leaks terminal contents.
 */
export function buildAnalyticsReport(rep: AnReport, s: ReportStrings): string {
  const t = rep.totals;
  const out: string[] = [];

  out.push(s.intro, "");
  out.push(`# ymux server report${s.name ? ` — ${s.name}` : ""}`);
  out.push(
    `Range: last ${s.rangeLabel} (${stamp(rep.since)} → ${stamp(rep.until)}). ` +
      `All timestamps are the LOCAL time of the machine that copied this, not the server's.`,
  );

  const observed = t.last_ts - t.first_ts;
  const span = rep.until - rep.since;
  const coverage = span > 0 && t.samples ? (observed / span) * 100 : 0;
  out.push(
    `Samples: ${t.samples} covering ${fmtSpan(observed)} — ${pct(coverage)} of the range, ` +
      `one series row per ${fmtSpan(rep.bucket_s)}.`,
  );
  out.push(
    "Source: ymux Monitor → Analytics (sampled every 5s, 7-day retention). " +
      "Machine metrics only — no command output, no file contents, no credentials.",
  );

  out.push("", "## Totals");
  out.push(
    textTable(
      ["metric", "average", "peak", "note"],
      [
        [
          "CPU",
          pct(t.cpu_avg),
          pct(t.cpu_max),
          `at or over 80% in ${pct(t.busy_pct)} of samples`,
        ],
        [
          "RAM",
          pct(t.mem_pct_avg),
          pct(t.mem_pct_max),
          `${fmtBytes(t.mem_used_max)} peak of ${fmtBytes(t.mem_total)}`,
        ],
        ["Swap", "-", fmtBytes(t.swap_max), ""],
        ["Load", t.load_avg.toFixed(2), t.load_max.toFixed(2), ""],
        [
          "Net down",
          fmtBps(t.rx_avg_bps),
          "-",
          `~${fmtBytes(t.rx_bytes)} total, estimated from the mean rate`,
        ],
        [
          "Net up",
          fmtBps(t.tx_avg_bps),
          "-",
          `~${fmtBytes(t.tx_bytes)} total, estimated from the mean rate`,
        ],
      ],
    ),
  );

  if (rep.series.length) {
    out.push("", `## Series — one row per ${fmtSpan(rep.bucket_s)}`);
    out.push(
      textTable(
        ["time", "cpu%", "cpu_peak%", "ram%", "load", "rx_KB/s", "tx_KB/s", "n"],
        rep.series.map((p) => [
          stamp(p.t),
          p.cpu.toFixed(1),
          p.cpu_max.toFixed(1),
          p.mem_pct.toFixed(1),
          p.load.toFixed(2),
          kbps(p.rx_bps),
          kbps(p.tx_bps),
          String(p.n),
        ]),
      ),
    );
  }

  if (rep.by_period.length) {
    const unit = rep.period_s >= 86400 ? "day" : "hour";
    out.push("", `## By ${unit} (newest first)`);
    out.push(
      textTable(
        ["period", "cpu_avg%", "cpu_peak%", "ram_avg%", "load_avg", "n"],
        rep.by_period.map((r) => [
          stamp(r.t),
          r.cpu_avg.toFixed(1),
          r.cpu_max.toFixed(1),
          r.mem_pct_avg.toFixed(1),
          r.load_avg.toFixed(2),
          String(r.n),
        ]),
      ),
    );
  }

  if (rep.by_disk.length) {
    out.push("", "## By disk");
    out.push(
      textTable(
        ["mount", "used%", "growth_over_range", "used", "total"],
        rep.by_disk.map((d) => [
          d.mount,
          pct(d.pct_last),
          `${d.growth_bytes > 0 ? "+" : ""}${fmtBytes(d.growth_bytes)}`,
          fmtBytes(d.used_last),
          fmtBytes(d.total),
        ]),
      ),
    );
  }

  if (rep.by_container.length) {
    out.push("", "## By container");
    out.push(
      textTable(
        ["name", "cpu_avg%", "cpu_peak%", "ram_avg", "uptime%", "n"],
        rep.by_container.map((c) => [
          c.name,
          c.cpu_avg.toFixed(1),
          c.cpu_max.toFixed(1),
          fmtBytes(c.mem_avg),
          c.uptime_pct.toFixed(1),
          String(c.n),
        ]),
      ),
    );
  } else {
    out.push("", "## By container", "No container samples in this range (Docker absent or unreachable).");
  }

  return out.join("\n") + "\n";
}
