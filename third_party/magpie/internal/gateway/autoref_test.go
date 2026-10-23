package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
)

// A group magpie found is listed as auto/<model> (ZeroClover on X), and
// served by that id and by its old group/auto-<slug> alike: one group, one
// route, one name in the usage log, whichever an agent sent.
func TestFoundGroupServedByBothIDs(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"glm-5.3"}, a)
	serveOn(t, "b", "kb", []string{"glm-5.3"}, b)
	s := New()

	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("GET", "/v1/models", nil))
	var list struct{ Data []struct{ ID string } }
	if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
		t.Fatal(err)
	}
	var ids []string
	for _, m := range list.Data {
		ids = append(ids, m.ID)
	}
	if !slices.Contains(ids, "auto/glm-5.3") || slices.Contains(ids, "group/auto-glm-5-3") {
		t.Fatalf("/v1/models: %v", ids)
	}

	for _, model := range []string{"auto/glm-5.3", "group/auto-glm-5-3", "auto/GLM-5.3", "auto/glm-5-3"} {
		code, body := postAs(t, s, "", `{"model":"`+model+`","messages":[{"role":"user","content":"hi `+model+`"}]}`)
		if code != 200 {
			t.Fatalf("%s: %d %s", model, code, body)
		}
		r := s.trace.routes[len(s.trace.routes)-1]
		if r.Group == nil || r.Group.ID != "auto-glm-5-3" {
			t.Fatalf("%s, not routed as the found group: %+v", model, r)
		}
		if u := lastUsage(t); u.Requested != "group/auto-glm-5-3" {
			t.Errorf("%s logged as asked for %q", model, u.Requested)
		}
	}
}

// A gateway key held to a found group passes for it by either id: one
// naming auto/<model>, as the lists now give it, for an agent left on the
// old id, and one naming the old id, or group/*, for auto/<model>. auto/*
// holds every found group by its new id. A key held to another group gets
// neither.
func TestKeyHeldToAFoundGroupByEitherID(t *testing.T) {
	fresh(t)
	serveOn(t, "a", "ka", []string{"glm-5.3", "x"}, &keyed{})
	serveOn(t, "b", "kb", []string{"glm-5.3", "x"}, &keyed{})
	if err := provider.SaveGroup(provider.Group{Name: "Other", Members: []string{"a/x"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	keys, secrets := newCaller(t, "k")
	post := func(model string) int {
		t.Helper()
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`))
		req.Header.Set("Authorization", "Bearer "+secrets[0])
		s.Handler().ServeHTTP(rec, req)
		return rec.Code
	}
	for _, held := range [][]string{{"auto/glm-5.3"}, {"auto/glm-5-3"}, {"group/auto-glm-5-3"}, {"group/*"}, {"auto/*"}, {"group/other"}} {
		if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: held}); err != nil {
			t.Fatal(err)
		}
		want := 200
		if held[0] == "group/other" {
			want = 403
		}
		for _, model := range []string{"auto/glm-5.3", "group/auto-glm-5-3"} {
			if code := post(model); code != want {
				t.Errorf("a key held to %v asking for %s: %d, want %d", held, model, code, want)
			}
		}
	}
}
