// Package logging is the ymux-server unified logger (Phase 79.D). Every
// subsystem logs through log/slog with a shared handler that renders one line
// format:
//
//	[YYYY-MM-DD HH:MM:SS.mmm +TZ:00] [LEVEL] [COMP] message key=val ...
//
// Local time with UTC offset, LEVEL padded to width 5, COMP like [SRV:CHAT].
// slog attrs render as trailing key=val pairs (%q-quoted when the value holds
// spaces, quotes, or '='). The process-wide minimum level lives in Level and
// is runtime-adjustable via WatchLevelFile (~/.ymux/log-level).
//
// Output goes through a shared writer indirection, so package-level loggers
// created before Setup runs (var logger = logging.New("SRV:X")) transparently
// pick up the log file once Setup installs it.
package logging

import (
	"context"
	"fmt"
	"io"
	"log"
	"log/slog"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Level is the process-wide minimum log level. Zero value is Info. Adjust at
// runtime with Level.Set (WatchLevelFile does this from the level file).
var Level slog.LevelVar

// defaultComp tags lines from the default logger and the stdlib log bridge.
const defaultComp = "SRV:MAIN"

// compKey is the reserved attr key: WithAttrs treats it as "set the component
// tag" rather than rendering a comp=... pair.
const compKey = "comp"

// watchInterval is how often WatchLevelFile re-reads the level file. A var so
// tests can shrink it.
var watchInterval = 30 * time.Second

// sharedWriter is the swappable output sink. All handler instances write
// through the package-level `out`, so Setup can retarget every logger — even
// ones created earlier — by swapping the inner writer.
type sharedWriter struct {
	mu sync.Mutex
	w  io.Writer
}

func (s *sharedWriter) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.w.Write(p)
}

func (s *sharedWriter) set(w io.Writer) {
	s.mu.Lock()
	s.w = w
	s.mu.Unlock()
}

func (s *sharedWriter) get() io.Writer {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.w
}

var out = &sharedWriter{w: os.Stderr}

// New returns a logger whose lines carry the given component tag, e.g.
// New("SRV:CHAT"). Safe to call at package init, before Setup.
func New(comp string) *slog.Logger {
	return slog.New(&handler{comp: comp})
}

// Setup points all loggers at w, installs the default slog logger (SRV:MAIN),
// and bridges the stdlib log package so any stray log.Printf lands in the
// unified format at Info under SRV:MAIN.
func Setup(w io.Writer) {
	out.set(w)
	slog.SetDefault(New(defaultComp))
	// Bridge stdlib log: no stdlib prefix/timestamp (the handler owns the
	// line), each Print/Printf becomes one Info record on the default logger.
	log.SetFlags(0)
	log.SetOutput(stdlogBridge{})
}

// stdlogBridge adapts the stdlib log package onto the default slog logger.
type stdlogBridge struct{}

func (stdlogBridge) Write(p []byte) (int, error) {
	slog.Default().Info(strings.TrimRight(string(p), "\r\n"))
	return len(p), nil
}

// WatchLevelFile polls path every 30s and applies its content to Level:
// debug/info/warn/error (case-insensitive, whitespace-trimmed). A missing or
// unparsable file means Info. Logs one Info line only when the level actually
// changes — written through the handler directly so it lands even when the
// active level would filter Info. Blocks until stop closes; run as a goroutine.
func WatchLevelFile(path string, stop <-chan struct{}) {
	check := func() {
		want := readLevelFile(path)
		if cur := Level.Level(); cur != want {
			Level.Set(want)
			// Bypass Enabled filtering: at level=error an Info record would be
			// dropped, but the operator must see that the switch took effect.
			var b strings.Builder
			writeHeader(&b, time.Now(), slog.LevelInfo, defaultComp)
			b.WriteString("log level changed")
			b.WriteString(" from=" + levelName(cur))
			b.WriteString(" to=" + levelName(want))
			b.WriteByte('\n')
			_, _ = out.Write([]byte(b.String()))
		}
	}
	check()
	t := time.NewTicker(watchInterval)
	defer t.Stop()
	for {
		select {
		case <-stop:
			return
		case <-t.C:
			check()
		}
	}
}

// readLevelFile maps the file content to a level. Missing file, read error,
// or unknown word → Info (the safe default).
func readLevelFile(path string) slog.Level {
	b, err := os.ReadFile(path)
	if err != nil {
		return slog.LevelInfo
	}
	return parseLevel(string(b))
}

