package gui

import (
	"encoding/json"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// The editor receives an authoritative list, even empty, so a Jev Router
// chat model on a remote isn't guessed to be a System One model.
func TestRemoteMagpieDecisionPicker(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "office", Preset: provider.RemoteMagpiePreset, Chat: "http://127.0.0.1:1", Key: "remote-key"}); err != nil {
		t.Fatal(err)
	}
	p, _ := provider.Find("office")
	for _, decisions := range [][]catalog.Model{
		{{ID: "judge/custom", Decides: true}},
		nil,
	} {
		ms := append([]catalog.Model{{ID: "relay/typesafe/jev-router"}}, decisions...)
		if err := catalog.SaveLive(p.ID, p.Chat, ms); err != nil {
			t.Fatal(err)
		}
		out := providerInfo(*p, nil)
		want := []string{"relay/typesafe/jev-router"}
		for _, m := range decisions {
			want = append(want, m.ID)
		}
		var got []string
		for _, m := range out.Models {
			got = append(got, m.ID)
		}
		if !slices.Equal(got, want) {
			t.Fatalf("editor models after refresh: %v; want %v", got, want)
		}
		if out.Deciders == nil || len(*out.Deciders) != len(decisions) {
			t.Fatalf("editor decision list: %v", out.Deciders)
		}
		if len(decisions) > 0 && !slices.Equal(*out.Deciders, []string{"judge/custom"}) {
			t.Fatalf("editor decisions: %v", *out.Deciders)
		}
		b, err := json.Marshal(out)
		if err != nil {
			t.Fatal(err)
		}
		var fields map[string]json.RawMessage
		json.Unmarshal(b, &fields)
		if len(decisions) == 0 && string(fields["deciders"]) != "[]" {
			t.Fatalf("empty decisions omitted: %s", b)
		}
		if out.Decide != "http://127.0.0.1:1/v1" || out.Chat != out.Decide {
			t.Fatalf("remote API bases: %+v", out)
		}
	}
}

// Bailian's Token Plan (#1506): the editor is told its one decision model
// beside the plan's chat models, so a routing group can pick it and no
// agent is offered it.
func TestBailianTokenPlanDecisionPicker(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	p, err := provider.FromPreset("bailian-token-plan")
	if err != nil {
		t.Fatal(err)
	}
	p.Key = "sk-sp-1"
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	saved, _ := provider.Find(p.ID)
	if err := catalog.SaveLive(saved.ID, saved.Chat, []catalog.Model{{ID: "qwen3.8-max"}, {ID: "glm-5.3"}}); err != nil {
		t.Fatal(err)
	}
	out := providerInfo(*saved, nil)
	if out.Deciders == nil || !slices.Equal(*out.Deciders, []string{provider.BailianDecision}) {
		t.Fatalf("editor decisions: %v", out.Deciders)
	}
	var got []string
	for _, m := range out.Models {
		got = append(got, m.ID)
	}
	if !slices.Equal(got, []string{"qwen3.8-max", "glm-5.3", provider.BailianDecision}) {
		t.Fatalf("editor models: %v", got)
	}
}
