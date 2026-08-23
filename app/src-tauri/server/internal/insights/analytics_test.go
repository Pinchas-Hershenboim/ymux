package insights

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

// seedStore writes `n` synthetic ticks ending at `end`, `step` seconds apart.
// cpu ramps 0→99 so avg/max/busy are all predictable, and disk `used` grows a
// fixed amount per tick so the growth column has a known answer.
func seedStore(t *testing.T, n int, step int64, end int64) *Store {
	t.Helper()
	st, err := OpenStore(filepath.Join(t.TempDir(), "metrics.db"))
	if err != nil {
		t.Fatalf("OpenStore: %v", err)
	}
	t.Cleanup(st.Close)
	for i := 0; i < n; i++ {
		ts := end - int64(n-1-i)*step
		snap := &Snapshot{
			TS:  ts,
			CPU: CPUInfo{Pct: float64(i % 100), Load: []float64{1.5}},
			Mem: MemInfo{Total: 1000, Used: uint64(400 + i%100), SwapUsed: 7},
			Disks: []DiskInfo{
				{Mount: "/", Used: uint64(1000 + i*10), Total: 100000},
			},
			NetRxBps: 100,
			NetTxBps: 50,
			Docker: []DockerContainer{
				{ID: "abc", Name: "web", CPUPct: 10, MemUsed: 2048, State: stateFor(i)},
			},
		}
		st.insert(snap)
	}
	return st
}

// Every 4th tick the container is down — 75% uptime, exactly.
func stateFor(i int) string {
	if i%4 == 3 {
		return "exited"
	}
	return "running"
}

func TestAnalyticsAggregatesWholeWindow(t *testing.T) {
	end := time.Now().Unix()
	// 200 ticks × 5s = the last ~16 minutes.
	st := seedStore(t, 200, 5, end)

	rep, err := st.analytics(end-3600, end, 20)
	if err != nil {
		t.Fatalf("analytics: %v", err)
	}
	if !rep.Bucketed {
		t.Fatal("report must be flagged bucketed")
	}
	if rep.Totals.Samples != 200 {
		t.Fatalf("samples: want 200 got %d", rep.Totals.Samples)
	}
	// cpu cycles 0..99 twice → max 99, avg 49.5.
	if rep.Totals.CPUMax != 99 {
		t.Fatalf("cpu_max: want 99 got %v", rep.Totals.CPUMax)
	}
	if rep.Totals.CPUAvg != 49.5 {
		t.Fatalf("cpu_avg: want 49.5 got %v", rep.Totals.CPUAvg)
	}
	// 40 of 200 samples are ≥ 80 (indices 80..99 in each of the two cycles).
	if rep.Totals.BusyPct != 20 {
		t.Fatalf("busy_pct: want 20 got %v", rep.Totals.BusyPct)
	}
	// mem_used 400..499 of 1000 → 40%..49.9%.
	if rep.Totals.MemPctMax < 49 || rep.Totals.MemPctMax > 50 {
		t.Fatalf("mem_pct_max out of range: %v", rep.Totals.MemPctMax)
	}
	if rep.Totals.MemTotal != 1000 {
		t.Fatalf("mem_total: want 1000 got %d", rep.Totals.MemTotal)
	}
	// The estimate is mean rate × observed span, not × requested span.
	observed := float64(rep.Totals.LastTS - rep.Totals.FirstTS)
	if want := 100 * observed; rep.Totals.RxBytes != want {
		t.Fatalf("rx_bytes: want %v got %v", want, rep.Totals.RxBytes)
	}
}

// The bug this endpoint exists to avoid: a long window must aggregate the WHOLE
// span, not return the oldest N raw rows. 6 hours of 5s samples is 4320 rows —
// more than /history's 2000-row cap — yet the last bucket must still land near
// the end of the window.
func TestAnalyticsLongWindowCoversTheEnd(t *testing.T) {
	end := time.Now().Unix()
	st := seedStore(t, 4320, 5, end)

	rep, err := st.analytics(end-6*3600, end, 120)
	if err != nil {
		t.Fatalf("analytics: %v", err)
	}
	if rep.Totals.Samples != 4320 {
		t.Fatalf("samples: want 4320 got %d", rep.Totals.Samples)
	}
	if len(rep.Series) == 0 {
		t.Fatal("empty series")
	}
	if got := len(rep.Series); got > 130 {
		t.Fatalf("series should be bucketed to ~120 points, got %d", got)
	}
	last := rep.Series[len(rep.Series)-1]
	if end-last.T > 2*rep.BucketS {
		t.Fatalf("last bucket %d is %ds behind the window end — window not covered",
			last.T, end-last.T)
	}
}

