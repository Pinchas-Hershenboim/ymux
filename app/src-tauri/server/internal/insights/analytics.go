package insights

// analytics.go — the `/analytics` endpoint behind the Monitor's Analytics tab.
//
// Why a dedicated endpoint instead of reusing `/history`: every desktop fetch is
// one `curl` over the workspace SSH session with `--max-time 6` (see
// `insights_fetch` in addons.rs), so N round trips for N series is not an
// option — the whole screen has to come back in ONE response. And `/history` is
// raw rows with `LIMIT 2000`: at the 5s sample interval that is 2.8 hours, so a
// "last 7 days" question served from it would silently answer with the OLDEST
// 2.8 hours of the window. Aggregation belongs in SQL, on the server; the client
// only draws.
//
// The three tables (`samples`, `disk_samples`, `docker_samples`) are already
// written by the sampler and swept to a 7-day retention window — this endpoint
// is the first reader of the two per-entity ones.

import (
	"database/sql"
	"math"
	"net/http"
	"strconv"
	"time"
)

// Widest window we will aggregate. Matches the store's retention — asking for
// more than is kept would just render a half-empty chart.
const analyticsMaxSpan = retentionDays * 24 * time.Hour

// Narrowest window. Below this the buckets hold one sample each and the chart
// is noise.
const analyticsMinSpan = 5 * time.Minute

// cpuBusyPct — a sample at or above this counts toward `busy_pct`.
const cpuBusyPct = 80.0

// memPctExpr computes used-memory percent, guarding the divide-by-zero that a
// sample written before the first successful mem read would otherwise cause.
const memPctExpr = `(CASE WHEN mem_total > 0 THEN mem_used * 100.0 / mem_total END)`

// AnalyticsTotals is the stat-tile row: one aggregate over the whole window.
type AnalyticsTotals struct {
	Samples    int     `json:"samples"`
	FirstTS    int64   `json:"first_ts"`
	LastTS     int64   `json:"last_ts"`
	CPUAvg     float64 `json:"cpu_avg"`
	CPUMax     float64 `json:"cpu_max"`
	BusyPct    float64 `json:"busy_pct"`
	MemPctAvg  float64 `json:"mem_pct_avg"`
	MemPctMax  float64 `json:"mem_pct_max"`
	MemUsedMax uint64  `json:"mem_used_max"`
	MemTotal   uint64  `json:"mem_total"`
	SwapMax    uint64  `json:"swap_max"`
	LoadAvg    float64 `json:"load_avg"`
	LoadMax    float64 `json:"load_max"`
	RxAvgBps   float64 `json:"rx_avg_bps"`
	TxAvgBps   float64 `json:"tx_avg_bps"`
	RxBytes    float64 `json:"rx_bytes"`
	TxBytes    float64 `json:"tx_bytes"`
}

// AnalyticsPoint is one bucket of the sparkline series.
type AnalyticsPoint struct {
	T      int64   `json:"t"`
	N      int     `json:"n"`
	CPU    float64 `json:"cpu"`
	CPUMax float64 `json:"cpu_max"`
	MemPct float64 `json:"mem_pct"`
	Load   float64 `json:"load"`
	RxBps  float64 `json:"rx_bps"`
	TxBps  float64 `json:"tx_bps"`
}

// AnalyticsPeriod is one row of the by-period rollup (hour or day buckets).
type AnalyticsPeriod struct {
	T         int64   `json:"t"`
	N         int     `json:"n"`
	CPUAvg    float64 `json:"cpu_avg"`
	CPUMax    float64 `json:"cpu_max"`
	MemPctAvg float64 `json:"mem_pct_avg"`
	LoadAvg   float64 `json:"load_avg"`
}

// AnalyticsDisk is one row of the per-mount rollup. `GrowthBytes` is signed:
// last observed `used` minus the first in the window, which is the number
// people actually want ("/var grew 3 GB overnight").
type AnalyticsDisk struct {
	Mount       string  `json:"mount"`
	UsedAvg     float64 `json:"used_avg"`
	UsedLast    uint64  `json:"used_last"`
	Total       uint64  `json:"total"`
	PctLast     float64 `json:"pct_last"`
	GrowthBytes int64   `json:"growth_bytes"`
	N           int     `json:"n"`
}

