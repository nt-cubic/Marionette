package gui

import (
	"net/http"
	"net/http/httptest"
	"runtime"
	"strings"
	"testing"
)

// Wails strips the URL's scheme before calling the window's handler. The
// policy must still name that scheme: WebKit blocks a scheme-less host
// source under wails://, so the bridge never says hello.
func TestPluginPageWindowPolicy(t *testing.T) {
	pagePlugins(t)
	page := pluginPages()[0]
	// Wails' own base URLs (internal/assetserver/assetserver_<os>.go):
	// wails://localhost on macOS and Linux, http://wails.localhost on Windows.
	desktop := "wails://localhost"
	if runtime.GOOS == "windows" {
		desktop = "http://wails.localhost"
	}
	desktopHost := strings.TrimPrefix(strings.TrimPrefix(desktop, "wails://"), "http://")
	for _, c := range []struct {
		name   string
		host   string
		window bool
		source string
	}{
		{"desktop", desktopHost, true, desktop},
		{"web", "127.0.0.1:3437", false, "127.0.0.1:3437"},
		{"web localhost", "localhost", false, "localhost"},
	} {
		t.Run(c.name, func(t *testing.T) {
			var handler http.Handler = pageMux()
			if c.window {
				handler = ownWindow(handler)
			}
			for _, file := range []string{"index.html", pluginBridgePath, "app.js", "style.css", "icon.svg"} {
				url := pluginPagePrefix + page.token + "/" + file
				r := httptest.NewRequest("GET", url, nil)
				r.Host = c.host
				// A web request cannot opt into the native policy by naming
				// the window's host or sending Wails' headers.
				r.Header.Set("X-Wails-Window-Id", "1")
				w := do(handler, r)
				if w.Code != http.StatusOK {
					t.Fatalf("%s: HTTP %d", file, w.Code)
				}
				policy := w.Header().Get("Content-Security-Policy")
				source := c.source + pluginPagePrefix + page.token + "/"
				for _, directive := range []string{"script-src", "style-src", "img-src", "font-src", "media-src"} {
					if !strings.Contains(policy, directive+" "+source) {
						t.Errorf("%s: %s cannot load the page's own files: %s", file, directive, policy)
					}
				}
				for _, directive := range []string{"sandbox allow-scripts", "connect-src 'none'", "frame-src 'none'", "form-action 'none'"} {
					if !strings.Contains(policy, directive) {
						t.Errorf("%s: missing %s", file, directive)
					}
				}
			}
		})
	}
}
