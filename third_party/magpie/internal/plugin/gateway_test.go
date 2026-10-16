package plugin

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func gatewayFixture(t *testing.T, source string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"magpie":{"hooks":"./request.js"}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "request.js"), []byte(source), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Add(t.Context(), dir); err != nil {
		t.Fatal(err)
	}
	return dir
}

func TestGatewayHookWaitsForStartingHost(t *testing.T) {
	sandbox(t)
	gatewayFixture(t, `export function onRequest(body){return {...body,note:"recalled"}}`)
	// Provider discovery holds this lock while starting the shared host.
	hostMu.Lock()
	locked := true
	defer func() {
		if locked {
			hostMu.Unlock()
		}
	}()
	ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
	defer cancel()
	done := make(chan *GatewayRun, 1)
	go func() { done <- GatewayBegin(ctx, []byte(`{"note":"original"}`), GatewayContext{}) }()
	select {
	case run := <-done:
		if run != nil {
			run.Close()
		}
		t.Fatal("first request skipped the starting host")
	case <-time.After(50 * time.Millisecond):
	}
	hostMu.Unlock()
	locked = false
	select {
	case run := <-done:
		if run == nil {
			t.Fatal("first request did not run its recall hook")
		}
		defer run.Close()
		if !strings.Contains(string(run.Body), `"recalled"`) {
			t.Fatalf("first request was not recalled: %s", run.Body)
		}
	case <-ctx.Done():
		t.Fatal("first request did not resume after host startup")
	}
}

func TestGatewayHookHostWaitCancellation(t *testing.T) {
	sandbox(t)
	gatewayFixture(t, `export function onRequest(body){return {...body,note:"recalled"}}`)
	hostMu.Lock()
	defer hostMu.Unlock()
	ctx, cancel := context.WithTimeout(t.Context(), 50*time.Millisecond)
	defer cancel()
	body := []byte(`{"note":"original"}`)
	if got := gatewayRequest(ctx, body, GatewayContext{}); string(got) != string(body) {
		t.Fatalf("cancelled startup changed the original request: %s", got)
	}
	if ctx.Err() == nil {
		t.Fatal("request skipped host startup instead of waiting for its deadline")
	}
}

