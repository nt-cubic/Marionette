package agent

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/pelletier/go-toml/v2"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// jiabubu on Discord: Kimi Code's own provider gives K2.8 Preview and
// K3.1 Pro thinking, always_thinking, image_in, video_in, tool_use and
// dynamically_loaded_tools, and starts K2.8 on max; through magpie they
// had thinking, image_in and tool_use, a thinking switch Kimi refuses to
// turn off, and high. A Kimi Code plan model is now told what Kimi's list
// says of it (Kimi Code 2.4.1's managed-kimi-code.ts capabilitiesForModel)
// but video_in, which magpie's gateway can't carry. Another vendor's model
// is told what it was, and kimi-cli, which knows none of the new words,
// gets none of them.
func TestKimiCodePlanCapabilities(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("KIMI_SHARE_DIR", "")
	t.Setenv("KIMI_CODE_HOME", "")

	// Kimi's /coding/v1/models as it answered on 2026-10-31, read the
	// way magpie reads it, with the levels models.dev's catalog gives
	// (Decorate)
	list, err := os.ReadFile("../catalog/testdata/kimi-code-cn-models-2026-10-31.json")
	if err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) { rw.Write(list) }))
	defer srv.Close()
	ms, _, err := catalog.FetchAt(context.Background(), srv.URL+"/coding/v1", "k", false, nil)
	if err != nil {
		t.Fatal(err)
	}
	ms = catalog.Decorate(ms, []catalog.Model{
		{ID: "kimi-for-coding", Efforts: []string{"none", "low", "high", "max"}, Images: true},
		{ID: "k3.1-pro", Efforts: []string{"low", "high", "max"}, Images: true},
	})
	base := "https://api.kimi.com/coding/v1"
	if err := provider.Save(provider.Provider{ID: "kimi-code-cn", Name: "Kimi Code", Preset: "kimi-code-cn", Chat: base, Key: "k",
		Models: []string{"kimi-for-coding", "kimi-for-coding-highspeed", "k3.1-pro"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("kimi-code-cn", base, ms); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Chat: "https://example.test/v1", Key: "k", Models: []string{"gemini-3-pro"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("relay", "https://example.test/v1", []catalog.Model{
		{ID: "gemini-3-pro", Efforts: []string{"low", "high"}, Images: true, InputModalities: []string{"text", "image", "video"}},
	}); err != nil {
		t.Fatal(err)
	}
	// jiabubu again: one Kimi model put in a group so it lists first keeps
	// what its list says; a group with another vendor's model has none
	if err := provider.SaveGroup(provider.Group{ID: "top", Name: "Top", Members: []string{"kimi-code-cn/kimi-for-coding"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "mix", Name: "Mix", Members: []string{"kimi-code-cn/kimi-for-coding", "relay/gemini-3-pro"}}); err != nil {
		t.Fatal(err)
	}

	type model struct {
		Capabilities   []string `toml:"capabilities"`
		SupportEfforts []string `toml:"support_efforts"`
		DefaultEffort  string   `toml:"default_effort"`
	}
	read := func(path string) map[string]model {
		t.Helper()
		b, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var c struct{ Models map[string]model }
		if err := toml.Unmarshal(b, &c); err != nil {
			t.Fatalf("%v\n%s", err, b)
		}
		return c.Models
	}
	path := filepath.Join(home, ".kimi-code", "config.toml")
	writeFile(t, path, "")
	if err := kimi(home).Field("model").Set("magpie/kimi-code-cn/kimi-for-coding"); err != nil {
		t.Fatal(err)
	}
	want := map[string]model{
		"magpie/kimi-code-cn/kimi-for-coding": {[]string{"thinking", "always_thinking", "image_in", "tool_use", "dynamically_loaded_tools"},
			[]string{"low", "high", "max"}, "max"},
		// its list says it always thinks and loads no tools, with no levels
		"magpie/kimi-code-cn/kimi-for-coding-highspeed": {[]string{"thinking", "always_thinking", "image_in", "tool_use"}, nil, ""},
		"magpie/kimi-code-cn/k3.1-pro": {[]string{"thinking", "always_thinking", "image_in", "tool_use", "dynamically_loaded_tools"},
			[]string{"low", "high", "max"}, "high"},
		// another vendor's model, video and all, as it was
		"magpie/relay/gemini-3-pro": {[]string{"thinking", "image_in", "tool_use"}, []string{"low", "high"}, "high"},
		"magpie/group/top": {[]string{"thinking", "always_thinking", "image_in", "tool_use", "dynamically_loaded_tools"},
			[]string{"low", "high", "max"}, "max"},
		"magpie/group/mix": {[]string{"thinking", "image_in", "tool_use"}, []string{"low", "high"}, "high"},
	}
	got := read(path)
	for k, w := range want {
		if g := got[k]; !reflect.DeepEqual(g, w) {
			t.Errorf("%s: %+v, want %+v", k, g, w)
		}
	}
	if b, _ := os.ReadFile(path); strings.Contains(string(b), "video_in") {
		t.Errorf("video_in written, which the gateway can't carry:\n%s", b)
	}

	// the user's own pick on K2.8 stays; K3.1 Pro's high, written by a
	// magpie that kept no record of it, is the vendor's default too
	b, _ := os.ReadFile(path)
	at := strings.Index(string(b), `[models."magpie/kimi-code-cn/kimi-for-coding"]`)
	s := string(b[:at]) + strings.Replace(string(b[at:]), `default_effort = "max"`, `default_effort = "low"`, 1)
	writeFile(t, path, s)
	if err := kimi(home).Field("model").Set("magpie/kimi-code-cn/kimi-for-coding"); err != nil {
		t.Fatal(err)
	}
	if g := read(path)["magpie/kimi-code-cn/kimi-for-coding"].DefaultEffort; g != "low" {
		t.Errorf("the user's default effort on K2.8: %q, want low", g)
	}

	// kimi-cli has none of the new words
	os.RemoveAll(filepath.Join(home, ".kimi-code"))
	legacy := filepath.Join(home, ".kimi", "config.toml")
	writeFile(t, legacy, "")
	if err := kimi(home).Field("model").Set("magpie/kimi-code-cn/kimi-for-coding"); err != nil {
		t.Fatal(err)
	}
	if g := read(legacy)["magpie/kimi-code-cn/kimi-for-coding"]; !reflect.DeepEqual(g.Capabilities, []string{"thinking", "image_in"}) || g.DefaultEffort != "" {
		t.Errorf("kimi-cli: %+v", g)
	}
}

// A Kimi Code config written by a magpie before this one has high on K2.8
// with no record that magpie wrote it: it becomes the vendor's max. One
// the user picked after a record was kept stays (TestKimiCodePlanCapabilities).
func TestKimiCodeOldHighBecomesTheVendorsDefault(t *testing.T) {
	kvs, def := kimiEfforts([]string{"none", "low", "high", "max"}, "max", "high", map[string]string{"display_name": "K2.8 Preview"})
	if def != "max" || kvs[len(kvs)-1].Value != "max" {
		t.Errorf("unrecorded high: %v, %q", kvs, def)
	}
	kvs, def = kimiEfforts([]string{"none", "low", "high", "max"}, "max", "high", map[string]string{"default_effort": "max"})
	if def != "" || kvs[len(kvs)-1].Value != "high" {
		t.Errorf("the user's high: %v, %q", kvs, def)
	}
	kvs, def = kimiEfforts([]string{"low", "high"}, "max", "", nil)
	if def != "high" || kvs[len(kvs)-1].Value != "high" {
		t.Errorf("a default the model hasn't: %v, %q", kvs, def)
	}
}
