package middleware

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// use makes plugins.json list entries, and the middleware load afresh.
func use(t *testing.T, entries ...plugin.Entry) {
	t.Helper()
	if err := os.MkdirAll(settings.Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(plugin.List{Plugins: entries})
	if err := os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
	Reload()
	t.Cleanup(func() {
		os.Remove(filepath.Join(settings.Dir(), "plugins.json"))
		Reload()
	})
}

// file writes a middleware file of its own and gives its entry.
func file(t *testing.T, src string) plugin.Entry {
	t.Helper()
	p := filepath.Join(t.TempDir(), "x.middleware.js")
	if err := os.WriteFile(p, []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	return plugin.Entry{Spec: p}
}

func info() Info { return Info{Protocol: "anthropic", Model: "m1", Stream: true, Path: "/v1/messages"} }

func TestNoMiddlewareIsNil(t *testing.T) {
	use(t)
	if r := Begin(info()); r != nil {
		t.Fatal("Begin with no middleware isn't nil")
	}
	// a nil Run passes everything as it is
	var r *Run
	body, rej := r.Request([]byte(`{"a":1}`))
	if string(body) != `{"a":1}` || rej != nil {
		t.Fatal(string(body), rej)
	}
	w := httptest.NewRecorder()
	if got, _ := r.Wrap(w); got != w {
		t.Fatal("a nil Run wrapped the writer")
	}
}

func TestRequestChain(t *testing.T) {
	// a folder package whose middleware imports a file beside it, and a
	// plugin that is only middleware
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"name":"alias","magpie":{"middleware":"./mw/index.js"}}`), 0o644)
	os.MkdirAll(filepath.Join(dir, "mw"), 0o755)
	os.WriteFile(filepath.Join(dir, "mw", "names.js"), []byte(`export const names = {"m1": "m2"};`), 0o644)
	os.WriteFile(filepath.Join(dir, "mw", "index.js"), []byte(`
import { names } from "./names.js";
export default {
  onRequest(body, ctx) {
    if (names[body.model]) body.model = names[body.model];
    body.seen = [ctx.protocol, ctx.path, ctx.options.tag];
    return body;
  },
};`), 0o644)
	if f, only := plugin.Middleware(dir); f != filepath.Join(dir, "mw", "index.js") || !only {
		t.Fatalf("Middleware(dir) = %q, %v", f, only)
	}
	second := file(t, `export function onRequest(body) { body.model += "!"; return body; }`)
	untouched := file(t, `export function onRequest(body) { body.ignored = true; }`)
	use(t, plugin.Entry{Spec: dir, Options: map[string]any{"tag": "t"}}, second, untouched)

	r := Begin(info())
	defer r.End()
	body, rej := r.Request([]byte(`{"model":"m1"}`))
	if rej != nil {
		t.Fatal(rej)
	}
	var got map[string]any
	json.Unmarshal(body, &got)
	if got["model"] != "m2!" || got["ignored"] != nil {
		t.Fatalf("body %s", body)
	}
	if s, _ := json.Marshal(got["seen"]); string(s) != `["anthropic","/v1/messages","t"]` {
		t.Fatalf("seen %s", s)
	}
}

func TestUndefinedKeepsBytes(t *testing.T) {
	use(t, file(t, `export function onRequest(body) {}`))
	in := `{"b": 1,   "a" : [1,2]}`
	r := Begin(info())
	defer r.End()
	if body, _ := r.Request([]byte(in)); string(body) != in {
		t.Fatalf("got %s", body)
	}
}

func TestReject(t *testing.T) {
	use(t, file(t, `export function onRequest(body, ctx) { if (body.model === "no") ctx.reject(429, "slow down"); }`),
		file(t, `export function onRequest(body) { throw new Error("never called"); }`))
	r := Begin(info())
	defer r.End()
	_, rej := r.Request([]byte(`{"model":"no"}`))
	if rej == nil || rej.Status != 429 || rej.Message != "slow down" {
		t.Fatalf("reject %+v", rej)
	}
	for spec, st := range States() {
		if st.Failures != 0 {
			t.Fatalf("%s failed after a reject: %+v", spec, st)
		}
	}
}

func TestFailOpen(t *testing.T) {
	was := requestLimit
	defer func() { requestLimit = was }()
	e := file(t, `
let n = 0;
export function onRequest(body) {
  n++;
  if (body.kind === "throw") throw new TypeError("bad " + n);
  if (body.kind === "spin") for (;;) {}
  if (body.kind === "string") return "x";
  body.ok = n;
  return body;
}`)
	use(t, e)
	for _, kind := range []string{"throw", "spin", "string"} {
		in := `{"kind":"` + kind + `"}`
		// Only the spin is cut short: under a full run's load a throw or a
		// string can take longer than 50ms, and would fail for the limit.
		requestLimit = was
		if kind == "spin" {
			requestLimit = 50 * time.Millisecond
		}
		r := Begin(info())
		start := time.Now()
		body, rej := r.Request([]byte(in))
		r.End()
		if string(body) != in || rej != nil {
			t.Fatalf("%s: got %s %v", kind, body, rej)
		}
		if time.Since(start) > time.Second {
			t.Fatalf("%s took %v", kind, time.Since(start))
		}
	}
	st := States()[e.Spec]
	if st.Failures != 3 || !strings.Contains(st.Last, "returned neither") {
		t.Fatalf("state %+v", st)
	}
	// the runtime that was stopped works again
	r := Begin(info())
	defer r.End()
	body, _ := r.Request([]byte(`{"kind":"fine"}`))
	if !strings.Contains(string(body), `"ok"`) {
		t.Fatalf("after failures: %s", body)
	}
}

func TestOnProvider(t *testing.T) {
	e := file(t, `
export function onProvider(body, ctx) {
  const p = ctx.provider;
  if (p.account === "antigravity") body.system = body.system.replace("SDK", "S\u200bDK");
  if (p.id === "throw") throw new Error("bad");
  if (p.id === "null") return null;
  if (p.id === "reject") ctx.reject(429, "no");
  body.to = p.id + "/" + p.model;
  return body;
}`)
	use(t, e)
	r := Begin(info())
	if r == nil {
		t.Fatal("a middleware with only onProvider began no run")
	}
	defer r.End()
	in := `{"system":"Claude Agent SDK"}`
	// onRequest isn't exported, so the agent's body goes on as it is
	if body, rej := r.Request([]byte(in)); string(body) != in || rej != nil {
		t.Fatalf("request %s %v", body, rej)
	}
	for _, c := range []struct {
		to   gwmw.To
		want string
	}{
		{gwmw.To{ID: "ag", Account: "antigravity", Model: "gemini-3-flash"}, `{"system":"Claude Agent S\u200bDK","to":"ag/gemini-3-flash"}`},
		// the next try starts from the agent's body, not the last try's
		{gwmw.To{ID: "k", Model: "m1"}, `{"system":"Claude Agent SDK","to":"k/m1"}`},
		{gwmw.To{ID: "throw"}, in},
		{gwmw.To{ID: "null"}, in},
		// a reject has nothing to turn away once the request is routed
		{gwmw.To{ID: "reject", Model: "m"}, `{"system":"Claude Agent SDK","to":"reject/m"}`},
	} {
		var got, want any
		json.Unmarshal(r.Tried([]byte(in), c.to), &got)
		json.Unmarshal([]byte(c.want), &want)
		if g, _ := json.Marshal(got); string(g) != func() string { w, _ := json.Marshal(want); return string(w) }() {
			t.Fatalf("%s: got %s, want %s", c.to.ID, g, c.want)
		}
	}
	st := States()[e.Spec]
	if st.Failures != 2 || !slices.Contains(st.Hooks, "onProvider") || !strings.Contains(st.Last, "null") {
		t.Fatalf("state %+v", st)
	}
	var none *Run
	if body := none.Tried([]byte(in), gwmw.To{}); string(body) != in {
		t.Fatalf("a nil Run changed %s", body)
	}
}

func TestAsyncHook(t *testing.T) {
	use(t, file(t, `
export async function onRequest(body) {
  const v = await Promise.resolve(7);
  body.v = v;
  return body;
}
export async function onResponse(body) { throw new Error("async no"); }`))
	r := Begin(info())
	defer r.End()
	body, _ := r.Request([]byte(`{}`))
	if string(body) != `{"v":7}` {
		t.Fatalf("got %s", body)
	}
}

func TestLoadErrors(t *testing.T) {
	none := file(t, `export const x = 1;`)
	bad := file(t, `export function onRequest( {`)
	outside := file(t, `import x from "lodash"; export function onRequest() {}`)
	use(t, none, bad, outside)
	st := States()
	for _, e := range []plugin.Entry{none, bad, outside} {
		if st[e.Spec].Error == "" {
			t.Fatalf("%s loaded: %+v", e.Spec, st[e.Spec])
		}
	}
	if !strings.Contains(st[outside.Spec].Error, "bundle") {
		t.Fatal(st[outside.Spec].Error)
	}
	if Begin(info()) != nil {
		t.Fatal("middleware that didn't load ran")
	}
}

func TestOffAndHostSkip(t *testing.T) {
	e := file(t, `export function onRequest(b) { b.x = 1; return b; }`)
	e.Off = true
	use(t, e)
	if Begin(info()) != nil {
		t.Fatal("an Off middleware ran")
	}
	// a package with a main is an OpenCode plugin too
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"main":"index.js","magpie":{"middleware":"mw.js"}}`), 0o644)
	if f, only := plugin.Middleware(dir); f == "" || only {
		t.Fatalf("Middleware = %q, %v", f, only)
	}
	if f, _ := plugin.Middleware(filepath.Join(dir, "package.json")); f != "" {
		t.Fatal("a file not named .middleware.js is middleware")
	}
}

func TestReloadOnChange(t *testing.T) {
	e := file(t, `export function onRequest(b) { b.v = 1; return b; }`)
	use(t, e)
	run := func() string {
		r := Begin(info())
		defer r.End()
		b, _ := r.Request([]byte(`{}`))
		return string(b)
	}
	if got := run(); got != `{"v":1}` {
		t.Fatal(got)
	}
	os.WriteFile(e.Spec, []byte(`export function onRequest(b) { b.v = "two"; return b; }`), 0o644)
	nextCheck.Store(0)
	if got := run(); got != `{"v":"two"}` {
		t.Fatal("not reloaded:", got)
	}
}

// sse is a stream as Anthropic's API sends it.
const sse = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n" +
	"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"secret here\"}}\n\n" +
	"event: ping\ndata: {\"type\":\"ping\"}\n\n" +
	"event: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"more\"}}\r\n\r\n" +
	"data: [DONE]\n\n"

func TestStream(t *testing.T) {
	e := file(t, `
export const events = ["content_block_delta", "ping"];
export function onRequest(body, ctx) { ctx.state.word = body.word; }
export function onEvent(ev, ctx) {
  if (ev.type === "ping") return null;
  ev.delta.text = ev.delta.text.replace(ctx.state.word, "******");
  console.log("event", ev);
  return ev;
}`)
	use(t, e)
	r := Begin(info())
	defer r.End()
	r.Request([]byte(`{"word":"secret"}`))
	rec := httptest.NewRecorder()
	w, done := r.Wrap(rec)
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Content-Length", "999")
	w.WriteHeader(200)
	// written in pieces that split events and lines
	for i := 0; i < len(sse); i += 7 {
		w.Write([]byte(sse[i:min(i+7, len(sse))]))
		w.(http.Flusher).Flush()
	}
	done()
	want := "event: message_start\ndata: {\"type\":\"message_start\"}\n\n" +
		"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"****** here\"}}\n\n" +
		"event: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"more\"}}\r\n\r\n" +
		"data: [DONE]\n\n"
	if got := rec.Body.String(); got != want {
		t.Fatalf("got\n%q\nwant\n%q", got, want)
	}
	if rec.Header().Get("Content-Length") != "" {
		t.Fatal("Content-Length kept")
	}
	if st := States()[e.Spec]; st.Failures != 0 || st.Calls != 4 {
		t.Fatalf("state %+v", st)
	}
}

func TestStreamUnchangedPassesBytes(t *testing.T) {
	use(t, file(t, `export function onEvent(ev) {}`))
	r := Begin(info())
	defer r.End()
	rec := httptest.NewRecorder()
	w, done := r.Wrap(rec)
	w.Header().Set("Content-Type", "text/event-stream")
	io := "data: {\"a\" :  1}\n\ndata: {\"b\":2}\n\n: comment\n\ndata: [DONE]\n\npartial"
	w.Write([]byte(io))
	done()
	if rec.Body.String() != io {
		t.Fatalf("got %q", rec.Body.String())
	}
	// [DONE] never reached the hook as JSON it couldn't parse
	for _, st := range States() {
		if st.Failures != 0 || st.Calls != 2 {
			t.Fatalf("state %+v", st)
		}
	}
}

// A keepalive comment goes to the agent as it is written, not with the
// event after it: held, a proxy in front of magpie heard nothing while the
// vendor reasoned silently and hung up (#2402).
func TestStreamKeepaliveNotHeld(t *testing.T) {
	use(t, file(t, `export function onEvent(ev) { ev.seen = true; return ev; }`))
	r := Begin(info())
	defer r.End()
	rec := httptest.NewRecorder()
	w, done := r.Wrap(rec)
	w.Header().Set("Content-Type", "text/event-stream")
	w.WriteHeader(200)
	for i, s := range []string{": keepalive\n", ": keepalive\r\n", "\n", "data: {\"a\":1}\n", ": keep"} {
		w.Write([]byte(s))
		if want := []int{1, 2, 2, 2, 2}[i]; strings.Count(rec.Body.String(), ": keepalive") != want {
			t.Fatalf("after %q: %q", s, rec.Body.String())
		}
	}
	w.Write([]byte("alive\n\n"))
	done()
	// a comment inside an event stays in it, the event changed around it
	want := ": keepalive\n: keepalive\r\n\n" + `data: {"a":1,"seen":true}` + "\n: keepalive\n\n"
	if got := rec.Body.String(); got != want {
		t.Fatalf("got %q, want %q", got, want)
	}
}

func TestWholeResponse(t *testing.T) {
	use(t, file(t, `
export function onResponse(body, ctx) { body.status = ctx.status; body.n = (body.n || 0) + 1; return body; }`))
	r := Begin(info())
	defer r.End()
	rec := httptest.NewRecorder()
	w, done := r.Wrap(rec)
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Content-Length", "8")
	w.WriteHeader(201)
	w.Write([]byte(`{"n":`))
	w.Write([]byte(`41}`))
	done()
	if got := rec.Body.String(); got != `{"n":42,"status":201}` {
		t.Fatal(got)
	}
	if rec.Header().Get("Content-Length") != "" {
		t.Fatal("Content-Length kept")
	}
	// a gzipped reply goes as it is
	r2 := Begin(info())
	defer r2.End()
	rec = httptest.NewRecorder()
	w, done = r2.Wrap(rec)
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Content-Encoding", "gzip")
	w.Write([]byte("\x1f\x8b..."))
	done()
	if rec.Body.String() != "\x1f\x8b..." {
		t.Fatal("gzip body changed")
	}
}

func TestParallel(t *testing.T) {
	use(t, file(t, `
export function onRequest(b, ctx) { ctx.state.id = b.id; }
export function onEvent(ev, ctx) { ev.id = ctx.state.id; return ev; }`))
	var wg sync.WaitGroup
	errs := make(chan string, 64)
	for i := range 64 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			r := Begin(info())
			defer r.End()
			r.Request([]byte(`{"id":` + itoa(i) + `}`))
			rec := httptest.NewRecorder()
			w, done := r.Wrap(rec)
			w.Header().Set("Content-Type", "text/event-stream")
			for range 20 {
				w.Write([]byte("data: {}\n\n"))
			}
			done()
			if want := strings.Repeat(`data: {"id":`+itoa(i)+"}\n\n", 20); rec.Body.String() != want {
				errs <- rec.Body.String()
			}
		}()
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		t.Fatal("crossed requests:", e)
	}
}

