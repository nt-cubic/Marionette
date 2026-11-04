package gateway

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/yetone/magpie/internal/browserext"
)

func browserPost(h http.Handler, addr, body string, hdr ...string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(http.MethodPost, BrowserMCPPath, strings.NewReader(body))
	r.RemoteAddr = addr
	r.Host = "127.0.0.1:3425"
	r.Header.Set("Content-Type", "application/json")
	r.Header.Set("Accept", "application/json, text/event-stream")
	for i := 0; i+1 < len(hdr); i += 2 {
		r.Header.Set(hdr[i], hdr[i+1])
	}
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

// fakeExtension connects to the gateway at base as Magpie Browser does,
// and answers each call with answer.
func fakeExtension(t *testing.T, base string, answer func(id, session, tool string, args map[string]any) any) {
	t.Helper()
	fakeExtensionAs(t, base, "Chrome/140", answer)
}

// fakeExtensionAs is fakeExtension in the browser whose User-Agent is agent.
func fakeExtensionAs(t *testing.T, base, agent string, answer func(id, session, tool string, args map[string]any) any) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	c, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(base, "http")+BrowserExtensionPath,
		&websocket.DialOptions{HTTPHeader: http.Header{"Origin": {browserext.Origin}}})
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	done := make(chan struct{})
	t.Cleanup(func() { cancel(); c.CloseNow(); <-done })
	if err := c.Write(ctx, websocket.MessageText, []byte(`{"type":"hello","version":"0.1.0","agent":"`+agent+`"}`)); err != nil {
		t.Fatal(err)
	}
	go func() {
		defer close(done)
		for {
			_, b, err := c.Read(ctx)
			if err != nil {
				return
			}
			var m struct {
				ID, Session, Tool string
				Args              map[string]any
			}
			if json.Unmarshal(b, &m) != nil || m.ID == "" {
				continue
			}
			out, _ := json.Marshal(map[string]any{"id": m.ID, "result": answer(m.ID, m.Session, m.Tool, m.Args)})
			c.Write(ctx, websocket.MessageText, out)
		}
	}()
}

// browserConnected waits until the gateway has the extension, as it adds
// it on its hello.
func browserConnected(t *testing.T, s *Server) {
	t.Helper()
	for deadline := time.Now().Add(10 * time.Second); s.browser.current() == nil; {
		if time.Now().After(deadline) {
			t.Fatal("the extension never connected")
		}
		time.Sleep(5 * time.Millisecond)
	}
}

