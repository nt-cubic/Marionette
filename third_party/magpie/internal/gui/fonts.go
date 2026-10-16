package gui

import (
	"encoding/json"
	"net/http"
	"sync"

	"github.com/yetone/magpie/internal/fonts"
	"github.com/yetone/magpie/internal/settings"
)

// onFonts updates existing webviews even while the other one has focus.
var onFonts func()

func sameFont(a, b *fonts.Face) bool {
	return a == nil && b == nil || a != nil && b != nil && *a == *b
}

func fontSettingsJS(s settings.Settings) string {
	b, _ := json.Marshal(map[string]any{"uiFont": s.UIFont, "codeFont": s.CodeFont})
	return "window.receiveFonts?.(" + string(b) + ");"
}

// Font discovery belongs to the desktop, never to a remote browser's
// server. A failed refresh leaves the last successful collection intact.
func fontRoutes(mux *http.ServeMux, web bool, discover func() ([]fonts.Family, error)) {
	var mu sync.Mutex
	var cached []fonts.Family
	mux.HandleFunc("GET /api/fonts", func(rw http.ResponseWriter, r *http.Request) {
		if web {
			http.Error(rw, "installed fonts are available only in the desktop app", http.StatusNotFound)
			return
		}
		mu.Lock()
		defer mu.Unlock()
		if cached == nil || r.URL.Query().Get("refresh") == "1" {
			found, err := discover()
			if err != nil {
				http.Error(rw, "could not read installed fonts", http.StatusServiceUnavailable)
				return
			}
			cached = found
			if cached == nil {
				cached = []fonts.Family{}
			}
		}
		writeJSON(rw, cached)
	})
}