func TestAnalyticsRollups(t *testing.T) {
	end := time.Now().Unix()
	st := seedStore(t, 100, 5, end)

	rep, err := st.analytics(end-3600, end, 60)
	if err != nil {
		t.Fatalf("analytics: %v", err)
	}

	if len(rep.ByDisk) != 1 || rep.ByDisk[0].Mount != "/" {
		t.Fatalf("by_disk: want one '/' row, got %+v", rep.ByDisk)
	}
	d := rep.ByDisk[0]
	// used grows by 10 per tick across 100 ticks → 990.
	if d.GrowthBytes != 990 {
		t.Fatalf("growth_bytes: want 990 got %d", d.GrowthBytes)
	}
	if d.UsedLast != 1990 {
		t.Fatalf("used_last: want 1990 got %d", d.UsedLast)
	}

	if len(rep.ByCont) != 1 || rep.ByCont[0].Name != "web" {
		t.Fatalf("by_container: want one 'web' row, got %+v", rep.ByCont)
	}
	if got := rep.ByCont[0].UptimePct; got != 75 {
		t.Fatalf("uptime_pct: want 75 got %v", got)
	}

	if len(rep.ByPeriod) == 0 {
		t.Fatal("by_period must not be empty")
	}
	if rep.PeriodS != 3600 {
		t.Fatalf("a 1h window should use hour buckets, got period_s=%d", rep.PeriodS)
	}
}

// A window longer than two days rolls up per day instead of per hour, so the
// table stays ≤24 rows instead of 168.
func TestAnalyticsSwitchesToDayBuckets(t *testing.T) {
	end := time.Now().Unix()
	st := seedStore(t, 10, 60, end)

	rep, err := st.analytics(end-5*86400, end, 60)
	if err != nil {
		t.Fatalf("analytics: %v", err)
	}
	if rep.PeriodS != 86400 {
		t.Fatalf("a 5-day window should use day buckets, got period_s=%d", rep.PeriodS)
	}
}

// An empty store must return a well-formed report with zeroed totals and empty
// (non-nil) slices — the desktop renders it as "no data yet", not as an error.
func TestAnalyticsEmptyStoreIsNotAnError(t *testing.T) {
	end := time.Now().Unix()
	st := seedStore(t, 0, 5, end)

	rep, err := st.analytics(end-3600, end, 60)
	if err != nil {
		t.Fatalf("analytics on empty store: %v", err)
	}
	if rep.Totals.Samples != 0 || rep.Totals.CPUAvg != 0 || rep.Totals.RxBytes != 0 {
		t.Fatalf("empty store should zero the totals, got %+v", rep.Totals)
	}
	for name, n := range map[string]int{
		"series": len(rep.Series), "by_period": len(rep.ByPeriod),
		"by_disk": len(rep.ByDisk), "by_container": len(rep.ByCont),
	} {
		if n != 0 {
			t.Fatalf("%s should be empty, got %d rows", name, n)
		}
	}
	// Encodes as [] rather than null, so the client can map() without a guard.
	b, err := json.Marshal(rep)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	for _, want := range []string{`"series":[]`, `"by_disk":[]`, `"by_container":[]`} {
		if !strings.Contains(string(b), want) {
			t.Fatalf("json missing %s: %s", want, b)
		}
	}
}

// The handler clamps rather than 400s: a nonsense range still renders.
func TestHandleAnalyticsClampsRange(t *testing.T) {
	end := time.Now().Unix()
	st := seedStore(t, 50, 5, end)
	svc := NewService(st, NewSampler(), "")

	for _, q := range []string{
		"",                              // defaults to 24h
		"?since=0&until=0",              // both garbage
		"?since=99999999999",            // since in the far future
		"?since=1&points=99999",         // span beyond retention, absurd points
		"?since=not-a-time&points=-4",   // unparseable
		"?until=" + strconv.FormatInt(1<<40, 10), // until in the far future
	} {
		req := httptest.NewRequest("GET", "/analytics"+q, nil)
		rec := httptest.NewRecorder()
		svc.handleAnalytics(rec, req)
		if rec.Code != http.StatusOK {
			t.Fatalf("query %q: want 200 got %d (%s)", q, rec.Code, rec.Body.String())
		}
		var rep AnalyticsReport
		if err := json.Unmarshal(rec.Body.Bytes(), &rep); err != nil {
			t.Fatalf("query %q: bad json: %v", q, err)
		}
		if rep.Since >= rep.Until {
			t.Fatalf("query %q: clamped to an empty window %d..%d", q, rep.Since, rep.Until)
		}
		if rep.Until-rep.Since > int64(analyticsMaxSpan/time.Second) {
			t.Fatalf("query %q: span %ds exceeds retention", q, rep.Until-rep.Since)
		}
		if rep.BucketS < 1 {
			t.Fatalf("query %q: bucket_s must be ≥ 1, got %d", q, rep.BucketS)
		}
	}
}

// No store wired (the unit-test Service shape) → 503, never a panic.
func TestHandleAnalyticsWithoutStore(t *testing.T) {
	svc := NewService(nil, NewSampler(), "")
	rec := httptest.NewRecorder()
	svc.handleAnalytics(rec, httptest.NewRequest("GET", "/analytics", nil))
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("want 503 got %d", rec.Code)
	}
}
