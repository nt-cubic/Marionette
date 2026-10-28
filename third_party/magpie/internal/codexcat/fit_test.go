package codexcat

import (
	"encoding/json"
	"fmt"
	"os"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// The Codex app and its VS Code extension read the first AppList of
// model/list by priority and never the next page, hidden ones counted
// (@ZhenLuoTTAWDTT on X: "未被显示的模型也无法选择"). Fit puts a hidden
// entry after every listed one and, with more listed than AppList, brings
// the models Codex used into the first AppList, keeping the order
// otherwise.
func TestFitKeepsUsedModelsWhereTheCodexAppReads(t *testing.T) {
	var entries []any
	// Codex's own: two listed, three hidden near the top
	for i, s := range []string{"gpt-6", "codex-auto-review", "gpt-6-mini", "gpt-oss", "gpt-6-luna"} {
		v := "list"
		if i == 1 || i == 3 || i == 4 {
			v = "hide"
		}
		entries = append(entries, map[string]any{"slug": s, "priority": float64(i + 1), "visibility": v})
	}
	// magpie's 150, after them
	for i := range 150 {
		entries = append(entries, map[string]any{"slug": fmt.Sprintf("many/model-%03d", i), "priority": 106 + i, "visibility": "list"})
	}
	Fit(entries, []string{"many/model-140", "codex/gpt-6-luna", "many/model-003", "gone/model"})
	var order []string
	pri := map[string]int{}
	for i, e := range entries {
		o := e.(map[string]any)
		slug := o["slug"].(string)
		order = append(order, slug)
		pri[slug] = o["priority"].(int)
		if pri[slug] != i+1 {
			t.Fatalf("%s is at %d with priority %d", slug, i, pri[slug])
		}
	}
	first := order[:AppList]
	for _, s := range []string{"codex-auto-review", "gpt-oss", "gpt-6-luna"} {
		if slices.Contains(first, s) {
			t.Errorf("hidden %s is in the first %d", s, AppList)
		}
	}
	if !slices.Contains(first, "many/model-140") {
		t.Errorf("the used many/model-140 is at %d, past the first %d the Codex app reads", pri["many/model-140"], AppList)
	}
	// 2 of Codex's own listed and 98 of magpie's: 97 from the top, then the used one
	if want := append([]string{"gpt-6", "gpt-6-mini"}, order[2:99]...); !slices.Equal(first[:99], want) || order[98] != "many/model-096" || order[99] != "many/model-140" || order[100] != "many/model-097" {
		t.Errorf("the first 101: %v", order[:101])
	}
	if last := order[len(order)-3:]; !slices.Equal(last, []string{"codex-auto-review", "gpt-oss", "gpt-6-luna"}) {
		t.Errorf("the last three %v, want the hidden ones in their order", last)
	}
}

// A list the Codex app reads whole is left in its order, used or not.
func TestFitLeavesAShortListsOrder(t *testing.T) {
	var entries []any
	for i := range 20 {
		entries = append(entries, map[string]any{"slug": fmt.Sprintf("p/m%02d", i), "priority": i + 1, "visibility": "list"})
	}
	Fit(entries, []string{"p/m19", "p/m10"})
	for i, e := range entries {
		if s := e.(map[string]any)["slug"]; s != fmt.Sprintf("p/m%02d", i) {
			t.Fatalf("%d is %v", i, s)
		}
	}
}

// The model_catalog_json magpie writes Fits the same way.
func TestCatalogUsedFits(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	var ms []catalog.Model
	for i := range 130 {
		ms = append(ms, catalog.Model{ID: fmt.Sprintf("many/model-%03d", i)})
	}
	var got struct {
		Models []map[string]any `json:"models"`
	}
	if err := json.Unmarshal(CatalogUsed(ms, "", []string{"many/model-120"}), &got); err != nil {
		t.Fatal(err)
	}
	for _, m := range got.Models {
		if m["slug"] == "many/model-120" {
			if p := m["priority"].(float64); p > AppList {
				t.Fatalf("the used model's priority is %v", p)
			}
			return
		}
	}
	t.Fatal("many/model-120 is not in the catalog")
}

func TestUsedTagIsTheSet(t *testing.T) {
	if UsedTag([]string{"a", "b"}) != UsedTag([]string{"b", "a"}) {
		t.Error("the tag changed with the order alone")
	}
	if UsedTag([]string{"a"}) == UsedTag([]string{"a", "b"}) {
		t.Error("the tag didn't change with the set")
	}
}