// AnalyticsContainer is one row of the per-container rollup. `UptimePct` is the
// share of samples in which the container was running — a container that keeps
// crash-looping shows up here as a low number, which `/docker` (a point-in-time
// list) cannot tell you.
type AnalyticsContainer struct {
	Name      string  `json:"name"`
	CPUAvg    float64 `json:"cpu_avg"`
	CPUMax    float64 `json:"cpu_max"`
	MemAvg    float64 `json:"mem_avg"`
	MemMax    uint64  `json:"mem_max"`
	UptimePct float64 `json:"uptime_pct"`
	N         int     `json:"n"`
}

// AnalyticsReport is the whole screen in one response.
type AnalyticsReport struct {
	// Bucketed marks this as a real aggregate. A daemon predating this
	// endpoint has no /analytics route at all and answers 404, which is how
	// the desktop tells "old daemon" apart from "no data yet".
	Bucketed bool                 `json:"bucketed"`
	Since    int64                `json:"since"`
	Until    int64                `json:"until"`
	BucketS  int64                `json:"bucket_s"`
	PeriodS  int64                `json:"period_s"`
	Totals   AnalyticsTotals      `json:"totals"`
	Series   []AnalyticsPoint     `json:"series"`
	ByPeriod []AnalyticsPeriod    `json:"by_period"`
	ByDisk   []AnalyticsDisk      `json:"by_disk"`
	ByCont   []AnalyticsContainer `json:"by_container"`
}

// nz reads a nullable aggregate as 0 when the window held no rows, and rounds
// to two decimals so the wire form doesn't carry float noise across SSH.
func nz(v sql.NullFloat64) float64 {
	if !v.Valid || math.IsNaN(v.Float64) || math.IsInf(v.Float64, 0) {
		return 0
	}
	return math.Round(v.Float64*100) / 100
}

func nzU(v sql.NullFloat64) uint64 {
	if !v.Valid || v.Float64 <= 0 {
		return 0
	}
	return uint64(v.Float64)
}

// analytics aggregates one window into a report. `points` is the number of
// sparkline buckets to produce; the caller clamps it.
func (s *Store) analytics(since, until int64, points int) (*AnalyticsReport, error) {
	span := until - since
	if span < 1 {
		span = 1
	}
	bucket := span / int64(points)
	if bucket < 1 {
		bucket = 1
	}
	// Hour buckets for short windows, day buckets once the window is long
	// enough that per-hour rows would be a wall of numbers. Keys stay unix
	// timestamps so the desktop formats them in the VIEWER's timezone — the
	// server's TZ is not the one the user reads.
	period := int64(3600)
	if span >= 48*3600 {
		period = 86400
	}

	rep := &AnalyticsReport{
		Bucketed: true,
		Since:    since,
		Until:    until,
		BucketS:  bucket,
		PeriodS:  period,
		Series:   []AnalyticsPoint{},
		ByPeriod: []AnalyticsPeriod{},
		ByDisk:   []AnalyticsDisk{},
		ByCont:   []AnalyticsContainer{},
	}

	if err := s.analyticsTotals(rep, since, until); err != nil {
		return nil, err
	}
	if err := s.analyticsSeries(rep, since, until, bucket); err != nil {
		return nil, err
	}
	if err := s.analyticsByPeriod(rep, since, until, period); err != nil {
		return nil, err
	}
	if err := s.analyticsByDisk(rep, since, until); err != nil {
		return nil, err
	}
	if err := s.analyticsByContainer(rep, since, until); err != nil {
		return nil, err
	}
	return rep, nil
}

