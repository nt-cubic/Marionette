package gateway

import (
	"bytes"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// groupBenchUpstream answers an Anthropic request as it asked: streamed
// when its body says so. Refusing, it answers code with an overloaded
// error.
type groupBenchUpstream struct{ code int }

const groupBenchReply = `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":1}}`

var groupBenchStream = sse(
	`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"msg_1","model":"m1","usage":{"input_tokens":7}}}`,
	`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
	`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}`,
	`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":0}`,
	`event: message_delta`+"\n"+`data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}`,
	`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)

func (u groupBenchUpstream) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	if u.code != 0 {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Retry-After", "3600")
		w.WriteHeader(u.code)
		io.WriteString(w, `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`)
		return
	}
	if bytes.Contains(b, []byte(`"stream":true`)) {
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, groupBenchStream)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	io.WriteString(w, groupBenchReply)
}

// groupBenchCatalog is a catalog of providers serving models each, the
// same model ids at each (so magpie finds a group of each model), and
// groups of one, four and eight members, one whose first member fails
// every time, and one whose first member is resting.
func groupBenchCatalog(b *testing.B, providers, models int) {
	b.Helper()
	b.Setenv("XDG_CONFIG_HOME", b.TempDir())
	b.Setenv("XDG_CACHE_HOME", b.TempDir())
	good := httptest.NewServer(groupBenchUpstream{})
	b.Cleanup(good.Close)
	failing := httptest.NewServer(groupBenchUpstream{code: 500})
	b.Cleanup(failing.Close)
	resting := httptest.NewServer(groupBenchUpstream{code: 429})
	b.Cleanup(resting.Close)
	var ids []string
	for i := range models {
		ids = append(ids, fmt.Sprintf("m%d", i))
	}
	for i := range providers {
		p := provider.Provider{ID: fmt.Sprintf("p%d", i), Name: fmt.Sprintf("P%d", i), Key: "k", Models: ids, Anthropic: good.URL}
		if err := provider.Save(p); err != nil {
			b.Fatal(err)
		}
	}
	for _, p := range []provider.Provider{
		{ID: "bad", Name: "Bad", Key: "k", Models: ids, Anthropic: failing.URL},
		{ID: "rest", Name: "Rest", Key: "k", Models: ids, Anthropic: resting.URL},
	} {
		if err := provider.Save(p); err != nil {
			b.Fatal(err)
		}
	}
	if err := settings.Save(settings.Settings{}); err != nil {
		b.Fatal(err)
	}
	for _, g := range []provider.Group{
		{ID: "g1", Name: "G1", Members: []string{"p0/m1"}, Routing: provider.Ordered},
		{ID: "g4", Name: "G4", Members: []string{"p0/m1", "p1/m1", "p2/m1", "p3/m1"}, Routing: provider.Ordered},
		{ID: "g8u", Name: "G8U", Members: []string{"p0/m1", "p1/m1", "p2/m1", "p3/m1", "p4/m1", "p5/m1", "p6/m1", "p7/m1"}, Routing: provider.LeastUsed},
		{ID: "gfail", Name: "GFail", Members: []string{"bad/m1", "p0/m1", "p1/m1"}, Routing: provider.Ordered},
		{ID: "grest", Name: "GRest", Members: []string{"rest/m1", "p0/m1", "p1/m1"}, Routing: provider.Ordered},
	} {
		if err := provider.SaveGroup(g); err != nil {
			b.Fatal(err)
		}
	}
	// as a user's files are: written a while ago, not just now (a file
	// written in the last seconds is read again each time)
	old := time.Now().Add(-time.Hour)
	filepath.WalkDir(os.Getenv("XDG_CONFIG_HOME"), func(path string, _ fs.DirEntry, _ error) error {
		return os.Chtimes(path, old, old)
	})
	provider.Changed()
}

// groupBenchCase is one benchmark's request.
type groupBenchCase struct {
	providers, models, words int
	stream                   bool
	model                    string
}

func (c groupBenchCase) name() string {
	return fmt.Sprintf("catalog=%dx%d/%dKB/stream=%v/%s", c.providers, c.models, c.words*4/1000, c.stream, c.model)
}

// groupBenchCases: a dozen providers of 20 models, with a ~200KB and a
// ~2MB conversation, and a large catalog (30 providers of 60 models, as
// a user of several relays has) with the ~200KB one.
func groupBenchCases() []groupBenchCase {
	var out []groupBenchCase
	for _, size := range []struct{ providers, models, words int }{{12, 20, 50_000}, {12, 20, 500_000}, {30, 60, 50_000}} {
		for _, stream := range []bool{false, true} {
			for _, model := range []string{"p0/m1", "group/g1", "group/g4", "group/g8u", "group/gfail", "group/grest"} {
				out = append(out, groupBenchCase{size.providers, size.models, size.words, stream, model})
			}
		}
	}
	return out
}

// groupBenchServer is the gateway with c's catalog, and a request of the
// body c sends that fails the benchmark unless it is answered.
func groupBenchServer(b *testing.B, c groupBenchCase) func() {
	noRests()
	b.Cleanup(noRests)
	groupBenchCatalog(b, c.providers, c.models)
	body := largeConversation(provider.Anthropic, c.model, c.words)
	if c.stream {
		body = bytes.Replace(body, []byte(`"stream":false`), []byte(`"stream":true`), 1)
	}
	b.SetBytes(int64(len(body)))
	h := New().Handler()
	do := func() {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", bytes.NewReader(body)))
		if rec.Code != 200 || !bytes.Contains(rec.Body.Bytes(), []byte("ok")) {
			b.Fatalf("%d %s", rec.Code, rec.Body.String())
		}
	}
	do() // the resting member rests
	return do
}

// BenchmarkGroupOverhead is a request to a provider's model and the same
// request to routing groups, streamed and not, with a long conversation:
// what magpie adds for a group is the difference. cpu-ms/op is the CPU
// the process spent, which a busy machine doesn't change as it does the
// wall time.
func BenchmarkGroupOverhead(b *testing.B) {
	for _, c := range groupBenchCases() {
		b.Run(c.name(), func(b *testing.B) {
			do := groupBenchServer(b, c)
			b.ResetTimer()
			cpu, _ := processCPU()
			for range b.N {
				do()
			}
			after, _ := processCPU()
			b.ReportMetric(float64(after-cpu)/float64(b.N)/1e6, "cpu-ms/op")
		})
	}
}

// BenchmarkGroupOverheadParallel is BenchmarkGroupOverhead's requests sent
// at once, as several agents send them.
func BenchmarkGroupOverheadParallel(b *testing.B) {
	for _, c := range groupBenchCases() {
		if c.stream || c.words > 50_000 {
			continue
		}
		b.Run(c.name(), func(b *testing.B) {
			do := groupBenchServer(b, c)
			b.ResetTimer()
			cpu, _ := processCPU()
			b.RunParallel(func(pb *testing.PB) {
				for pb.Next() {
					do()
				}
			})
			after, _ := processCPU()
			b.ReportMetric(float64(after-cpu)/float64(b.N)/1e6, "cpu-ms/op")
		})
	}
}