func TestGatewayRunStateOwnershipAndAsyncResponse(t *testing.T) {
	sandbox(t)
	started, release := make(chan struct{}, 2), make(chan struct{}, 2)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		started <- struct{}{}
		select {
		case <-release:
		case <-r.Context().Done():
		}
	}))
	defer func() { close(release); server.Close() }()
	output := filepath.Join(t.TempDir(), "responses.jsonl")
	fixture := gatewayFixture(t, `import fs from "node:fs";
export function onRequest(body,ctx) {
  ctx.state.original=body;ctx.state.value=()=>ctx.session;
  return {...body,note:"recalled"};
}
export async function onResponse(reply,ctx) {
  await fetch(ctx.options.url,{signal:ctx.signal});
  fs.appendFileSync(ctx.options.output,JSON.stringify({reply,request:ctx.request,success:ctx.success,session:ctx.state.value(),original:ctx.state.original})+"\n");
}`)
	if err := SetOptions(fixture, map[string]any{"output": output, "url": server.URL, "gatewayTimeoutMs": 5000}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	first := GatewayBegin(ctx, []byte(`{"note":"original"}`), GatewayContext{Session: "first"})
	second := GatewayBegin(t.Context(), []byte(`{"note":"original"}`), GatewayContext{Session: "second"})
	if first == nil || second == nil || !strings.Contains(string(first.Body), `"recalled"`) {
		t.Fatal("request lifecycle did not begin")
	}
	t.Cleanup(first.Close)
	t.Cleanup(second.Close)
	first.Finish(ctx, json.RawMessage(`{"answer":"first"}`), true, first.Body)
	cancel() // Completed notifications outlive the HTTP request context.
	first.Finish(t.Context(), json.RawMessage(`{"duplicate":true}`), true)
	second.Finish(t.Context(), json.RawMessage(`{"answer":"second"}`), true, second.Body)
	for range 2 {
		select {
		case <-started:
		case <-time.After(10 * time.Second):
			t.Fatal("Finish blocked or notification was cancelled with client request")
		}
	}
	release <- struct{}{}
	release <- struct{}{}
	deadline := time.Now().Add(10 * time.Second)
	for {
		b, _ := os.ReadFile(output)
		if strings.Count(string(b), "\n") == 2 {
			if strings.Contains(string(b), "duplicate") || !strings.Contains(string(b), `"session":"first"`) || !strings.Contains(string(b), `"session":"second"`) || !strings.Contains(string(b), `"note":"original"`) || !strings.Contains(string(b), `"success":true`) {
				t.Fatalf("completion state crossed requests: %s", b)
			}
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("notifications did not finish: %s", b)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestGatewayHookCannotRegisterTools(t *testing.T) {
	for _, source := range []string{
		`export function onRequest(body,ctx){ctx.addTools([]);return {...body,note:"corrupt"}}`,
		`export function onRequest(body){return {...body,tools:[{type:"function",function:{name:"recall"}}]}}`,
	} {
		t.Run(source, func(t *testing.T) {
			sandbox(t)
			gatewayFixture(t, source)
			body := []byte(`{"note":"original","tools":[]}`)
			if out := gatewayRequest(t.Context(), body, GatewayContext{}); !strings.Contains(string(out), `"original"`) || strings.Contains(string(out), "recall") {
				t.Fatalf("context hook changed client tools: %s", out)
			}
		})
	}
}

// The gateway checked the caller's key against the model and answers in
// the streaming the client asked for: a hook that changes either is rolled
// back, while one that adds context alone is kept.
func TestGatewayHookCannotChangeModelOrStream(t *testing.T) {
	body := []byte(`{"model":"original","stream":true,"messages":[]}`)
	for _, tc := range []struct{ source, want string }{
		{`export function onRequest(body){return {...body,model:"other"}}`, `{"model":"original","stream":true,"messages":[]}`},
		{`export function onRequest(body){return {...body,stream:false}}`, `{"model":"original","stream":true,"messages":[]}`},
		{`export function onRequest(body){const {stream, ...rest} = body; return rest}`, `{"model":"original","stream":true,"messages":[]}`},
		{`export function onRequest(body){return {...body,messages:[{role:"system",content:"recalled"}]}}`, `{"model":"original","stream":true,"messages":[{"role":"system","content":"recalled"}]}`},
	} {
		t.Run(tc.source, func(t *testing.T) {
			sandbox(t)
			gatewayFixture(t, tc.source)
			var got, want any
			json.Unmarshal(gatewayRequest(t.Context(), body, GatewayContext{}), &got)
			json.Unmarshal([]byte(tc.want), &want)
			if !reflect.DeepEqual(got, want) {
				t.Fatalf("got %v, want %s", got, tc.want)
			}
		})
	}
}

func TestGatewayRunCapacityAndRelease(t *testing.T) {
	sandbox(t)
	gatewayFixture(t, `export function onRequest(body){return body}`)
	runs := make([]*GatewayRun, 0, 128)
	t.Cleanup(func() {
		for _, run := range runs {
			run.Close()
		}
	})
	for range 128 {
		run := GatewayBegin(t.Context(), []byte(`{}`), GatewayContext{})
		if run == nil {
			t.Fatalf("run capacity exhausted at %d", len(runs))
		}
		runs = append(runs, run)
	}
	if run := GatewayBegin(t.Context(), []byte(`{}`), GatewayContext{}); run != nil {
		run.Close()
		t.Fatal("host retained more than its bounded request capacity")
	}
	runs[0].Close()
	run := GatewayBegin(t.Context(), []byte(`{}`), GatewayContext{})
	if run == nil {
		t.Fatal("closed run did not release capacity")
	}
	defer run.Close()
}

func TestGatewayHookChainRollbackAndOptions(t *testing.T) {
	sandbox(t)
	first := gatewayFixture(t, `export async function onRequest(body, ctx) { await Promise.resolve(); body.note = ctx.options.note; return body }`)
	if err := SetOptions(first, map[string]any{"note": "first"}); err != nil {
		t.Fatal(err)
	}
	gatewayFixture(t, `export async function onRequest(body) { body.note = "corrupt"; throw new Error("private token must not be logged") }`)
	gatewayFixture(t, `export async function onRequest(body) { body.note += "-last"; return body }`)
	loaded, err := Plugins(t.Context())
	if err != nil || len(loaded) != 3 {
		t.Fatalf("loaded: %+v %v", loaded, err)
	}
	for _, p := range loaded {
		if p.Error != "" {
			t.Fatalf("load: %+v", p)
		}
	}
	out := gatewayRequest(t.Context(), []byte(`{"note":"original","messages":[]}`), GatewayContext{Protocol: "chat"})
	var got struct{ Note string }
	if err := json.Unmarshal(out, &got); err != nil || got.Note != "first-last" {
		t.Fatalf("chain did not preserve successful hooks and undo failed mutation: %s (%v)", out, err)
	}
	if err := SetOff(first, true); err != nil {
		t.Fatal(err)
	}
	out = gatewayRequest(t.Context(), []byte(`{"note":"original"}`), GatewayContext{})
	if !strings.Contains(string(out), "original-last") {
		t.Fatalf("disabled hook ran: %s", out)
	}
}

func TestGatewayHookMixedProviderPackage(t *testing.T) {
	sandbox(t)
	dir := gatewayFixture(t, `export function onRequest(body) { body.note="recalled";return body }`)
	if err := os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"type":"module","main":"./provider.js","magpie":{"hooks":"./request.js"}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "provider.js"), []byte(`export default async function(){return {auth:{provider:"fixture",methods:[]},config(config){config.provider.fixture={name:"Gateway fixture",models:{m1:{name:"m1"}}}}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	providers, err := Providers(t.Context())
	if err != nil || len(providers) != 1 || providers[0].ID != "fixture" {
		t.Fatalf("mixed package lost its provider: %+v %v", providers, err)
	}
	out := gatewayRequest(t.Context(), []byte(`{"note":"original"}`), GatewayContext{})
	if !strings.Contains(string(out), "recalled") {
		t.Fatalf("mixed package lost its gateway hook: %s", out)
	}
}

func TestGatewayHookMixedPiPackage(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(t.Context(), 4*time.Minute)
	defer cancel()
	installPinnedPi(t, ctx)
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "extensions"), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, file := range []string{"package.json", "extensions/fakepi.ts"} {
		b, err := os.ReadFile(filepath.Join("testdata", "pi", file))
		if err != nil {
			t.Fatal(err)
		}
		if file == "package.json" {
			var pkg map[string]any
			if err := json.Unmarshal(b, &pkg); err != nil {
				t.Fatal(err)
			}
			pkg["magpie"] = map[string]string{"hooks": "./request.js"}
			b = mustJSON(t, pkg)
		}
		if err := os.WriteFile(filepath.Join(dir, filepath.FromSlash(file)), b, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "request.js"), []byte(`export function onRequest(body){body.note="recalled";return body}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Add(ctx, dir); err != nil {
		t.Fatal(err)
	}
	providers, err := Providers(ctx)
	if err != nil || len(providers) != 2 {
		t.Fatalf("mixed package lost Pi providers: %+v %v", providers, err)
	}
	out := gatewayRequest(t.Context(), []byte(`{"note":"original"}`), GatewayContext{})
	if !strings.Contains(string(out), "recalled") {
		t.Fatalf("mixed Pi package lost gateway hook: %s", out)
	}
}

func TestGatewayHookTimeoutCancelsHTTPAndKeepsBody(t *testing.T) {
	sandbox(t)
	started, stopped := make(chan struct{}), make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(started)
		<-r.Context().Done()
		close(stopped)
	}))
	defer srv.Close()
	source := `export async function onRequest(body, ctx) { body.note="corrupt"; await fetch(` + string(mustJSON(t, srv.URL)) + `,{signal:ctx.signal}); return body }`
	gatewayFixture(t, source)
	if _, err := Plugins(t.Context()); err != nil {
		t.Fatal(err)
	}
	original := []byte(`{"note":"original"}`)
	out := gatewayRequest(t.Context(), original, GatewayContext{})
	if string(out) != string(original) {
		t.Fatalf("timeout changed request: %s", out)
	}
	select {
	case <-started:
	default:
		t.Fatal("hook never reached HTTP")
	}
	select {
	case <-stopped:
	case <-time.After(10 * time.Second):
		t.Fatal("timed-out hook did not cancel HTTP")
	}
}

