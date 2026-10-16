package otlp

import (
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/usage"
)

func TestOTelSessionDiscoveredDuringFirstRequest(t *testing.T) {
	t.Setenv("PI_CODING_AGENT_DIR", t.TempDir())
	t.Setenv("PI_CODING_AGENT_SESSION_DIR", "")
	t.Setenv("CODEX_HOME", t.TempDir())
	t.Setenv("MAGPIE_OTEL_ENABLED", "true")
	t.Setenv("MAGPIE_OTEL_ENDPOINT", "http://127.0.0.1:1")
	t.Setenv("MAGPIE_OTEL_HEADERS", "")
	t.Setenv("MAGPIE_OTEL_SESSIONS", "true")
	fastOTel(t)
	// Begin's cached miss must outlast the test however slow the machine
	// is (#1811): neither the transcript watcher's poll nor the end of the
	// discovery cache window may find the transcript before skip() does.
	sessionsEvery, ttl := OTelSessionsEvery, sessions.TraceIdentityTTL
	OTelSessionsEvery, sessions.TraceIdentityTTL = time.Hour, time.Hour
	t.Cleanup(func() { OTelSessionsEvery, sessions.TraceIdentityTTL = sessionsEvery, ttl })
	stop := StartOTel()
	t.Cleanup(stop)
	// a local pi turn with no key, as the gateway describes it
	q := usage.Request{Header: http.Header{}, Agent: "pi", Session: "late-session", Plain: true}
	pending, _ := testX.Begin(q).(*request)
	exported, _ := testX.Begin(q).(*request)
	if pending == nil || exported == nil || !pending.sessionCandidate {
		t.Fatal("unseen session must retain gateway fallback")
	}
	if exported.skip() {
		t.Fatal("unseen session suppressed")
	}
	dir := filepath.Join(os.Getenv("PI_CODING_AGENT_DIR"), "sessions", "work")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "2026-10-03T00-00-00_late-session.jsonl"), []byte("{\"type\":\"session\",\"id\":\"late-session\"}\n"), 0600); err != nil {
		t.Fatal(err)
	}
	if OTelSession("pi", "late-session") {
		t.Fatal("test must exercise a fresh cached miss")
	}
	if !pending.skip() {
		t.Fatal("newly discovered transcript duplicated gateway trace")
	}
	if exported.skip() {
		t.Fatal("discovery after an exported attempt must retain its root")
	}
	remote := &request{x: testX, agent: "pi", session: "late-session"}
	if remote.skip() {
		t.Fatal("remote/keyed request must retain gateway tracing")
	}
}

func TestOTelParentValidation(t *testing.T) {
	valid := "00-" + strings.Repeat("a", 32) + "-" + strings.Repeat("b", 16) + "-01"
	for _, value := range []string{valid, "", strings.Replace(valid, "00-", "ff-", 1), strings.ToUpper(valid), strings.Replace(valid, strings.Repeat("a", 32), strings.Repeat("0", 32), 1), valid + "-extra", valid[:len(valid)-1] + "z"} {
		trace, parent := otelParent(value)
		if value == valid {
			if trace == "" || parent == "" {
				t.Fatal("valid parent rejected")
			}
		} else if trace != "" || parent != "" {
			t.Fatalf("accepted malformed parent: %q", value)
		}
	}
}

// fastOTel has the exporter read the agents' transcripts and send what it
// holds within a fraction of a second, not every 2s and 5s.
func fastOTel(t *testing.T) {
	every, sessionsEvery := OTelEvery, OTelSessionsEvery
	OTelEvery, OTelSessionsEvery = 200*time.Millisecond, 50*time.Millisecond
	t.Cleanup(func() { OTelEvery, OTelSessionsEvery = every, sessionsEvery })
}
