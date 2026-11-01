package provider

import (
	"runtime"
	"strconv"
	"testing"
)

// GroupFinder reads every provider's models once and looks up many
// groups in them; the list's index is made once too, not for each group.
func TestGroupFinderIndexesTheListOnce(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	models := make([]string, 2000)
	for i := range models {
		models[i] = "model-with-a-longer-name-" + strconv.Itoa(i)
	}
	if err := Save(Provider{ID: "a", Name: "a", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: models}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{ID: "g", Name: "G", Members: []string{"a/" + models[7]}}); err != nil {
		t.Fatal(err)
	}
	find := GroupFinder()
	if _, ms, ok := find("group/g"); !ok || len(ms) != 1 || ms[0].Model != models[7] {
		t.Fatalf("group/g: %+v %v", ms, ok)
	}
	var before, after runtime.MemStats
	runtime.ReadMemStats(&before)
	const n = 20
	for range n {
		find("group/g")
	}
	runtime.ReadMemStats(&after)
	per := (after.TotalAlloc - before.TotalAlloc) / n
	t.Logf("%d bytes a lookup", per)
	if per > 16<<10 {
		t.Errorf("a group looked up again cost %d bytes: the list of %d models was indexed again", per, len(models))
	}
}
