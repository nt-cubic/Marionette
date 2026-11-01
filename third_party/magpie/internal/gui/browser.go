package gui

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"github.com/yetone/magpie/internal/browserext"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/settings"
)

// Magpie Browser's extension, as the Library page sets it up: written to a
// folder of magpie's, which the user loads in their own browser
// (chrome://extensions → Load unpacked), and connected or not as the
// gateway says.

func browserExtDir() string { return filepath.Join(settings.Dir(), "browser-extension") }

type browserJSON struct {
	Dir       string `json:"dir"`
	Written   bool   `json:"written"`
	Version   string `json:"version"`
	Connected bool   `json:"connected"`
	Extension string `json:"extension,omitempty"` // the connected one's version
	Browser   string `json:"browser,omitempty"`
}

func browserState(ctx context.Context) browserJSON {
	out := browserJSON{Dir: browserExtDir(), Version: browserext.Version()}
	if _, err := os.Stat(filepath.Join(out.Dir, "manifest.json")); err == nil {
		out.Written = true
		// kept as this magpie has it, and pointed at its gateway: the browser
		// reads the folder again when it starts
		browserext.Write(out.Dir, gateway.URL())
	}
	ctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, gateway.URL()+gateway.BrowserStatusPath, nil)
	if err != nil {
		return out
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return out
	}
	defer resp.Body.Close()
	var st struct {
		Connected bool   `json:"connected"`
		Extension string `json:"extension"`
		Browser   string `json:"browser"`
	}
	if resp.StatusCode == http.StatusOK && json.NewDecoder(resp.Body).Decode(&st) == nil {
		out.Connected, out.Extension, out.Browser = st.Connected, st.Extension, st.Browser
	}
	return out
}

func browserRoutes(mux *http.ServeMux, w Windows) {
	mux.HandleFunc("GET /api/library/browser", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, browserState(r.Context()))
	})
	// Set up: the extension written, and its folder shown for Load unpacked
	mux.HandleFunc("POST /api/library/browser/setup", func(rw http.ResponseWriter, r *http.Request) {
		dir := browserExtDir()
		if err := browserext.Write(dir, gateway.URL()); err != nil {
			fail(rw, err)
			return
		}
		st := browserState(r.Context())
		var reveal string
		if err := w.OpenFolder(dir); err != nil {
			reveal = err.Error()
		}
		writeJSON(rw, struct {
			browserJSON
			RevealError string `json:"revealError,omitempty"`
		}{st, reveal})
	})
}
