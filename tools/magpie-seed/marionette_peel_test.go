package provider

import (
	"os"
	"testing"
)

func TestGroupForPeelsCodexEffortSuffix(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := Save(Provider{ID: "relay", Name: "Relay", Chat: "http://127.0.0.1:1/v1", Key: "k",
		Models: []string{"gpt-6.1-terra"}}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{
		ID:      "gpt-5.6-luna",
		Name:    "Marionette gpt-5.6-luna",
		Members: []string{"relay/gpt-6.1-terra:xhigh"},
		Pick:    "relay/gpt-6.1-terra:xhigh",
		Routing: Manual,
	}); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"gpt-5.6-luna", "gpt-5.6-luna[xhigh]", "gpt-5.6-luna[high]", "gpt-5.6-luna[max]"} {
		got, ok := GroupFor(id)
		if !ok || got != GroupPrefix+"gpt-5.6-luna" {
			t.Errorf("GroupFor(%q) = %q %v, want group/gpt-5.6-luna", id, got, ok)
		}
	}
	if got, ok := GroupFor("gpt-5.6-luna[1m]"); !ok || got != GroupPrefix+"gpt-5.6-luna" {
		t.Errorf("GroupFor(gpt-5.6-luna[1m]) = %q %v", got, ok)
	}
}
