package gateway

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/browserext"
)

// The fixture page the real browser works in, served from this test.
const browserFixture = `<!doctype html>
<title>Fixture</title>
<h1>Magpie fixture</h1>
<input aria-label="Name" id="name">
<button onclick="out.textContent = 'clicked ' + document.getElementById('name').value">Go</button>
<select aria-label="Colour" onchange="out.textContent = 'colour ' + this.value"><option value="r">Red</option><option value="g">Green</option></select>
<button onmouseover="out.textContent = 'hovered'">Hover me</button>
<input type="file" aria-label="Upload" onchange="out.textContent = 'file ' + this.files[0].name">
<button id="src">Drag me</button> <button id="dst">Drop here</button>
<button onclick="alert('hello')">Alert</button>
<button onclick="out.textContent = confirm('sure?') ? 'confirmed' : 'declined'">Confirm</button>
<button onclick="console.error('boom from the page')">Log</button>
<button onclick="setTimeout(() => out.textContent = 'later', 400)">Later</button>
<a href="/two" target="_blank">Open two</a>
<div id="out" role="status"></div>
<div style="height: 4000px"></div>
<p>Bottom of the page</p>
<script>
  const out = document.getElementById('out');
  let down = null;
  document.addEventListener('mousedown', (e) => { down = e.target.id; });
  document.addEventListener('mouseup', (e) => { if (down === 'src' && e.target.id === 'dst') out.textContent = 'dropped'; down = null; });
</script>`

// Magpie Browser in a real Chromium: the extension magpie writes, loaded
// as a user loads it, connects to the gateway, and every tool an agent
// calls is carried out in a real tab. Set MAGPIE_BROWSER_CHROME to a
// Chromium that takes --load-extension (Chrome for Testing, as Playwright
// has it) to run it.
func TestBrowserToolsInARealBrowser(t *testing.T) {
	srv, site := realBrowser(t)
	call, ok := browserCaller(t, srv, "e2e")
	ref := func(page, role, name string) string {
		t.Helper()
		m := regexp.MustCompile(`- ` + role + ` "` + regexp.QuoteMeta(name) + `"(?: \[[a-z=, ]+\])? \[ref=(e\d+)\]`).FindStringSubmatch(page)
		if m == nil {
			t.Fatalf("no %s %q with a ref in:\n%s", role, name, page)
		}
		return m[1]
	}
	shows := func(want string) {
		t.Helper()
		ok("browser_evaluate", map[string]any{"script": "() => document.getElementById('out').textContent"}, want)
	}

	page := ok("browser_tabs", map[string]any{"action": "new", "url": site.URL}, "Opened t", "Tab t", "Fixture", "Magpie fixture")
	ok("browser_tabs", nil, "* t", site.URL)
	page = ok("browser_snapshot", nil, `heading "Magpie fixture"`)
	ok("browser_snapshot", map[string]any{"interactive": true}, `button "Go"`)

	ok("browser_type", map[string]any{"ref": ref(page, "textbox", "Name"), "text": "Ada"}, "Typed into")
	ok("browser_click", map[string]any{"ref": ref(page, "button", "Go")}, "Clicked", "clicked Ada")
	ok("browser_press", map[string]any{"key": "Backspace", "ref": ref(page, "textbox", "Name")}, "Pressed Backspace")
	ok("browser_click", map[string]any{"ref": ref(page, "button", "Go")}, "clicked Ad")
	ok("browser_type", map[string]any{"ref": ref(page, "textbox", "Name"), "text": "Bo", "slowly": true, "clear": false})
	ok("browser_click", map[string]any{"ref": ref(page, "button", "Go")}, "clicked AdBo")

	ok("browser_select", map[string]any{"ref": ref(page, "combobox", "Colour"), "values": []string{"Green"}}, `Chose "Green"`)
	shows("colour g")
	ok("browser_hover", map[string]any{"ref": ref(page, "button", "Hover me")}, "Hovering")
	shows("hovered")
	ok("browser_drag", map[string]any{"from": ref(page, "button", "Drag me"), "to": ref(page, "button", "Drop here")}, "Dragged")
	shows("dropped")

	file := filepath.Join(t.TempDir(), "notes.txt")
	os.WriteFile(file, []byte("hi"), 0o600)
	ok("browser_upload", map[string]any{"ref": ref(page, `\w+`, "Upload"), "paths": []string{file}}, "Chose 1 file")
	shows("file notes.txt")

	ok("browser_click", map[string]any{"ref": ref(page, "button", "Later")})
	ok("browser_wait", map[string]any{"text": "later"}, "Done waiting")
	ok("browser_wait", map[string]any{"seconds": 0.2}, "Waited")

	ok("browser_click", map[string]any{"ref": ref(page, "button", "Alert")}, `alert dialog is open: "hello"`)
	ok("browser_dialog", nil, `Accepted the alert dialog ("hello")`)
	ok("browser_dialog", map[string]any{"accept": false}, "next one", "dismissed")
	ok("browser_click", map[string]any{"ref": ref(page, "button", "Confirm")}, "was dismissed as asked")
	shows("declined")

	ok("browser_click", map[string]any{"ref": ref(page, "button", "Log")})
	ok("browser_console", map[string]any{"level": "errors"}, "boom from the page")

	ok("browser_scroll", map[string]any{"direction": "down", "amount": 600}, "Scrolled down 600px")
	ok("browser_evaluate", map[string]any{"script": "window.scrollY"}, "600")
	ok("browser_text", nil, "Magpie fixture", "Bottom of the page")

	shot := call("browser_screenshot", nil)
	if shot.Result.IsError || shot.Result.Content[0].Type != "image" || shot.Result.Content[0].MimeType != "image/jpeg" {
		t.Fatalf("screenshot: %+v", shot.Result)
	}
	if b, err := base64.StdEncoding.DecodeString(shot.Result.Content[0].Data); err != nil || !bytes.HasPrefix(b, []byte{0xff, 0xd8}) {
		t.Fatalf("the screenshot isn't a JPEG: %v", err)
	}
	png := call("browser_screenshot", map[string]any{"ref": ref(page, "button", "Go"), "png": true})
	if b, _ := base64.StdEncoding.DecodeString(png.Result.Content[0].Data); png.Result.IsError || !bytes.HasPrefix(b, []byte("\x89PNG")) {
		t.Fatalf("an element's screenshot isn't a PNG: %+v", png.Result.Content[len(png.Result.Content)-1])
	}

	ok("browser_navigate", map[string]any{"url": site.URL + "/two"}, "Opened", "Page two")
	ok("browser_navigate", map[string]any{"go": "back"}, "Went back", "Magpie fixture")
	page = ok("browser_navigate", map[string]any{"go": "reload"}, "Reloaded", "Magpie fixture")

	// a link that opens a tab: the session goes on in it
	two := ok("browser_click", map[string]any{"ref": ref(page, "link", "Open two")}, "opened a new tab", "Page two")
	id := regexp.MustCompile(`new tab, (t\d+)`).FindStringSubmatch(two)[1]
	ok("browser_tabs", map[string]any{"action": "close", "tab": id}, "Closed "+id)
	if m := call("browser_snapshot", map[string]any{"tab": id}); !m.Result.IsError {
		t.Fatalf("a closed tab still answers: %+v", m.Result)
	}
}

