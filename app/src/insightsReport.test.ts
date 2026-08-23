// Unit tests for the pasteable Analytics report.
// Run: node --experimental-strip-types --test src/insightsReport.test.ts  (node >= 22.6)
// (Excluded from the app tsconfig -- this is a node test, not browser code.)
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  buildAnalyticsReport,
  stamp,
  textTable,
  type AnReport,
} from "./insightsReport.ts";

const STRINGS = { intro: "Tell me what happened.", name: "prod-1", rangeLabel: "24h" };

// A window whose numbers are all recognisable on sight, so a wrong column or a
// swapped field is obvious in the assertion rather than plausible.
function fixture(over: Partial<AnReport> = {}): AnReport {
  const until = 1_770_000_000;
  const since = until - 24 * 3600;
  return {
    bucketed: true,
    since,
    until,
    bucket_s: 720,
    period_s: 3600,
    totals: {
      samples: 8614,
      first_ts: since + 60,
      last_ts: until,
      cpu_avg: 23.4,
      cpu_max: 98,
      busy_pct: 4.2,
      mem_pct_avg: 61.2,
      mem_pct_max: 74.5,
      mem_used_max: 6_227_000_000,
      mem_total: 8_371_000_000,
      swap_max: 134_217_728,
      load_avg: 1.42,
      load_max: 7.9,
      rx_avg_bps: 104_857,
      tx_avg_bps: 21_504,
      rx_bytes: 9_059_000_000,
      tx_bytes: 1_857_000_000,
    },
    series: [
      { t: since, n: 144, cpu: 12.1, cpu_max: 30, mem_pct: 58, load: 0.9, rx_bps: 40960, tx_bps: 8192 },
      { t: since + 720, n: 144, cpu: 91.5, cpu_max: 98, mem_pct: 74.5, load: 7.9, rx_bps: 1024, tx_bps: 512 },
    ],
    by_period: [
      { t: until - 3600, n: 720, cpu_avg: 30.5, cpu_max: 98, mem_pct_avg: 70, load_avg: 2.1 },
    ],
    by_disk: [
      { mount: "/", n: 8614, used_avg: 150e9, used_last: 152e9, total: 214e9, pct_last: 71.2, growth_bytes: 3_435_973_836 },
      { mount: "/var", n: 8614, used_avg: 10e9, used_last: 10e9, total: 50e9, pct_last: 20, growth_bytes: -1_073_741_824 },
    ],
    by_container: [
      { name: "web", n: 8614, cpu_avg: 10, cpu_max: 55, mem_avg: 2_147_483_648, mem_max: 3e9, uptime_pct: 100 },
      { name: "worker", n: 8614, cpu_avg: 3.5, cpu_max: 12, mem_avg: 5e8, mem_max: 9e8, uptime_pct: 62.5 },
    ],
    ...over,
  };
}

test("stamp renders local time in a fixed, language-independent shape", () => {
  const s = stamp(1_770_000_000);
  assert.match(s, /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  // Same instant, same string — no drift between calls.
  assert.equal(s, stamp(1_770_000_000));
});

test("textTable left-aligns the first column and right-aligns the rest", () => {
  const out = textTable(["name", "n"], [["a", "1"], ["bbbb", "22"]]);
  const lines = out.split("\n");
  assert.equal(lines[0], "name   n");
  assert.equal(lines[1], "a      1");
  assert.equal(lines[2], "bbbb  22");
  // No trailing whitespace on any row — it survives a paste into chat cleanly.
  for (const l of lines) assert.equal(l, l.trimEnd());
});

test("textTable widens a column to fit its widest cell, header included", () => {
  const out = textTable(["mount", "used"], [["/var/lib/docker", "9 GB"]]);
  const [head, row] = out.split("\n");
  assert.ok(head.startsWith("mount          "), head);
  assert.ok(row.startsWith("/var/lib/docker"), row);
  assert.equal(head.length, row.length);
});

test("the report leads with the ask, then the header block", () => {
  const out = buildAnalyticsReport(fixture(), STRINGS);
  const lines = out.split("\n");
  assert.equal(lines[0], "Tell me what happened.");
  assert.equal(lines[1], "");
  assert.equal(lines[2], "# ymux server report — prod-1");
  // The timezone caveat is load-bearing: without it every spike gets misdated.
  assert.match(out, /LOCAL time of the machine that copied this/);
  // And the privacy claim has to be explicit, not implied.
  assert.match(out, /no command output, no file contents, no credentials/);
});

test("totals carry both the average and the peak for every metric", () => {
  const out = buildAnalyticsReport(fixture(), STRINGS);
  assert.match(out, /CPU\s+23\.4%\s+98\.0%\s+at or over 80% in 4\.2% of samples/);
  assert.match(out, /Load\s+1\.42\s+7\.90/);
  assert.match(out, /Swap\s+-\s+128 MB/);
  // Byte totals are labelled as estimates, because they are: the sampler
  // stores a rate, not a counter.
  assert.match(out, /estimated from the mean rate/);
});

test("every series bucket is present, with its own timestamp", () => {
  const rep = fixture();
  const out = buildAnalyticsReport(rep, STRINGS);
  const body = out.slice(out.indexOf("## Series"));
  for (const p of rep.series) assert.ok(body.includes(stamp(p.t)), stamp(p.t));
  assert.match(out, /one row per 12m/);
  // The spike bucket keeps its shape: 91.5 avg under a 98 peak.
  assert.match(out, /91\.5\s+98\.0\s+74\.5\s+7\.90/);
});

test("disk growth is signed, so shrinkage cannot read as growth", () => {
  const out = buildAnalyticsReport(fixture(), STRINGS);
  assert.match(out, /\/\s+71\.2%\s+\+3\.2 GB/);
  assert.match(out, /\/var\s+20\.0%\s+-1\.0 GB/);
});

test("container uptime survives as a percentage of the window", () => {
  const out = buildAnalyticsReport(fixture(), STRINGS);
  assert.match(out, /worker\s+3\.5\s+12\.0\s+477 MB\s+62\.5/);
});

test("an absent Docker says so instead of dropping the section", () => {
  const out = buildAnalyticsReport(fixture({ by_container: [] }), STRINGS);
  assert.match(out, /## By container\nNo container samples in this range/);
});

test("a day-bucketed report labels its rollup as days, not hours", () => {
  const out = buildAnalyticsReport(fixture({ period_s: 86400 }), STRINGS);
  assert.match(out, /## By day \(newest first\)/);
  assert.ok(!out.includes("## By hour"));
});

test("an empty window still produces a valid report rather than throwing", () => {
  const empty = fixture({
    series: [],
    by_period: [],
    by_disk: [],
    by_container: [],
    totals: { ...fixture().totals, samples: 0, first_ts: 0, last_ts: 0 },
  });
  const out = buildAnalyticsReport(empty, STRINGS);
  assert.ok(out.includes("Samples: 0"));
  assert.ok(!out.includes("## Series"));
  assert.ok(!out.includes("NaN"), out);
});

test("no NaN or undefined reaches the clipboard for a populated window", () => {
  const out = buildAnalyticsReport(fixture(), STRINGS);
  assert.ok(!out.includes("NaN"));
  assert.ok(!out.includes("undefined"));
  assert.ok(out.endsWith("\n"));
});
