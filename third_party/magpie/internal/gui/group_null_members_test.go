package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A group made of patterns only whose provider stops listing the models
// they matched — a plugin behind on updates lists none — has no members.
// /api/groups says so as [], never null: the Routing page read null as
// "null is not an object (evaluating 'g.members.map')" and drew no group
// at all (#1585, dbydd). The templates that can't be made say [] too.
func TestGroupsNeverNullMembers(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "a", Name: "a", Key: "ka", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "p", Name: "P", Match: []string{"a/m*"}}); err != nil {
		t.Fatal(err)
	}
	p, err := provider.Find("a")
	if err != nil {
		t.Fatal(err)
	}
	p.Models = []string{"other"} // what the pattern matched is gone
	if err := provider.Save(*p); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	groupRoutes(mux)
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/api/groups", nil))
	var got struct {
		Groups []struct {
			ID      string           `json:"id"`
			Members *json.RawMessage `json:"members"`
		} `json:"groups"`
		Templates []struct {
			Kind  string `json:"kind"`
			Group struct {
				Members *json.RawMessage `json:"members"`
			} `json:"group"`
		} `json:"templates"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatalf("%v: %s", err, w.Body)
	}
	seen := false
	for _, g := range got.Groups {
		if g.ID == "p" {
			seen = true
		}
		if g.Members == nil || string(*g.Members) == "null" {
			t.Errorf("group %s: members %v, want a list", g.ID, g.Members)
		}
	}
	if !seen {
		t.Fatalf("no group p: %s", w.Body)
	}
	for _, tp := range got.Templates {
		if tp.Group.Members == nil || string(*tp.Group.Members) == "null" {
			t.Errorf("template %s: members null, want a list", tp.Kind)
		}
	}
}