func TestGatewayHookCallerCancellation(t *testing.T) {
	sandbox(t)
	started, stopped := make(chan struct{}), make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(started)
		<-r.Context().Done()
		close(stopped)
	}))
	defer srv.Close()
	gatewayFixture(t, `export async function onRequest(body,ctx) { await fetch(`+string(mustJSON(t, srv.URL))+`,{signal:ctx.signal}); return body }`)
	if _, err := Plugins(t.Context()); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	done := make(chan []byte, 1)
	go func() { done <- gatewayRequest(ctx, []byte(`{"note":"original"}`), GatewayContext{}) }()
	select {
	case <-started:
	case <-time.After(10 * time.Second):
		t.Fatal("hook never started")
	}
	cancel()
	select {
	case out := <-done:
		if string(out) != `{"note":"original"}` {
			t.Fatalf("cancel changed body: %s", out)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("caller cancellation did not return")
	}
	select {
	case <-stopped:
	case <-time.After(10 * time.Second):
		t.Fatal("caller cancellation did not stop host HTTP")
	}
	// The canceled RPC releases its capacity; another RPC can still finish.
	var out any
	if err := Call(t.Context(), "providers", map[string]any{}, &out); err != nil {
		t.Fatal(err)
	}
}

func mustJSON(t *testing.T, v any) []byte {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestGatewayDiagnosticsAndExtendedBudget(t *testing.T) {
	sandbox(t)
	spec := gatewayFixture(t, `export async function onRequest(body,ctx) {
    await new Promise(resolve=>setTimeout(resolve,1100));
    ctx.report("private secret query", 99);
    ctx.report("ready", 12);
    body.note="recalled"; return body
  }`)
	if err := SetOptions(spec, map[string]any{"gatewayTimeoutMs": 2000}); err != nil {
		t.Fatal(err)
	}
	out := gatewayRequest(t.Context(), []byte(`{"note":"original"}`), GatewayContext{Query: "private secret query"})
	if !strings.Contains(string(out), "recalled") {
		t.Fatalf("configured recall budget ignored: %s", out)
	}
	state := GatewayStates(t.Context())[spec]
	if state.Calls != 1 || state.Failures != 0 || state.LastOutcome != "ready" || state.LastBytes != 12 || state.AvgMillis < 1000 {
		t.Fatalf("diagnostics: %+v", state)
	}
	if strings.Contains(string(mustJSON(t, state)), "private") {
		t.Fatal("diagnostics retained private data")
	}
}

// A malformed result and an undefined result both keep original bytes.
func TestGatewayHookInvalidOrUndefinedResult(t *testing.T) {
	for _, result := range []string{"null", "[]", "undefined", "{toJSON(){return null}}"} {
		t.Run(result, func(t *testing.T) {
			sandbox(t)
			gatewayFixture(t, `export function onRequest(body){body.note="corrupt";return `+result+`}`)
			out := gatewayRequest(t.Context(), []byte(`{"note":"original"}`), GatewayContext{})
			if !strings.Contains(string(out), `"original"`) {
				t.Fatalf("invalid return changed request: %s", out)
			}
		})
	}
}

// Bun loads TypeScript itself: a direct *.hooks.ts file and a package
// whose magpie.hooks names a .ts entry both run without a build step.
func TestGatewayHookTypeScript(t *testing.T) {
	sandbox(t)
	direct := filepath.Join(t.TempDir(), "recall.hooks.ts")
	if err := os.WriteFile(direct, []byte(`type Body = { note: string }
export async function onRequest(body: Body): Promise<Body> { body.note += "-direct"; return body }`), 0o644); err != nil {
		t.Fatal(err)
	}
	if Gateway(direct) == "" {
		t.Fatal("direct .hooks.ts file not recognised")
	}
	if _, err := Add(t.Context(), direct); err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"magpie":{"hooks":"./src/request.ts"}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(dir, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "src", "request.ts"), []byte(`interface Body { note: string }
export function onRequest(body: Body): Body { body.note += "-package"; return body }`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Add(t.Context(), dir); err != nil {
		t.Fatal(err)
	}
	out := gatewayRequest(t.Context(), []byte(`{"note":"m"}`), GatewayContext{Protocol: "chat"})
	if !strings.Contains(string(out), `"m-direct-package"`) {
		t.Fatalf("TypeScript hooks did not run in order: %s", out)
	}
}

// gatewayRequest is a hook run on body alone: the body it leaves.
func gatewayRequest(ctx context.Context, body []byte, info GatewayContext) []byte {
	run := GatewayBegin(ctx, body, info)
	if run == nil {
		return body
	}
	defer run.Close()
	return run.Body
}

// A JS gateway middleware (*.gateway.js, magpie.gateway) and a request hook
// (*.hooks.js, magpie.hooks) never claim the same entry: a hook importing
// node:fs loaded as middleware too would show "didn't load" for a plugin
// that works.
func TestGatewayHookNamesApartFromMiddleware(t *testing.T) {
	files := map[string]bool{"a.hooks.js": true, "a.hooks.mjs": true, "a.hooks.ts": true, "a.hooks.mts": true, "a.gateway.js": false, "a.gateway.mjs": false, "a.middleware.js": false}
	for name, hook := range files {
		p := filepath.Join(t.TempDir(), name)
		if err := os.WriteFile(p, []byte(`export async function onRequest(b) { return b }`), 0o644); err != nil {
			t.Fatal(err)
		}
		if got := Gateway(p) != ""; got != hook {
			t.Errorf("%s: hook %v, want %v", name, got, hook)
		}
		if f, _ := Middleware(p); hook && f != "" {
			t.Errorf("%s is loaded as middleware too", name)
		}
	}
	for key, hook := range map[string]bool{"hooks": true, "gateway": false, "middleware": false} {
		dir := t.TempDir()
		if err := os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"magpie":{"`+key+`":"./entry.js"}}`), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "entry.js"), []byte(`export async function onRequest(b) { return b }`), 0o644); err != nil {
			t.Fatal(err)
		}
		if got := Gateway(dir) != ""; got != hook {
			t.Errorf("magpie.%s: hook %v, want %v", key, got, hook)
		}
		if f, _ := Middleware(dir); hook && f != "" {
			t.Errorf("magpie.%s is loaded as middleware too", key)
		}
	}
}
