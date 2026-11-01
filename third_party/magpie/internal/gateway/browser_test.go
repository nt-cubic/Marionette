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
	ctx, cancel := context.WithCancel(context.Background())
	c, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(base, "http")+BrowserExtensionPath,
		&websocket.DialOptions{HTTPHeader: http.Header{"Origin": {browserext.Origin}}})
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	done := make(chan struct{})
	t.Cleanup(func() { cancel(); c.CloseNow(); <-done })
	if err := c.Write(ctx, websocket.MessageText, []byte(`{"type":"hello","version":"0.1.0","agent":"Chrome/140"}`)); err != nil {
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
