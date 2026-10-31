package cli

import (
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// magpie provider fallback takes a routing group's id ("group/<id>") as a
// fallback beside models (01huadalang on Discord); a group that isn't
// there is an error, as a model none serves is.
func TestFallbackCmdTakesGroup(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("DSH_HOME", "")
	for _, p := range []provider.Provider{
		{ID: "plan", Name: "Plan", Key: "kp", Models: []string{"m"}, Chat: "http://127.0.0.1:1/v1"},
		{ID: "a", Name: "A", Key: "ka", Models: []string{"x"}, Chat: "http://127.0.0.1:2/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "backup", Name: "Backup", Members: []string{"a/x"}}); err != nil {
		t.Fatal(err)
	}
	if err := providerCmd([]string{"provider", "fallback", "plan", "group/backup", "a/x"}); err != nil {
		t.Fatal(err)
	}
	p, err := provider.Find("plan")
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"group/backup", "a/x"}; !slices.Equal(p.Fallback, want) {
		t.Fatalf("fallback %v, want %v", p.Fallback, want)
	}
	if err := providerCmd([]string{"provider", "fallback", "plan", "group/nothing"}); err == nil {
		t.Fatal("a group that isn't there was taken")
	}
}
