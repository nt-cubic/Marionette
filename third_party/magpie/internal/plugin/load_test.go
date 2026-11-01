package plugin

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A plugin reports its models' loads through input.magpie.load (axelburks
// on Discord: an in-house vendor's busy models queue requests), when it
// likes, from its own timer: magpie keeps each, a percent from 0 up, past
// 100 for a vendor that queues (axelburks again: 200% happens), capped at
// LoadMost,
// matched by id whatever its case, and a null clears one. What isn't a
// number is dropped, and a call that isn't a provider and a map throws.
func TestPluginReportsModelLoad(t *testing.T) {
	sandbox(t)
	ForgetLoads()
	t.Cleanup(ForgetLoads)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	dir := filepath.Join(t.TempDir(), "loadco")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	src := `export default async (input) => {
  let threw = 0
  for (const bad of [[5, {}], ["loadco", null], ["loadco", [1]], ["", {}]]) {
    try { input.magpie.load(...bad) } catch (e) { if (e instanceof TypeError) threw++ }
  }
  const n = input.magpie.load("loadco", { Big: 87.5, small: 0, over: 250, huge: 1e9, under: -3, word: "busy", nan: NaN, inf: Infinity, threw })
  input.magpie.load("loadco", { taken: n })
  setTimeout(() => input.magpie.load("loadco", { small: null, later: 12 }), 50)
  return {}
}
`
	if err := os.WriteFile(filepath.Join(dir, "index.js"), []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Add(ctx, filepath.Join(dir, "index.js")); err != nil {
		t.Fatal(err)
	}
	if ls, err := Plugins(ctx); err != nil || len(ls) != 1 || ls[0].Error != "" {
		t.Fatalf("Plugins = %+v, %v", ls, err)
	}
	// the timer's call comes last: once it is in, so is everything before it
	for {
		if _, _, ok := ModelLoad("loadco", "later", time.Now()); ok {
			break
		}
		if ctx.Err() != nil {
			t.Fatal("the plugin's loads never arrived")
		}
		time.Sleep(10 * time.Millisecond)
	}
	now := time.Now()
	for model, want := range map[string]float64{"big": 87.5, "BIG": 87.5, "over": 250, "huge": LoadMost, "under": 0, "threw": 4, "taken": 6, "later": 12} {
		if got, _, ok := ModelLoad("loadco", model, now); !ok || got != want {
			t.Errorf("ModelLoad(%s) = %v, %v; want %v", model, got, ok, want)
		}
	}
	for _, model := range []string{"small", "word", "nan", "inf", "nobody"} {
		if got, _, ok := ModelLoad("loadco", model, now); ok {
			t.Errorf("ModelLoad(%s) = %v, want unknown", model, got)
		}
	}
	if _, _, ok := ModelLoad("other", "big", now); ok {
		t.Error("another provider's model has loadco's load")
	}
}

// A load counts for LoadFresh from the call that reported it, then is
// unknown, never taken for idle; a model@size has its model's load.
func TestModelLoadGoesStale(t *testing.T) {
	ForgetLoads()
	t.Cleanup(ForgetLoads)
	at := time.Date(2026, 11, 1, 12, 0, 0, 0, time.UTC)
	noteLoads("p", map[string]float64{"m": 80, "n": 10}, at)
	noteLoads("p", map[string]float64{"n": 20}, at.Add(4*time.Minute))
	if got, _, ok := ModelLoad("p", "m@256k", at.Add(LoadFresh)); !ok || got != 80 {
		t.Fatalf("m@256k at LoadFresh = %v, %v", got, ok)
	}
	later := at.Add(LoadFresh + time.Second)
	if got, _, ok := ModelLoad("p", "m", later); ok {
		t.Fatalf("m past LoadFresh = %v, want unknown", got)
	}
	if got, _, ok := ModelLoad("p", "n", later); !ok || got != 20 {
		t.Fatalf("n, reported again = %v, %v", got, ok)
	}
	noteLoads("p", map[string]float64{"n": -1}, later)
	if _, _, ok := ModelLoad("p", "n", later); ok {
		t.Fatal("a cleared load is still known")
	}
}
