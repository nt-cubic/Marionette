package provider

import (
	"path/filepath"
	"testing"
	"time"
)

// A model value no entry has (an agent's bare "gpt-5.5" in its config)
// and a group's ref are resolved, request after request, from what the
// first request built while nothing moved: the window's drift poll asked
// for each every 15s, and each built every provider's whole list again,
// or copied the whole catalog to find one group (1.3 GB of a live
// magpie's 33 GB in 53 minutes). A provider saved since is seen at once.
func TestResolveKeptAcrossRequests(t *testing.T) {
	isolate(t)
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	defer func(d, k time.Duration) { heldFor, keptFor = d, k }(heldFor, keptFor)
	heldFor = 0           // each request after the last one's hold has gone
	keptFor = time.Minute // a loaded machine's pause isn't the bound
	var models []string
	for _, m := range []string{"a", "b", "c", "d", "e", "f", "g", "h"} {
		for _, n := range []string{"1", "2", "3", "4", "5"} {
			models = append(models, "m-"+m+n)
		}
	}
	for _, id := range []string{"r1", "r2", "r3"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: models}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{ID: "g", Name: "g", Members: []string{"r1/m-a1", "r2/m-a1"}}); err != nil {
		t.Fatal(err)
	}
	request := func(id string) bool {
		defer Hold()()
		_, _, ok := Resolve(id)
		return ok
	}
	for _, id := range []string{"group/g", "nope", "m-b2"} {
		request(id)
		if n := testing.AllocsPerRun(20, func() { request(id) }); n > 60 {
			t.Errorf("%s resolved again with nothing changed: %.0f allocations", id, n)
		}
	}
	if !request("group/g") || request("nope") || !request("m-b2") {
		t.Fatal("resolved wrong")
	}
	if err := Save(Provider{ID: "r4", Name: "r4", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m-new"}}); err != nil {
		t.Fatal(err)
	}
	if !request("m-new") {
		t.Fatal("a model saved since isn't resolved by the next request")
	}
}