func parseLevel(s string) slog.Level {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "debug":
		return slog.LevelDebug
	case "warn":
		return slog.LevelWarn
	case "error":
		return slog.LevelError
	default: // "info", empty, or anything unrecognized
		return slog.LevelInfo
	}
}

func levelName(l slog.Level) string {
	switch {
	case l < slog.LevelInfo:
		return "debug"
	case l < slog.LevelWarn:
		return "info"
	case l < slog.LevelError:
		return "warn"
	default:
		return "error"
	}
}

// ─── handler ────────────────────────────────────────────────────────────────

// handler renders the unified line format. Immutable: WithAttrs/WithGroup
// return copies. attrs holds the pre-rendered " key=val" suffix from
// Logger.With; prefix is the open group path ("g1.g2.") for record attrs.
type handler struct {
	comp   string
	attrs  string
	prefix string
}

func (h *handler) Enabled(_ context.Context, l slog.Level) bool {
	return l >= Level.Level()
}

func (h *handler) Handle(_ context.Context, r slog.Record) error {
	var b strings.Builder
	writeHeader(&b, r.Time, r.Level, h.comp)
	b.WriteString(r.Message)
	b.WriteString(h.attrs)
	r.Attrs(func(a slog.Attr) bool {
		appendAttr(&b, h.prefix, a)
		return true
	})
	b.WriteByte('\n')
	_, err := out.Write([]byte(b.String()))
	return err
}

func (h *handler) WithAttrs(attrs []slog.Attr) slog.Handler {
	nh := *h
	var b strings.Builder
	for _, a := range attrs {
		// Reserved attr: comp swaps the component tag (only outside groups).
		if a.Key == compKey && h.prefix == "" && a.Value.Kind() == slog.KindString {
			nh.comp = a.Value.String()
			continue
		}
		appendAttr(&b, h.prefix, a)
	}
	nh.attrs += b.String()
	return &nh
}

func (h *handler) WithGroup(name string) slog.Handler {
	if name == "" {
		return h
	}
	nh := *h
	nh.prefix += name + "."
	return &nh
}

// writeHeader emits `[2026-07-15 14:32:05.123 +03:00] [INFO ] [SRV:MAIN] `.
func writeHeader(b *strings.Builder, t time.Time, l slog.Level, comp string) {
	if t.IsZero() {
		t = time.Now()
	}
	b.WriteByte('[')
	b.WriteString(t.Format("2006-01-02 15:04:05.000 -07:00"))
	b.WriteString("] [")
	b.WriteString(levelTag(l))
	b.WriteString("] [")
	b.WriteString(comp)
	b.WriteString("] ")
}

// levelTag pads the level name to width 5 so the column aligns.
func levelTag(l slog.Level) string {
	switch {
	case l < slog.LevelInfo:
		return "DEBUG"
	case l < slog.LevelWarn:
		return "INFO "
	case l < slog.LevelError:
		return "WARN "
	default:
		return "ERROR"
	}
}

// appendAttr renders ` key=val`, flattening groups as `group.key=val` and
// %q-quoting values that contain spaces, quotes, or '=' (or are empty).
func appendAttr(b *strings.Builder, prefix string, a slog.Attr) {
	a.Value = a.Value.Resolve()
	if a.Equal(slog.Attr{}) {
		return
	}
	if a.Value.Kind() == slog.KindGroup {
		g := a.Value.Group()
		if len(g) == 0 {
			return
		}
		p := prefix
		if a.Key != "" {
			p += a.Key + "."
		}
		for _, ga := range g {
			appendAttr(b, p, ga)
		}
		return
	}
	if a.Key == "" {
		return
	}
	b.WriteByte(' ')
	b.WriteString(prefix)
	b.WriteString(a.Key)
	b.WriteByte('=')
	b.WriteString(quoteVal(valueString(a.Value)))
}

func valueString(v slog.Value) string {
	if v.Kind() == slog.KindAny {
		if err, ok := v.Any().(error); ok {
			return err.Error()
		}
	}
	return fmt.Sprintf("%v", v.Any())
}

func quoteVal(s string) string {
	if s == "" || strings.ContainsAny(s, " \t\"=") {
		return strconv.Quote(s)
	}
	return s
}