func (s *Store) analyticsTotals(rep *AnalyticsReport, since, until int64) error {
	row := s.db.QueryRow(`
		SELECT COUNT(*), MIN(ts), MAX(ts),
		       AVG(cpu_pct), MAX(cpu_pct),
		       SUM(CASE WHEN cpu_pct >= ? THEN 1 ELSE 0 END),
		       AVG(`+memPctExpr+`), MAX(`+memPctExpr+`),
		       MAX(mem_used), MAX(mem_total), MAX(swap_used),
		       AVG(load1), MAX(load1),
		       AVG(net_rx_bps), AVG(net_tx_bps)
		FROM samples WHERE ts >= ? AND ts <= ?`,
		cpuBusyPct, since, until)

	var n int
	var first, last sql.NullInt64
	var busy sql.NullFloat64
	var cpuAvg, cpuMax, memAvg, memMax, memUsedMax, memTotal, swapMax sql.NullFloat64
	var loadAvg, loadMax, rxAvg, txAvg sql.NullFloat64
	if err := row.Scan(&n, &first, &last, &cpuAvg, &cpuMax, &busy,
		&memAvg, &memMax, &memUsedMax, &memTotal, &swapMax,
		&loadAvg, &loadMax, &rxAvg, &txAvg); err != nil {
		return err
	}

	t := AnalyticsTotals{
		Samples:    n,
		FirstTS:    first.Int64,
		LastTS:     last.Int64,
		CPUAvg:     nz(cpuAvg),
		CPUMax:     nz(cpuMax),
		MemPctAvg:  nz(memAvg),
		MemPctMax:  nz(memMax),
		MemUsedMax: nzU(memUsedMax),
		MemTotal:   nzU(memTotal),
		SwapMax:    nzU(swapMax),
		LoadAvg:    nz(loadAvg),
		LoadMax:    nz(loadMax),
		RxAvgBps:   nz(rxAvg),
		TxAvgBps:   nz(txAvg),
	}
	if n > 0 && busy.Valid {
		t.BusyPct = math.Round(busy.Float64/float64(n)*10000) / 100
	}
	// Transferred bytes ≈ mean rate × observed span. The sampler stores a rate,
	// not a counter, so this is an estimate and is labelled as one in the UI.
	if observed := t.LastTS - t.FirstTS; observed > 0 {
		t.RxBytes = math.Round(t.RxAvgBps * float64(observed))
		t.TxBytes = math.Round(t.TxAvgBps * float64(observed))
	}
	rep.Totals = t
	return nil
}

func (s *Store) analyticsSeries(rep *AnalyticsReport, since, until, bucket int64) error {
	rows, err := s.db.Query(`
		SELECT (ts / ?) * ? AS b, COUNT(*),
		       AVG(cpu_pct), MAX(cpu_pct), AVG(`+memPctExpr+`),
		       AVG(load1), AVG(net_rx_bps), AVG(net_tx_bps)
		FROM samples WHERE ts >= ? AND ts <= ?
		GROUP BY b ORDER BY b ASC`,
		bucket, bucket, since, until)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var p AnalyticsPoint
		var cpu, cpuMax, memPct, load, rx, tx sql.NullFloat64
		if err := rows.Scan(&p.T, &p.N, &cpu, &cpuMax, &memPct, &load, &rx, &tx); err != nil {
			continue
		}
		p.CPU, p.CPUMax, p.MemPct = nz(cpu), nz(cpuMax), nz(memPct)
		p.Load, p.RxBps, p.TxBps = nz(load), nz(rx), nz(tx)
		rep.Series = append(rep.Series, p)
	}
	return rows.Err()
}

func (s *Store) analyticsByPeriod(rep *AnalyticsReport, since, until, period int64) error {
	// Newest first (that is the order the table renders in), capped so a 7-day
	// window in hour buckets can't return 168 rows.
	rows, err := s.db.Query(`
		SELECT (ts / ?) * ? AS b, COUNT(*),
		       AVG(cpu_pct), MAX(cpu_pct), AVG(`+memPctExpr+`), AVG(load1)
		FROM samples WHERE ts >= ? AND ts <= ?
		GROUP BY b ORDER BY b DESC LIMIT 24`,
		period, period, since, until)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var r AnalyticsPeriod
		var cpuAvg, cpuMax, memAvg, loadAvg sql.NullFloat64
		if err := rows.Scan(&r.T, &r.N, &cpuAvg, &cpuMax, &memAvg, &loadAvg); err != nil {
			continue
		}
		r.CPUAvg, r.CPUMax = nz(cpuAvg), nz(cpuMax)
		r.MemPctAvg, r.LoadAvg = nz(memAvg), nz(loadAvg)
		rep.ByPeriod = append(rep.ByPeriod, r)
	}
	return rows.Err()
}