// realBrowser starts a Chromium with the extension magpie writes loaded
// in a profile of its own, pointed at a gateway of the test's, and waits
// for it to connect; site serves browserFixture (and /two).
func realBrowser(t *testing.T) (srv, site *httptest.Server) {
	t.Helper()
	chrome := os.Getenv("MAGPIE_BROWSER_CHROME")
	if chrome == "" {
		t.Skip("MAGPIE_BROWSER_CHROME names no Chromium")
	}
	fresh(t)

	s := New()
	srv = httptest.NewServer(lanGuard(s.Handler()))
	t.Cleanup(srv.Close)
	site = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		if r.URL.Path == "/two" {
			io.WriteString(w, "<!doctype html><title>Two</title><h1>Page two</h1>")
			return
		}
		io.WriteString(w, browserFixture)
	}))
	t.Cleanup(site.Close)

	ext := t.TempDir()
	if err := browserext.Write(ext, srv.URL); err != nil {
		t.Fatal(err)
	}
	profile := t.TempDir()
	cmd := exec.Command(chrome, "--headless=new", "--user-data-dir="+profile,
		"--disable-extensions-except="+ext, "--load-extension="+ext,
		"--no-first-run", "--no-default-browser-check", "--disable-gpu", "--remote-debugging-port=0",
		// with the test's own HOME there is no keychain (macOS) or keyring
		// (Linux), and Chrome waits on it before the extension's storage
		// answers
		"--use-mock-keychain", "--password-store=basic", "about:blank")
	var logs bytes.Buffer
	cmd.Stdout, cmd.Stderr = &logs, &logs
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		defer cmd.Wait()
		defer cmd.Process.Kill()
		if t.Failed() {
			t.Logf("chromium said:\n%s", logs.String())
			// what it runs: is the extension's worker among them
			if port, err := os.ReadFile(filepath.Join(profile, "DevToolsActivePort")); err == nil {
				if resp, err := http.Get("http://127.0.0.1:" + strings.SplitN(string(port), "\n", 2)[0] + "/json/list"); err == nil {
					b, _ := io.ReadAll(resp.Body)
					resp.Body.Close()
					t.Logf("its targets: %s", b)
				}
			}
		}
	})
	for deadline := time.Now().Add(60 * time.Second); s.browser.current() == nil; {
		if time.Now().After(deadline) {
			t.Fatal("the extension never connected")
		}
		time.Sleep(50 * time.Millisecond)
	}
	return srv, site
}