func itoa(i int) string { b, _ := json.Marshal(i); return string(b) }

// BenchmarkEvent is what one event costs with a middleware that changes it.
func BenchmarkEvent(b *testing.B) {
	dir := b.TempDir()
	p := filepath.Join(dir, "x.middleware.js")
	os.WriteFile(p, []byte(`export function onEvent(ev) { if (ev.delta) ev.delta.text = ev.delta.text.toUpperCase(); return ev; }`), 0o644)
	os.MkdirAll(settings.Dir(), 0o755)
	l, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: p}}})
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), l, 0o644)
	Reload()
	defer func() { os.Remove(filepath.Join(settings.Dir(), "plugins.json")); Reload() }()
	ev := []byte("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello there, how are you\"}}\n\n")
	r := Begin(info())
	defer r.End()
	w, done := r.Wrap(discard{})
	w.Header().Set("Content-Type", "text/event-stream")
	b.ReportAllocs()
	b.ResetTimer()
	for range b.N {
		w.Write(ev)
	}
	done()
}

type discard struct{}

func (discard) Header() http.Header         { return http.Header{"Content-Type": {"text/event-stream"}} }
func (discard) Write(p []byte) (int, error) { return len(p), nil }
func (discard) WriteHeader(int)             {}

// A request whose reply is streaming holds its runtimes, and they hold
// neither its body as the hooks were given it nor what they made of it:
// a long context's body was kept alive until the reply ended, ~10 MB of a
// 7 MB body a request, and with many agents' requests streaming at once,
// hundreds of MB (#3383). What a hook put in ctx.state stays.
func TestStreamingRequestHoldsNoBody(t *testing.T) {
	use(t, file(t, `export function onRequest(body, ctx) { ctx.state.n = body.messages.length; }
export function onProvider(body, ctx) { ctx.state.p = ctx.provider.id; }
export function onEvent(ev, ctx) { ev.n = ctx.state.n; return ev; }`))
	body := []byte(`{"model":"m1","messages":[` + strings.Repeat(`{"role":"user","content":"`+strings.Repeat("x", 1000)+`"},`, 8000) + `{"role":"user","content":"y"}]}`)
	heap := func() int64 {
		runtime.GC()
		var m runtime.MemStats
		runtime.ReadMemStats(&m)
		return int64(m.HeapAlloc)
	}
	before := heap()
	runs := make([]*Run, 8)
	for i := range runs {
		runs[i] = Begin(info())
		runs[i].Request(body)
		runs[i].Tried(body, gwmw.To{ID: "p1", Model: "m1"})
	}
	held := heap() - before
	for _, r := range runs {
		rec := httptest.NewRecorder()
		w, done := r.Wrap(rec)
		w.Header().Set("Content-Type", "text/event-stream")
		w.Write([]byte("data: {\"a\":1}\n\n"))
		done()
		if !strings.Contains(rec.Body.String(), `"n":8001`) {
			t.Errorf("ctx.state was lost: %s", rec.Body)
		}
		r.End()
	}
	if held > int64(len(body)) {
		t.Fatalf("8 streaming requests hold %d MB, their body is %d MB", held>>20, len(body)>>20)
	}
}