func (s *Store) analyticsByDisk(rep *AnalyticsReport, since, until int64) error {
	// The two correlated subqueries pick the first and last `used` reading per
	// mount inside the window — that difference is the growth number. Both ride
	// idx_disk_ts and there are only a handful of mounts.
	rows, err := s.db.Query(`
		SELECT d.mount, COUNT(*), AVG(d.used), MAX(d.total),
		       (SELECT f.used FROM disk_samples f
		         WHERE f.mount = d.mount AND f.ts >= ? AND f.ts <= ?
		         ORDER BY f.ts ASC LIMIT 1),
		       (SELECT l.used FROM disk_samples l
		         WHERE l.mount = d.mount AND l.ts >= ? AND l.ts <= ?
		         ORDER BY l.ts DESC LIMIT 1)
		FROM disk_samples d WHERE d.ts >= ? AND d.ts <= ?
		GROUP BY d.mount ORDER BY 3 DESC`,
		since, until, since, until, since, until)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var d AnalyticsDisk
		var avg, total, firstUsed, lastUsed sql.NullFloat64
		if err := rows.Scan(&d.Mount, &d.N, &avg, &total, &firstUsed, &lastUsed); err != nil {
			continue
		}
		d.UsedAvg = nz(avg)
		d.Total = nzU(total)
		d.UsedLast = nzU(lastUsed)
		if d.Total > 0 {
			d.PctLast = math.Round(float64(d.UsedLast)/float64(d.Total)*10000) / 100
		}
		if firstUsed.Valid && lastUsed.Valid {
			d.GrowthBytes = int64(lastUsed.Float64 - firstUsed.Float64)
		}
		rep.ByDisk = append(rep.ByDisk, d)
	}
	return rows.Err()
}

func (s *Store) analyticsByContainer(rep *AnalyticsReport, since, until int64) error {
	rows, err := s.db.Query(`
		SELECT name, COUNT(*), AVG(cpu_pct), MAX(cpu_pct), AVG(mem_used), MAX(mem_used),
		       SUM(CASE WHEN state = 'running' THEN 1 ELSE 0 END)
		FROM docker_samples WHERE ts >= ? AND ts <= ? AND name <> ''
		GROUP BY name ORDER BY 3 DESC LIMIT 20`,
		since, until)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var c AnalyticsContainer
		var cpuAvg, cpuMax, memAvg, memMax, running sql.NullFloat64
		if err := rows.Scan(&c.Name, &c.N, &cpuAvg, &cpuMax, &memAvg, &memMax, &running); err != nil {
			continue
		}
		c.CPUAvg, c.CPUMax, c.MemAvg = nz(cpuAvg), nz(cpuMax), nz(memAvg)
		c.MemMax = nzU(memMax)
		if c.N > 0 && running.Valid {
			c.UptimePct = math.Round(running.Float64/float64(c.N)*10000) / 100
		}
		rep.ByCont = append(rep.ByCont, c)
	}
	return rows.Err()
}

// parseWhen reads a unix-seconds or RFC3339 query value; 0 when absent/garbage.
func parseWhen(v string) int64 {
	if v == "" {
		return 0
	}
	if n, err := strconv.ParseInt(v, 10, 64); err == nil {
		return n
	}
	if t, err := time.Parse(time.RFC3339, v); err == nil {
		return t.Unix()
	}
	return 0
}

// handleAnalytics serves GET /analytics?since=&until=&points=.
//
// Everything is clamped rather than rejected (the desktop should never have to
// render a validation error for a range picker it drew itself).
func (s *Service) handleAnalytics(w http.ResponseWriter, r *http.Request) {
	now := time.Now().Unix()
	until := parseWhen(r.URL.Query().Get("until"))
	if until <= 0 || until > now {
		until = now
	}
	since := parseWhen(r.URL.Query().Get("since"))
	if since <= 0 {
		since = until - int64(24*time.Hour/time.Second)
	}
	if oldest := until - int64(analyticsMaxSpan/time.Second); since < oldest {
		since = oldest
	}
	if newest := until - int64(analyticsMinSpan/time.Second); since > newest {
		since = newest
	}

	points := 120
	if v, err := strconv.Atoi(r.URL.Query().Get("points")); err == nil && v > 0 {
		points = v
	}
	if points < 20 {
		points = 20
	}
	if points > 400 {
		points = 400
	}

	if s.store == nil {
		http.Error(w, "metrics store unavailable", http.StatusServiceUnavailable)
		return
	}
	t0 := time.Now()
	rep, err := s.store.analytics(since, until, points)
	if err != nil {
		logger.Error("analytics query failed", "err", err, "since", since, "until", until)
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	logger.Debug("analytics ok",
		"span_s", until-since, "buckets", len(rep.Series),
		"samples", rep.Totals.Samples, "took_ms", time.Since(t0).Milliseconds())
	writeJSON(w, rep)
}
