package logging

import (
	"bytes"
	"errors"
	"log"
	"log/slog"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

// lineRe matches one full unified-format line:
// [2026-07-15 14:32:05.123 +03:00] [INFO ] [SRV:MAIN] message ...
var lineRe = regexp.MustCompile(
	`^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} [+-]\d{2}:\d{2}\] ` +
		`\[(DEBUG|INFO |WARN |ERROR)\] \[[A-Z]+:[A-Z]+\] .*$`)

// capture redirects the shared writer to a buffer for the test's duration and
// resets Level afterwards. Tests in this package must not run in parallel.
func capture(t *testing.T) *bytes.Buffer {
	t.Helper()
	buf := &bytes.Buffer{}
	prev := out.get()
	out.set(buf)
	t.Cleanup(func() {
		out.set(prev)
		Level.Set(slog.LevelInfo)
	})
	return buf
}

func lines(buf *bytes.Buffer) []string {
	s := strings.TrimRight(buf.String(), "\n")
	if s == "" {
		return nil
	}
	return strings.Split(s, "\n")
}

func TestLineFormat(t *testing.T) {
	buf := capture(t)
	New("SRV:CHAT").Info("session stopped", "session", "abc123")

	got := lines(buf)
	if len(got) != 1 {
		t.Fatalf("want 1 line, got %d: %q", len(got), got)
	}
	if !lineRe.MatchString(got[0]) {
		t.Errorf("line does not match unified format: %q", got[0])
	}
	if !strings.Contains(got[0], "] [INFO ] [SRV:CHAT] session stopped session=abc123") {
		t.Errorf("unexpected tail: %q", got[0])
	}
	// Timestamp is local time with the local UTC offset.
	wantOff := time.Now().Format("-07:00")
	if !strings.Contains(got[0], " "+wantOff+"] ") {
		t.Errorf("line %q missing local offset %q", got[0], wantOff)
	}
}

func TestLevelColumnWidth(t *testing.T) {
	buf := capture(t)
	Level.Set(slog.LevelDebug)
	lg := New("SRV:TEST")
	lg.Debug("d")
	lg.Info("i")
	lg.Warn("w")
	lg.Error("e")

	got := lines(buf)
	if len(got) != 4 {
		t.Fatalf("want 4 lines, got %d: %q", len(got), got)
	}
	for i, tag := range []string{"[DEBUG]", "[INFO ]", "[WARN ]", "[ERROR]"} {
		if !strings.Contains(got[i], "] "+tag+" [") {
			t.Errorf("line %d: want tag %q in %q", i, tag, got[i])
		}
		if !lineRe.MatchString(got[i]) {
			t.Errorf("line %d does not match format: %q", i, got[i])
		}
	}
}

func TestRuntimeFiltering(t *testing.T) {
	buf := capture(t)
	lg := New("SRV:TEST")

	lg.Debug("hidden-debug") // default Info: filtered
	lg.Info("shown-info")

	Level.Set(slog.LevelError)
	lg.Info("hidden-info")
	lg.Warn("hidden-warn")
	lg.Error("shown-error")

	Level.Set(slog.LevelDebug)
	lg.Debug("shown-debug")

	s := buf.String()
	for _, want := range []string{"shown-info", "shown-error", "shown-debug"} {
		if !strings.Contains(s, want) {
			t.Errorf("output missing %q:\n%s", want, s)
		}
	}
	for _, not := range []string{"hidden-debug", "hidden-info", "hidden-warn"} {
		if strings.Contains(s, not) {
			t.Errorf("output should not contain %q:\n%s", not, s)
		}
	}
}

func TestAttrRendering(t *testing.T) {
	buf := capture(t)
	lg := New("SRV:TEST")

	lg.Info("attrs",
		"plain", "val",
		"count", 42,
		"spaced", "a b",
		"eq", "a=b",
		"quoted", `he said "hi"`,
		"empty", "",
		"err", errors.New("boom failed"))

	line := lines(buf)[0]
	for _, want := range []string{
		" plain=val",
		" count=42",
		` spaced="a b"`,
		` eq="a=b"`,
		` quoted="he said \"hi\""`,
		` empty=""`,
		` err="boom failed"`,
	} {
		if !strings.Contains(line, want) {
			t.Errorf("line missing %q: %q", want, line)
		}
	}
}

func TestWithAttrsAndGroups(t *testing.T) {
	buf := capture(t)
	lg := New("SRV:TEST").With("device", "d1")
	lg.WithGroup("ws").Info("fanout", "session", "s1")

	line := lines(buf)[0]
	if !strings.Contains(line, "[SRV:TEST] fanout device=d1 ws.session=s1") {
		t.Errorf("unexpected line: %q", line)
	}
}

func TestCompReservedAttr(t *testing.T) {
	buf := capture(t)
	New("SRV:MAIN").With("comp", "SRV:PUSH").Info("swapped")

	line := lines(buf)[0]
	if !strings.Contains(line, "[SRV:PUSH] swapped") {
		t.Errorf("comp attr should replace component tag: %q", line)
	}
	if strings.Contains(line, "comp=") {
		t.Errorf("reserved comp attr must not render as key=val: %q", line)
	}
}

func TestStdlibBridge(t *testing.T) {
	buf := capture(t)
	Setup(buf) // re-points the shared writer at buf and installs the bridge

	log.Printf("stray printf %d", 7)

	got := lines(buf)
	if len(got) != 1 {
		t.Fatalf("want 1 line, got %d: %q", len(got), got)
	}
	if !lineRe.MatchString(got[0]) {
		t.Errorf("bridged line does not match format: %q", got[0])
	}
	if !strings.Contains(got[0], "] [INFO ] [SRV:MAIN] stray printf 7") {
		t.Errorf("unexpected bridged line: %q", got[0])
	}

	// Bridged lines respect runtime filtering too.
	Level.Set(slog.LevelError)
	log.Printf("filtered printf")
	if strings.Contains(buf.String(), "filtered printf") {
		t.Errorf("bridged Info line should be filtered at level=error")
	}
}

func TestParseLevel(t *testing.T) {
	cases := []struct {
		in   string
		want slog.Level
	}{
		{"debug", slog.LevelDebug},
		{"info", slog.LevelInfo},
		{"warn", slog.LevelWarn},
		{"error", slog.LevelError},
		{"  DEBUG \n", slog.LevelDebug},
		{"Error\r\n", slog.LevelError},
		{"bogus", slog.LevelInfo},
		{"", slog.LevelInfo},
	}
	for _, c := range cases {
		if got := parseLevel(c.in); got != c.want {
			t.Errorf("parseLevel(%q) = %v, want %v", c.in, got, c.want)
		}
	}
}

func TestReadLevelFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "log-level")

	if got := readLevelFile(path); got != slog.LevelInfo {
		t.Errorf("missing file: got %v, want Info", got)
	}
	if err := os.WriteFile(path, []byte("  warn\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := readLevelFile(path); got != slog.LevelWarn {
		t.Errorf("whitespace file: got %v, want Warn", got)
	}
	if err := os.WriteFile(path, []byte("verbose"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := readLevelFile(path); got != slog.LevelInfo {
		t.Errorf("bogus file: got %v, want Info", got)
	}
}

func TestWatchLevelFile(t *testing.T) {
	buf := capture(t)
	dir := t.TempDir()
	path := filepath.Join(dir, "log-level")

	prevInterval := watchInterval
	watchInterval = 10 * time.Millisecond
	t.Cleanup(func() { watchInterval = prevInterval })

	stop := make(chan struct{})
	done := make(chan struct{})
	go func() {
		WatchLevelFile(path, stop)
		close(done)
	}()
	t.Cleanup(func() {
		close(stop)
		<-done
	})

	waitFor := func(desc string, cond func() bool) {
		t.Helper()
		deadline := time.Now().Add(5 * time.Second)
		for time.Now().Before(deadline) {
			if cond() {
				return
			}
			time.Sleep(5 * time.Millisecond)
		}
		t.Fatalf("timeout waiting for %s", desc)
	}

	// Missing file: stays Info, no change line.
	time.Sleep(50 * time.Millisecond)
	if Level.Level() != slog.LevelInfo {
		t.Fatalf("missing file: level = %v, want Info", Level.Level())
	}
	if strings.Contains(buf.String(), "log level changed") {
		t.Fatalf("no change should be logged for a missing file:\n%s", buf.String())
	}

	// error: level applies and exactly one change line is written — through
	// the handler directly, so it survives the new error-level filter.
	if err := os.WriteFile(path, []byte("error\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	waitFor("level=error", func() bool { return Level.Level() == slog.LevelError })
	waitFor("change line", func() bool {
		return strings.Count(buf.String(), "log level changed") == 1
	})
	if !strings.Contains(buf.String(), "from=info to=error") {
		t.Errorf("change line should carry from/to:\n%s", buf.String())
	}

	// Unchanged content: no extra change lines after several polls.
	time.Sleep(50 * time.Millisecond)
	if n := strings.Count(buf.String(), "log level changed"); n != 1 {
		t.Errorf("steady state: want 1 change line, got %d:\n%s", n, buf.String())
	}

	// Bogus content: falls back to Info, second change line.
	if err := os.WriteFile(path, []byte("chatty"), 0o644); err != nil {
		t.Fatal(err)
	}
	waitFor("level back to Info", func() bool { return Level.Level() == slog.LevelInfo })
	waitFor("second change line", func() bool {
		return strings.Count(buf.String(), "log level changed") == 2
	})
}
