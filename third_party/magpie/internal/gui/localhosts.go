package gui

import (
	"encoding/json"
	"net/http"
	"strings"
	"sync"

	"github.com/yetone/magpie/internal/provider"
)

// localHostsRoutes: POST /api/local-hosts {"hosts":[...]} answers
// {"local":{host:bool}}, whether each host is on this computer or the
// local network (provider.LocalHost). The provider editor shows a caution
// under "Send requests unmasked" for a provider with an address that
// isn't; a page can't resolve a name of the user's own DNS
// (http://llm.corp.lan:8000, AcMilan on Discord), so it asks here. It
// only words the editor: the switch is the user's, and no request asks.
func localHostsRoutes(mux *http.ServeMux) {
	mux.HandleFunc("POST /api/local-hosts", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			Hosts []string `json:"hosts"`
		}
		if err := json.NewDecoder(http.MaxBytesReader(rw, r.Body, 16<<10)).Decode(&in); err != nil {
			http.Error(rw, err.Error(), http.StatusBadRequest)
			return
		}
		if len(in.Hosts) > 8 { // a provider has five addresses
			in.Hosts = in.Hosts[:8]
		}
		var mu sync.Mutex
		var wg sync.WaitGroup
		out := map[string]bool{}
		for _, h := range in.Hosts {
			h = strings.ToLower(strings.TrimSpace(h))
			if h == "" || len(h) > 253 {
				continue
			}
			wg.Go(func() {
				local := provider.LocalHost(strings.Trim(h, "[]"))
				mu.Lock()
				out[h] = local
				mu.Unlock()
			})
		}
		wg.Wait()
		writeJSON(rw, map[string]any{"local": out})
	})
}
