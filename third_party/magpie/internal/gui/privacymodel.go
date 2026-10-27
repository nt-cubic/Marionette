package gui

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/settings"
)

// privacyModelJSON is Settings → Privacy's local masking model: whether it
// is on, the model masking uses (or would, turned on) with its own status
// (downloaded, downloading, where it runs), and every model there is to
// pick, with theirs.
type privacyModelJSON struct {
	On bool `json:"on"`
	pii.Status
	Models []pii.Status `json:"models"`
}

// modelOf and models are pii's; vars for tests.
var modelOf, models = pii.Of, pii.Models

func privacyModelState() privacyModelJSON {
	st := settings.Load()
	out := privacyModelJSON{On: st.RedactModel, Status: modelOf(pii.Choose(st.RedactModelName, st.RedactModel)).Now()}
	for _, m := range models() {
		out.Models = append(out.Models, m.Now())
	}
	return out
}

// privacyModelRoutes: GET /api/privacy-model is its state; POST with
// {"do": "on" | "off" | "cancel" | "remove" | "pick", "model": id} turns
// it on (as model, or the one it would use: downloading it first, and
// saving it on once that is done), off (a download is stopped, the model
// let go and kept), stops a download, turns it off and deletes every
// model downloaded, or picks model: while it is on, that is turning it on
// as model (the one before masks until model is downloaded); while off,
// the pick is kept for turning it on.
func privacyModelRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/privacy-model", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, privacyModelState())
	})
	mux.HandleFunc("POST /api/privacy-model", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			Do    string `json:"do"`
			Model string `json:"model"`
		}
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		if in.Model != "" && in.Model != pii.PrivacyFilter && in.Model != pii.AstrLinkGuard {
			fail(rw, fmt.Errorf("model is %s or %s, not %q", pii.PrivacyFilter, pii.AstrLinkGuard, in.Model))
			return
		}
		st := settings.Load()
		was := pii.Choose(st.RedactModelName, st.RedactModel)
		var err error
		switch in.Do {
		case "pick":
			switch {
			case in.Model == "":
				err = fmt.Errorf("pick needs a model")
			case st.RedactModel:
				err = turnOn(in.Model, was)
			default:
				err = settings.SetRedactModelName(in.Model)
			}
		case "on":
			id := in.Model
			if id == "" {
				id = was
			}
			err = turnOn(id, was)
		case "off":
			for _, m := range models() {
				m.Cancel()
			}
			err = settings.SetRedactModel(false)
			for _, m := range models() {
				m.Stop()
			}
		case "cancel":
			for _, m := range models() {
				m.Cancel()
			}
		case "remove":
			if err = settings.SetRedactModel(false); err == nil {
				for _, m := range models() {
					if e := m.Remove(); e != nil && err == nil {
						err = e
					}
				}
			}
		default:
			err = fmt.Errorf("do is on, off, cancel, remove or pick, not %q", in.Do)
		}
		if err != nil {
			fail(rw, err)
			return
		}
		writeJSON(rw, privacyModelState())
	})
}

// turnOn turns the local model on as id, once it is on disk: a request
// in between is masked as before (by the rules, or by was, the model on
// till then), and not held back. A download of another model under way
// is stopped; was is let go once id masks.
func turnOn(id, was string) error {
	m := modelOf(id)
	if !m.Supported() {
		return fmt.Errorf("the local masking model doesn't run on this system yet")
	}
	for _, o := range models() {
		if o.ID() != id {
			o.Cancel()
		}
	}
	on := func() {
		if settings.UseRedactModel(id) == nil {
			if was != id {
				modelOf(was).Stop()
			}
			m.Warm()
		}
	}
	if m.Installed() {
		on()
		return nil
	}
	return m.Download(on)
}
