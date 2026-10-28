package plugin

import (
	"context"
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// Vocllum (#2038): a plugin declares switches for its provider
// (auth.options) and reads the user's values. Only well-formed ones are
// listed, at most 16, their text capped; the plugin reads the user's value,
// else the default, from the host's start and again after each change,
// without a restart.
func TestPluginOptions(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	s := settings.Load()
	s.PluginOptions = map[string]map[string]bool{"optco": {"auto-claim": false}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	abs, _ := filepath.Abs("testdata/options")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	ps, err := Providers(ctx)
	if err != nil {
		t.Fatal(err)
	}
	by := map[string]Provider{}
	for _, p := range ps {
		by[p.ID] = p
	}
	if got := by["plain"].Options; len(got) != 0 {
		t.Errorf("plain's options = %+v, want none", got)
	}
	opts := by["optco"].Options
	if len(opts) != 16 {
		t.Fatalf("optco has %d options, want 16: %+v", len(opts), opts)
	}
	if opts[0] != (Option{Key: "auto-claim", Label: "Claim rewards", Description: "Claims the daily reward when usage is read.", Default: true}) {
		t.Errorf("first = %+v", opts[0])
	}
	if opts[1] != (Option{Key: "beta", Label: "Beta models", Confirm: "Beta models may be withdrawn."}) {
		t.Errorf("second = %+v", opts[1])
	}
	if o := opts[2]; o.Key != "long" || len([]rune(o.Label)) > 60 || len([]rune(o.Description)) > 300 || o.Default {
		t.Errorf("long = %q (%d), %d, default %v", o.Key, len([]rune(o.Label)), len([]rune(o.Description)), o.Default)
	}
	if opts[15].Key != "o12" {
		t.Errorf("last = %q, want o12", opts[15].Key)
	}
	read := func() string {
		t.Helper()
		got, err := Validate(ctx, "optco", 0, "probe", "")
		if err != nil {
			t.Fatal(err)
		}
		var v []any
		if json.Unmarshal([]byte(got), &v) != nil || len(v) != 3 {
			t.Fatalf("the plugin read %q", got)
		}
		m, _ := v[0].(map[string]any)
		return strings.Join([]string{jsonOf(m["auto-claim"]), jsonOf(m["beta"]), jsonOf(m["long"]), jsonOf(v[1]), jsonOf(v[2])}, " ")
	}
	// the user's off over the default's on; the rest their defaults
	if got := read(); got != "false false false false false" {
		t.Errorf("at start the plugin read %s", got)
	}
	s = settings.Load()
	s.PluginOptions = map[string]map[string]bool{"optco": {"beta": true}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if err := SetProviderOptions(ctx); err != nil {
		t.Fatal(err)
	}
	if got := read(); got != "true true false true false" {
		t.Errorf("after a change the plugin read %s", got)
	}
	if !OptionOn(s.PluginOptions, "optco", opts[0]) || !OptionOn(s.PluginOptions, "optco", opts[1]) || OptionOn(s.PluginOptions, "optco", opts[2]) {
		t.Error("OptionOn doesn't take the user's value, else the default")
	}
	// a models hook may list by a switch (WorkBuddy's "Free models only",
	// plugins#430): the providers kept are asked again after a change, so
	// the list follows the switch without a restart
	models := func() []string {
		t.Helper()
		Cached()
		Refreshed()
		for _, p := range Cached() {
			if p.ID == "optco" {
				var ids []string
				for _, m := range p.Models {
					ids = append(ids, m.ID)
				}
				return ids
			}
		}
		t.Fatal("optco isn't listed")
		return nil
	}
	if got := strings.Join(models(), " "); !strings.Contains(got, "beta-model") {
		t.Errorf("with beta on, optco lists %q, want beta-model", got)
	}
	s = settings.Load()
	s.PluginOptions = map[string]map[string]bool{"optco": {"beta": false}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if err := SetProviderOptions(ctx); err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(models(), " "); strings.Contains(got, "beta-model") || !strings.Contains(got, "base-model") {
		t.Errorf("with beta off, optco lists %q, want base-model alone", got)
	}
}

func jsonOf(v any) string {
	b, _ := json.Marshal(v)
	return string(b)
}
