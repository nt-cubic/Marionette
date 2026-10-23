package agent

import (
	"encoding/json"
	"testing"

	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
)

// An agent magpie set to a found group by its old id, group/auto-<slug>,
// is moved onto the group's auto/<model> on the next catalog sync, its
// other settings kept (ZeroClover on X); one whose field the user set
// themselves, or changed since magpie set it, keeps their value.
func TestFoundGroupRenamedOnlyWhereMagpieSetIt(t *testing.T) {
	syncHome(t)
	// relay and spare both serve glm-4.6: magpie finds the group
	if err := provider.Save(provider.Provider{ID: "spare", Name: "Spare", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"glm-4.6"}}); err != nil {
		t.Fatal(err)
	}
	if got := provider.PublicRef("group/auto-glm-4-6"); got != "auto/glm-4.6" {
		t.Fatalf("the found group is listed as %q", got)
	}
	a, err := Find("opencode")
	if err != nil {
		t.Fatal(err)
	}
	if err := a.Apply("model", "relay/glm-4.6"); err != nil {
		t.Fatal(err)
	}
	set := a.Values()["model"]
	if set == "" {
		t.Fatalf("nothing set: %v", a.Values())
	}
	// what an older magpie wrote and recorded: the group by its old id,
	// beside a key of the user's own
	old := "magpie/group/auto-glm-4-6"
	if err := edit.SetJSON(a.Path, edit.KV{Path: "model", Value: old}, edit.KV{Path: "theme", Value: "mine"}); err != nil {
		t.Fatal(err)
	}
	record(a.ID, "model", old)

	SyncCatalog()
	if got := a.Values()["model"]; got != "magpie/auto/glm-4.6" {
		t.Fatalf("magpie's own value became %q, want magpie/auto/glm-4.6\n%s", got, readFile(a.Path))
	}
	if got := appliedOf(a.ID).Fields["model"]; got != "magpie/auto/glm-4.6" {
		t.Errorf("applied.json holds %q", got)
	}
	var doc map[string]any
	if err := json.Unmarshal([]byte(readFile(a.Path)), &doc); err != nil || doc["theme"] != "mine" {
		t.Errorf("the user's own key: %v %v", doc["theme"], err)
	}
	if d := a.Drift(); d != nil {
		t.Errorf("drift after the move: %+v", d)
	}

	// the user's own value, the group by its old id at an effort of their
	// choosing, where magpie had set the group plain: theirs, kept
	theirs := "magpie/group/auto-glm-4-6:high"
	if err := edit.SetJSON(a.Path, edit.KV{Path: "model", Value: theirs}); err != nil {
		t.Fatal(err)
	}
	record(a.ID, "model", old)
	SyncCatalog()
	if got := a.Values()["model"]; got != theirs {
		t.Fatalf("the user's %q became %q", theirs, got)
	}
	// and one magpie never set: no record of the field at all
	appliedMu.Lock()
	m := appliedLoad()
	delete(m, a.ID)
	appliedSave(m)
	appliedMu.Unlock()
	if err := edit.SetJSON(a.Path, edit.KV{Path: "model", Value: old}); err != nil {
		t.Fatal(err)
	}
	SyncCatalog()
	if got := a.Values()["model"]; got != old {
		t.Fatalf("the user's own %q became %q", old, got)
	}
}