// browserCaller calls the browser's tools as the agent session session:
// call answers with the reply, ok with the text of one that should work.
func browserCaller(t *testing.T, srv *httptest.Server, session string) (call func(string, map[string]any) imageReply, ok func(string, map[string]any, ...string) string) {
	call = func(tool string, args map[string]any) imageReply {
		t.Helper()
		req, _ := http.NewRequest(http.MethodPost, srv.URL+BrowserMCPPath, strings.NewReader(imageToolCall(tool, args)))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Accept", "application/json, text/event-stream")
		req.Header.Set(BrowserSessionHeader, session)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		b, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		var m imageReply
		if resp.StatusCode != 200 || json.Unmarshal(b, &m) != nil || m.Error != nil || len(m.Result.Content) == 0 {
			t.Fatalf("%s: %d %s", tool, resp.StatusCode, b)
		}
		return m
	}
	// ok calls a tool that should work and answers with its text
	ok = func(tool string, args map[string]any, want ...string) string {
		t.Helper()
		m := call(tool, args)
		var said []string
		for _, c := range m.Result.Content {
			said = append(said, c.Text)
		}
		out := strings.Join(said, "\n")
		if m.Result.IsError {
			t.Fatalf("%s %v failed: %s", tool, args, out)
		}
		for _, w := range want {
			if !strings.Contains(out, w) {
				t.Fatalf("%s %v doesn't say %q:\n%s", tool, args, w, out)
			}
		}
		return out
	}
	return call, ok
}

// Two agent sessions at once in a real Chromium (packinggoodsllc_07668 on
// Discord: two agents behind one router took one tab): the first works in
// the tab the user is looking at, the second, finding that tab another's,
// in a new tab of its own; each opens its tabs in a group of its own
// ("Magpie", "Magpie 2"), and each sees the other's tab marked ~.
func TestBrowserSessionsApartInARealBrowser(t *testing.T) {
	srv, site := realBrowser(t)
	_, a := browserCaller(t, srv, "agent-a")
	_, b := browserCaller(t, srv, "agent-b")
	tabOf := func(page string) string {
		t.Helper()
		m := regexp.MustCompile(`Tab (t\d+):`).FindStringSubmatch(page)
		if m == nil {
			t.Fatalf("no tab in:\n%s", page)
		}
		return m[1]
	}
	ta := tabOf(a("browser_navigate", map[string]any{"url": site.URL}, "Magpie fixture"))
	tb := tabOf(b("browser_navigate", map[string]any{"url": site.URL + "/two"}, "Page two"))
	if ta == tb {
		t.Fatalf("both sessions work in %s", ta)
	}
	if got := tabOf(a("browser_snapshot", nil)); got != ta {
		t.Fatalf("a went from %s to %s", ta, got)
	}
	a("browser_evaluate", map[string]any{"script": "document.title"}, "Fixture")
	b("browser_evaluate", map[string]any{"script": "document.title"}, "Two")
	if list := a("browser_tabs", nil); !regexp.MustCompile(`(?m)^\* `+ta+` `).MatchString(list) || !regexp.MustCompile(`(?m)^~ `+tb+` `).MatchString(list) {
		t.Fatalf("a's list doesn't mark its tab %s * and b's %s ~:\n%s", ta, tb, list)
	}

	// a tab each opens goes in its own group, and calls go on in it
	a("browser_tabs", map[string]any{"action": "new", "url": site.URL})
	b("browser_tabs", map[string]any{"action": "new", "url": site.URL + "/two"})
	// the group of the tab a list marks mark with the page title
	groupOf := func(list, mark, title string) string {
		t.Helper()
		m := regexp.MustCompile(`(?m)^` + regexp.QuoteMeta(mark) + ` t\d+ \[(Magpie(?: \d+)?)\] "` + title + `"`).FindStringSubmatch(list)
		if m == nil {
			t.Fatalf("no %s tab %q in a Magpie group:\n%s", mark, title, list)
		}
		return m[1]
	}
	list := a("browser_tabs", nil)
	ga, gb := groupOf(list, "*", "Fixture"), groupOf(list, "~", "Two")
	if ga == gb {
		t.Fatalf("both sessions' tabs are in %s:\n%s", ga, list)
	}
	list = b("browser_tabs", nil)
	if groupOf(list, "*", "Two") != gb || groupOf(list, "~", "Fixture") != ga {
		t.Fatalf("b's list doesn't have its tab in %s and a's in %s:\n%s", gb, ga, list)
	}
}
