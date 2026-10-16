package provider

import (
	"fmt"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// #1818: a provider kept only to watch its balance, saved with Select
// none (and Only through routing groups or not), serves none of its
// models: not to agents, not in an auto group, not by its own
// "provider/model" id. Before, with no picks it served the vendor's first
// 24 and joined every auto group.
func TestSelectNoneServesNone(t *testing.T) {
	for _, unlisted := range []bool{false, true} {
		t.Run(fmt.Sprintf("unlisted=%v", unlisted), func(t *testing.T) {
			azureHome(t)
			var ms []catalog.Model
			for i := range manyModels + 6 {
				ms = append(ms, catalog.Model{ID: fmt.Sprintf("m%d", i)})
			}
			for _, id := range []string{"used", "other", "watched"} {
				p := Provider{ID: id, Name: id, Key: "sk-x", Chat: "https://" + id + ".example.invalid/v1"}
				if id == "watched" {
					p.NoModels, p.Unlisted = true, unlisted
				}
				if err := Save(p); err != nil {
					t.Fatal(err)
				}
				if err := catalog.SaveLive(id, p.Chat, ms); err != nil {
					t.Fatal(err)
				}
			}
			w, err := Find("watched")
			if err != nil {
				t.Fatal(err)
			}
			if !w.NoModels || !w.ServesNone() {
				t.Fatalf("Select none was not kept: %+v", w)
			}
			if n := len(w.Exposed()); n != 0 {
				t.Errorf("exposed %d, want none", n)
			}
			for _, e := range Catalog() {
				if e.Provider.ID == "watched" {
					t.Errorf("catalog has %s", e.ID)
				}
			}
			auto := 0
			for _, g := range Groups() {
				for _, m := range append(g.Members, g.Matched...) {
					if strings.HasPrefix(m, "watched/") {
						t.Errorf("group %s has %s", g.ID, m)
					}
				}
				if strings.HasPrefix(g.ID, "auto-") {
					auto++
				}
			}
			if auto == 0 { // used and other share every model
				t.Error("no auto groups")
			}
			if _, _, ok := Resolve("watched/m0"); ok {
				t.Error("watched/m0 resolved")
			}
			if p, _, ok := Resolve("used/m0"); !ok || p.ID != "used" {
				t.Errorf("used/m0: %v %v", p.ID, ok)
			}
			// a pick clears it
			if err := SetModels("watched", []string{"m3"}); err != nil {
				t.Fatal(err)
			}
			w, _ = Find("watched")
			if w.NoModels || w.ServesNone() || len(w.Exposed()) != 1 {
				t.Errorf("a pick after Select none: %+v exposed %d", w.NoModels, len(w.Exposed()))
			}
		})
	}
}

// A providers.json from before #1818 has no models key: the vendor's
// list is served as ever, up to manyModels.
func TestNoPicksStillServesTheVendorList(t *testing.T) {
	azureHome(t)
	var ms []catalog.Model
	for i := range manyModels + 6 {
		ms = append(ms, catalog.Model{ID: fmt.Sprintf("m%d", i)})
	}
	p := Provider{ID: "old", Name: "old", Key: "sk-x", Chat: "https://old.example.invalid/v1"}
	if err := Save(p); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("old", p.Chat, ms); err != nil {
		t.Fatal(err)
	}
	got, err := Find("old")
	if err != nil {
		t.Fatal(err)
	}
	if got.NoModels || len(got.Exposed()) != manyModels {
		t.Errorf("noModels %v exposed %d, want %d", got.NoModels, len(got.Exposed()), manyModels)
	}
}
