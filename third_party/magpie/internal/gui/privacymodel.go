package gui

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/settings"
)

// privacyModelJSON is Settings → Privacy's local masking model: whether it
// is on, and the model's own status (downloaded, downloading, where it
// runs).
type privacyModelJSON struct {
	On bool `json:"on"`
	pii.Status
}

func privacyModelState() privacyModelJSON {
	return privacyModelJSON{On: settings.Load().RedactModel, Status: pii.Now()}
}

// privacyModelRoutes: GET /api/privacy-model is its state; POST with
// {"do": "on" | "off" | "cancel" | "remove"} turns it on (downloading the
// model first, and saving it on once that is done), off (a download is
// stopped, pf-serve ended, the model kept), stops a download, or turns it off and deletes
// the model.
func privacyModelRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/privacy-model", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, privacyModelState())
	})
	mux.HandleFunc("POST /api/privacy-model", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			Do string `json:"do"`
		}
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		var err error
		switch in.Do {
		case "on":
			if pii.Installed() {
				if err = settings.SetRedactModel(true); err == nil {
					pii.Warm()
				}
				break
			}
			// on only once the model is on disk: a request in between is
			// masked by the rules, as before, and not held back
			err = pii.Download(func() {
				if settings.SetRedactModel(true) == nil {
					pii.Warm()
				}
			})
		case "off":
			pii.Cancel()
			err = settings.SetRedactModel(false)
			pii.Stop()
		case "cancel":
			pii.Cancel()
		case "remove":
			if err = settings.SetRedactModel(false); err == nil {
				err = pii.Remove()
			}
		default:
			err = fmt.Errorf("do is on, off, cancel or remove, not %q", in.Do)
		}
		if err != nil {
			fail(rw, err)
			return
		}
		writeJSON(rw, privacyModelState())
	})
}
