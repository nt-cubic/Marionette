package agent

import (
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// taotao7 on Discord: omp on a group/* model counted every call at 0. omp's
// custom provider prices a call by the cost in its models.yml alone, and
// magpie wrote none, for any model; a group whose members cost differently
// had no price in magpie's list at all, so Pi, OpenCode and Crush got none
// either. A group is now priced at the member it sends to first, and that
// price follows the group's members.
func TestGroupModelsCarryTheirFirstMembersPrice(t *testing.T) {
	syncHome(t)
	f := func(v float64) *float64 { return &v }
	if err := settings.Save(settings.Settings{ModelPrices: map[string]settings.ModelPrice{
		"a/cheap": {Input: f(1), Output: f(2), CacheRead: f(0.1), CacheWrite: f(1.25)},
		"b/dear":  {Input: f(10), Output: f(20), CacheRead: f(1), CacheWrite: f(12.5)},
	}}); err != nil {
		t.Fatal(err)
	}
	for _, p := range []provider.Provider{
		{ID: "a", Name: "A", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"cheap"}},
		{ID: "b", Name: "B", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"dear"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	save := func(g provider.Group) {
		t.Helper()
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
		provider.Changed()
	}
	ompCostOf := func(id string) *ompCost {
		for _, m := range ompProviderAt("http://127.0.0.1:3425", "18.9.1").Models {
			if m.ID == id {
				return m.Cost
			}
		}
		t.Fatalf("omp has no %s", id)
		return nil
	}
	modelsOf := func(shape, id string) map[string]any {
		switch v := magpieProviderJSONAt(shape, shape, "http://127.0.0.1:3425").(map[string]any)["models"].(type) {
		case map[string]any:
			m, _ := v[id].(map[string]any)
			return m
		case []map[string]any:
			for _, m := range v {
				if m["id"] == id {
					return m
				}
			}
		}
		return nil
	}

	save(provider.Group{ID: "mix", Name: "Mix", Members: []string{"a/cheap", "b/dear"}})
	if c := ompCostOf("a/cheap"); c == nil || c.Input != 1 || c.Output != 2 || c.CacheRead != 0.1 || c.CacheWrite != 1.25 {
		t.Fatalf("omp's a/cheap cost %+v", c)
	}
	if c := ompCostOf("group/mix"); c == nil || c.Input != 1 || c.Output != 2 {
		t.Fatalf("omp's group/mix cost %+v, want a/cheap's", c)
	}
	if c, _ := modelsOf("pi", "group/mix")["cost"].(map[string]any); c["input"] != 1.0 || c["output"] != 2.0 {
		t.Fatalf("Pi's group/mix cost %v", c)
	}
	if c, _ := modelsOf("opencode", "group/mix")["cost"].(map[string]any); c["input"] != 1.0 || c["cache_read"] != 0.1 || c["cache_write"] != 1.25 {
		t.Fatalf("OpenCode's group/mix cost %v", c)
	}
	if m := modelsOf("crush", "group/mix"); m["cost_per_1m_in"] != 1.0 || m["cost_per_1m_out"] != 2.0 || m["cost_per_1m_in_cached"] != 1.25 || m["cost_per_1m_out_cached"] != 0.1 {
		t.Fatalf("Crush's group/mix %v", m)
	}

	// the members reordered, the price follows the new first
	save(provider.Group{ID: "mix", Name: "Mix", Members: []string{"b/dear", "a/cheap"}})
	if c := ompCostOf("group/mix"); c == nil || c.Input != 10 || c.Output != 20 {
		t.Fatalf("after the reorder omp's group/mix cost %+v, want b/dear's", c)
	}
	// a manual group is priced at its pick; the first switched off, at the next
	save(provider.Group{ID: "mix", Name: "Mix", Members: []string{"b/dear", "a/cheap"}, Routing: provider.Manual, Pick: "a/cheap"})
	if c := ompCostOf("group/mix"); c == nil || c.Input != 1 {
		t.Fatalf("manual pick a/cheap: omp's cost %+v", c)
	}
	save(provider.Group{ID: "mix", Name: "Mix", Members: []string{"b/dear", "a/cheap"}, Off: []string{"b/dear"}})
	if c := ompCostOf("group/mix"); c == nil || c.Input != 1 {
		t.Fatalf("b/dear off: omp's cost %+v", c)
	}
}