// An agent's call goes to the extension with its session, and the
// extension's MCP content comes back as the tool's result.
func TestBrowserMCPRelaysToTheExtension(t *testing.T) {
	fresh(t)
	s := New()
	srv := httptest.NewServer(lanGuard(s.Handler()))
	t.Cleanup(srv.Close)
	type got struct {
		session, tool string
		args          map[string]any
	}
	calls := make(chan got, 4)
	fakeExtension(t, srv.URL, func(_, session, tool string, args map[string]any) any {
		calls <- got{session, tool, args}
		return map[string]any{"content": []any{map[string]any{"type": "text", "text": "Page: Example"}}}
	})
	browserConnected(t, s)

	body := imageToolCall("browser_navigate", map[string]any{"url": "example.com"})
	req, _ := http.NewRequest(http.MethodPost, srv.URL+BrowserMCPPath, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	req.Header.Set(BrowserSessionHeader, "s1")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	var m imageReply
	if resp.StatusCode != 200 || json.Unmarshal(b, &m) != nil || m.Result.IsError || len(m.Result.Content) != 1 || m.Result.Content[0].Text != "Page: Example" {
		t.Fatalf("%d %s", resp.StatusCode, b)
	}
	c := <-calls
	if c.session != "s1" || c.tool != "browser_navigate" || c.args["url"] != "example.com" {
		t.Fatalf("the extension was handed %+v", c)
	}

	st, err := http.Get(srv.URL + BrowserStatusPath)
	if err != nil {
		t.Fatal(err)
	}
	var status struct {
		Connected          bool
		Extension, Browser string
	}
	json.NewDecoder(st.Body).Decode(&status)
	st.Body.Close()
	if !status.Connected || status.Extension != "0.1.0" || status.Browser != "Chrome/140" {
		t.Fatalf("status: %+v", status)
	}
}

// With no extension connected, the tools are still listed, and a call says
// how to set it up instead of waiting.
func TestBrowserMCPSaysWhenNoBrowser(t *testing.T) {
	fresh(t)
	h := lanGuard(New().Handler())
	w := browserPost(h, "127.0.0.1:5000", `{"jsonrpc":"2.0","id":1,"method":"tools/list"}`)
	var list struct {
		Result struct{ Tools []struct{ Name string } }
	}
	if w.Code != 200 || json.Unmarshal(w.Body.Bytes(), &list) != nil || len(list.Result.Tools) != len(browserTools()) {
		t.Fatalf("tools/list: %d %s", w.Code, w.Body.String())
	}
	m := readImageReply(t, browserPost(h, "127.0.0.1:5000", imageToolCall("browser_snapshot", nil)))
	if !m.Result.IsError || !strings.Contains(m.Result.Content[0].Text, "Magpie Browser isn't connected") {
		t.Fatalf("no browser: %+v", m)
	}
	w = browserPost(h, "127.0.0.1:5000", imageToolCall("browser_format_disk", nil))
	if !strings.Contains(w.Body.String(), "unknown tool") {
		t.Fatalf("an unknown tool: %s", w.Body.String())
	}
}

// The tools act as the user wherever their browser is signed in: never
// for another computer, shared and with its key or not, and never for a
// web page on this one.
func TestBrowserMCPGuarded(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	h := lanGuard(New().Handler())
	list := `{"jsonrpc":"2.0","id":1,"method":"tools/list"}`
	const there = "192.0.2.7:4000"
	if w := browserPost(h, there, list); w.Code/100 == 2 {
		t.Fatalf("not shared: %d %s", w.Code, w.Body.String())
	}
	_, secrets := newCaller(t, "Laptop")
	if w := browserPost(h, there, list, "Authorization", "Bearer "+secrets[0]); w.Code != http.StatusForbidden {
		t.Fatalf("shared, with a key: %d %s", w.Code, w.Body.String())
	}
	if w := browserPost(h, "127.0.0.1:5000", list, "Origin", "http://localhost:5173"); w.Code != http.StatusForbidden {
		t.Fatalf("a page on this computer: %d %s", w.Code, w.Body.String())
	}
	for _, addr := range []string{there, "127.0.0.1:5000"} {
		r := httptest.NewRequest(http.MethodGet, BrowserStatusPath, nil)
		r.RemoteAddr = addr
		r.Host = "127.0.0.1:3425"
		r.Header.Set("Authorization", "Bearer "+secrets[0])
		if addr != there {
			r.Header.Set("Origin", "http://localhost:5173")
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != http.StatusForbidden {
			t.Fatalf("status from %s: %d %s", addr, w.Code, w.Body.String())
		}
	}
}

// Only the extension itself, on this computer, may be the browser: a page
// that connects in its place would be handed every call, and could answer
// them.
func TestBrowserExtensionOnlyFromItsOrigin(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	h := lanGuard(New().Handler())
	newCaller(t, "Laptop")
	for _, c := range []struct{ addr, host, origin string }{
		{"127.0.0.1:5000", "127.0.0.1:3425", "chrome-extension://abcdefghijklmnopabcdefghijklmnop"},
		{"127.0.0.1:5000", "127.0.0.1:3425", "http://localhost:5173"},
		{"127.0.0.1:5000", "127.0.0.1:3425", "null"},
		{"127.0.0.1:5000", "127.0.0.1:3425", ""},
		{"127.0.0.1:5000", "rebound.example:3425", browserext.Origin}, // DNS rebinding
		{"192.0.2.7:4000", "192.0.2.1:3425", browserext.Origin},       // another computer
	} {
		r := httptest.NewRequest(http.MethodGet, BrowserExtensionPath, nil)
		r.RemoteAddr, r.Host = c.addr, c.host
		r.Header.Set("Connection", "Upgrade")
		r.Header.Set("Upgrade", "websocket")
		r.Header.Set("Sec-WebSocket-Version", "13")
		r.Header.Set("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
		if c.origin != "" {
			r.Header.Set("Origin", c.origin)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		// refused, by the LAN guard (401 for a computer sending no key) or
		// by the handler (403): never switched to a WebSocket
		if w.Code != http.StatusForbidden && w.Code != http.StatusUnauthorized {
			t.Fatalf("%+v: %d %s", c, w.Code, w.Body.String())
		}
	}
}

// browser_* tool names the gateway lists are the ones the extension
// carries out.
func TestBrowserToolsAreTheExtensions(t *testing.T) {
	src, err := browserext.File("background.js")
	if err != nil {
		t.Fatal(err)
	}
	for _, tl := range browserTools() {
		name := tl.(map[string]any)["name"].(string)
		if !strings.Contains(string(src), "  async "+name+"(session, args) {") {
			t.Errorf("%s isn't in background.js's TOOLS", name)
		}
	}
}

// With Magpie Browser in two browsers (packinggoodsllc_07668 on Discord:
// which one is used?), calls go to the last to connect until a session
// picks the other with browser_tabs use; another session stays where it
// was, the list of tabs says whose they are, and browsers lists both.
func TestBrowserMCPPicksAmongBrowsers(t *testing.T) {
	fresh(t)
	s := New()
	srv := httptest.NewServer(lanGuard(s.Handler()))
	t.Cleanup(srv.Close)
	in := func(name string) func(_, _, _ string, _ map[string]any) any {
		return func(_, _, _ string, _ map[string]any) any {
			return map[string]any{"content": []any{map[string]any{"type": "text", "text": "tabs of " + name}}}
		}
	}
	chrome := "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.7390.54 Safari/537.36"
	fakeExtensionAs(t, srv.URL, chrome, in("chrome"))
	browserConnected(t, s)
	fakeExtensionAs(t, srv.URL, chrome+" Edg/141.0.3537.57", in("edge"))
	for deadline := time.Now().Add(10 * time.Second); len(s.browser.all()) < 2; time.Sleep(5 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the second extension never connected")
		}
	}
	tabs := func(session string, args map[string]any) (string, bool) {
		t.Helper()
		w := browserPost(lanGuard(s.Handler()), "127.0.0.1:5000", imageToolCall("browser_tabs", args), BrowserSessionHeader, session)
		m := readImageReply(t, w)
		var all []string
		for _, c := range m.Result.Content {
			all = append(all, c.Text)
		}
		return strings.Join(all, "\n"), m.Result.IsError
	}
	if got, _ := tabs("s1", map[string]any{}); !strings.Contains(got, "tabs of edge") || !strings.Contains(got, "b2's tabs (Edge 141); 2 browsers") {
		t.Fatalf("s1 before picking: %q", got)
	}
	if got, _ := tabs("s1", map[string]any{"action": "browsers"}); !strings.Contains(got, "  b1 Chrome 141, extension 0.1.0") || !strings.Contains(got, "* b2 Edge 141, extension 0.1.0") {
		t.Fatalf("browsers: %q", got)
	}
	if got, bad := tabs("s1", map[string]any{"action": "use", "browser": "Chrome"}); bad || !strings.Contains(got, "b1 (Chrome 141)") {
		t.Fatalf("use chrome: %q", got)
	}
	if got, _ := tabs("s1", map[string]any{"action": "list"}); !strings.Contains(got, "tabs of chrome") || !strings.Contains(got, "b1's tabs") {
		t.Fatalf("s1 after picking chrome: %q", got)
	}
	if got, _ := tabs("s2", map[string]any{}); !strings.Contains(got, "tabs of edge") {
		t.Fatalf("s2 was moved too: %q", got)
	}
	if got, bad := tabs("s2", map[string]any{"action": "use", "browser": "firefox"}); !bad || !strings.Contains(got, `no connected browser is "firefox"`) {
		t.Fatalf("use firefox: %q", got)
	}
	if got, bad := tabs("s2", map[string]any{"action": "use", "browser": "b1"}); bad || !strings.Contains(got, "b1") {
		t.Fatalf("use b1: %q", got)
	}
}

func TestBrowserName(t *testing.T) {
	for ua, want := range map[string]string{
		"Chrome/140": "Chrome 140",
		"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.3537.57": "Edge 141",
		"Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 OPR/124.0.0.0":                                   "Opera 124",
		"": "a Chromium browser",
	} {
		if got := browserName(ua); got != want {
			t.Errorf("browserName(%q) = %q, want %q", ua, got, want)
		}
	}
}
