package agent

import (
	"encoding/json"
	"os"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
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

// A found group's auto/<model> follows the spelling models.dev gives the
// model most (XYenon on Discord), so one magpie listed before as
// auto/glm-5-3-flash is auto/glm-5.3-flash now. An agent magpie set to the
// old spelling is moved onto the new one on the next catalog sync, as one
// set to group/auto-<slug> is: OpenCode checks its model against the list
// magpie writes, which names the group by its new spelling only. One the
// user set to it themselves keeps their value, which is still the group.
func TestFoundGroupMovedOffAnOldSpelling(t *testing.T) {
	syncHome(t)
	os.WriteFile(catalog.CachePath(), []byte(`{
	  "zai": {"models": {"glm-5.3-flash": {"id":"glm-5.3-flash","name":"GLM-5.3-Flash"}}},
	  "fireworks": {"models": {"glm-5.3-flash": {"id":"glm-5.3-flash","name":"GLM-5.3-Flash"}}}}`), 0o644)
	catalog.Reset()
	for id, m := range map[string]string{"ark": "glm-5-3-flash", "spare": "glm-5.3-flash"} {
		if err := provider.Save(provider.Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{m}}); err != nil {
			t.Fatal(err)
		}
	}
	if got := provider.PublicRef("group/auto-glm-5-3-flash"); got != "auto/glm-5.3-flash" {
		t.Fatalf("the found group is listed as %q", got)
	}
	a, err := Find("opencode")
	if err != nil {
		t.Fatal(err)
	}
	if err := a.Apply("model", "spare/glm-5.3-flash"); err != nil {
		t.Fatal(err)
	}
	// what magpie wrote and recorded while the group was listed by the
	// first provider's spelling
	old := "magpie/auto/glm-5-3-flash"
	if err := edit.SetJSON(a.Path, edit.KV{Path: "model", Value: old}); err != nil {
		t.Fatal(err)
	}
	record(a.ID, "model", old)
	SyncCatalog()
	if got := a.Values()["model"]; got != "magpie/auto/glm-5.3-flash" {
		t.Fatalf("magpie's own value became %q, want magpie/auto/glm-5.3-flash\n%s", got, readFile(a.Path))
	}
	if got := appliedOf(a.ID).Fields["model"]; got != "magpie/auto/glm-5.3-flash" {
		t.Errorf("applied.json holds %q", got)
	}
	// set as it is listed now: left alone
	SyncCatalog()
	if got := a.Values()["model"]; got != "magpie/auto/glm-5.3-flash" {
		t.Errorf("the group as it is listed became %q", got)
	}

	// the user's own: no record of the field
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
	// and a pick of the old spelling written now is the group as listed
	if err := a.Apply("model", "magpie/auto/glm-5-3-flash"); err != nil {
		t.Fatal(err)
	}
	if got := a.Values()["model"]; got != "magpie/auto/glm-5.3-flash" {
		t.Errorf("a pick of the old spelling was written as %q", got)
	}
}
