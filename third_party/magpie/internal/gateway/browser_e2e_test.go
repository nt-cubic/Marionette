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
	chrome := os.Getenv("MAGPIE_BROWSER_CHROME")
	if chrome == "" {
		t.Skip("MAGPIE_BROWSER_CHROME names no Chromium")
	}
	fresh(t)
	s := New()
	srv := httptest.NewServer(lanGuard(s.Handler()))
	t.Cleanup(srv.Close)
	site := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
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

	call := func(tool string, args map[string]any) imageReply {
		t.Helper()
		req, _ := http.NewRequest(http.MethodPost, srv.URL+BrowserMCPPath, strings.NewReader(imageToolCall(tool, args)))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Accept", "application/json, text/event-stream")
		req.Header.Set(BrowserSessionHeader, "e2e")
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
	ok := func(tool string, args map[string]any, want ...string) string {
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
