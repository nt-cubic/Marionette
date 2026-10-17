package agent

// Codex's background compaction (0.163 on, #2490): with
// [features.async_compaction] enabled, Codex sends a compaction_trigger of
// its own while a turn goes on, once the conversation is past
// model_async_compact_threshold_percent of the model's window (in that
// table; one at the top level is not read), and takes the summary in at the
// turn's next step. It starts one only on a provider whose
// capabilities.remote_compaction is "v2", which Codex gives OpenAI's own and
// Azure alone: on magpie's [model_providers.magpie] it never ran until that
// was written there too. magpie's gateway answers the trigger as it does
// Codex's own compaction (codexCompact, or the provider's native one).
//
// What magpie writes is kept in the stash, so a value the user set there
// (by hand, or a value they changed since) is theirs: never written over,
// and left when the setting is turned off.

import (
	"encoding/json"
	"errors"

	"github.com/pelletier/go-toml/v2"

	"github.com/yetone/magpie/internal/edit"
)

const (
	codexAsyncTable   = "features.async_compaction"
	codexAsyncPercent = "model_async_compact_threshold_percent"
)

// codexAsyncStash is what magpie wrote.
type codexAsyncStash struct {
	Enabled bool `json:"enabled,omitempty"` // enabled = true
	Percent int  `json:"percent,omitempty"` // the threshold
	Cap     bool `json:"cap,omitempty"`     // capabilities = { remote_compaction = "v2" }
}

// codexAsyncNow is how the config has them: the async_compaction value
// (nil, a bool, or a table) and magpie's provider table, nil when absent.
func codexAsyncNow(path string) (feature any, provider map[string]any, err error) {
	raw, err := edit.Read(path)
	if err != nil || raw == nil {
		return nil, nil, err
	}
	var doc map[string]any
	if err := toml.Unmarshal(raw, &doc); err != nil {
		return nil, nil, err
	}
	features, _ := doc["features"].(map[string]any)
	providers, _ := doc["model_providers"].(map[string]any)
	p, _ := providers[magpieID].(map[string]any)
	return features["async_compaction"], p, nil
}

// codexAsyncSync puts the setting into the Codex config at path, where
// Codex is routed through magpie or not: on is the setting, percent its
// threshold (0 leaves Codex's own). key is the stash's. The capability is
// written for a feature the user turned on in the config themselves too:
// on magpie's provider it would never run without it (#2493).
func codexAsyncSync(path, key string, routed, on bool, percent int) error {
	feature, provider, err := codexAsyncNow(path)
	var bad *toml.DecodeError
	if errors.As(err, &bad) {
		return nil // one that doesn't parse is told by Check, and written to by nothing
	}
	if err != nil {
		return err
	}
	var st codexAsyncStash
	if raw := stashLoad()[key]; raw != "" {
		json.Unmarshal([]byte(raw), &st)
	}
	was := st
	want := routed && on
	if !want {
		percent = 0
	}

	// [features.async_compaction]: written only as that table, which a
	// config without the feature, or with the table, has. The bare
	// async_compaction = true, or the table spelled inline or as dotted
	// keys, is the user's, as written.
	table, _ := feature.(map[string]any)
	asTable := feature == nil
	if table != nil {
		t, err := edit.GetTOMLTable(path, codexAsyncTable)
		if err != nil {
			return err
		}
		asTable = t != nil
	}
	if asTable {
		enabled, hasEnabled := table["enabled"].(bool)
		if table["enabled"] != nil && !(hasEnabled && st.Enabled && enabled) {
			st.Enabled = false // set, or changed since, by the user
		}
		switch {
		case want && !hasEnabled && table["enabled"] == nil:
			if err := edit.SetTOMLKey(path, codexAsyncTable, "enabled", true); err != nil {
				return err
			}
			st.Enabled = true
		case !want && st.Enabled:
			if err := edit.DelTOMLKey(path, codexAsyncTable, "enabled"); err != nil {
				return err
			}
			st.Enabled = false
		}
		cur, hasPercent := table[codexAsyncPercent].(int64)
		if table[codexAsyncPercent] != nil && !(hasPercent && int64(st.Percent) == cur) {
			st.Percent = 0
		}
		switch {
		case percent > 0 && (table[codexAsyncPercent] == nil || st.Percent != 0):
			if st.Percent != percent {
				if err := edit.SetTOMLKey(path, codexAsyncTable, codexAsyncPercent, percent); err != nil {
					return err
				}
			}
			st.Percent = percent
		case percent == 0 && st.Percent != 0:
			if err := edit.DelTOMLKey(path, codexAsyncTable, codexAsyncPercent); err != nil {
				return err
			}
			st.Percent = 0
		}
	} else {
		st.Enabled, st.Percent = false, 0
	}

	// capabilities on magpie's provider table, when there is one: one the
	// user gave it, whatever it says, stays as it is
	if feature, _, err = codexAsyncNow(path); err != nil {
		return err
	}
	table, _ = feature.(map[string]any)
	if feature == true || table["enabled"] == true {
		want = routed
	}
	if provider == nil {
		st.Cap = false
	} else {
		caps, has := provider["capabilities"].(map[string]any)
		if has && !(st.Cap && len(caps) == 1 && caps["remote_compaction"] == "v2") {
			st.Cap = false
		}
		switch {
		case want && provider["capabilities"] == nil:
			if err := edit.SetTOMLKey(path, "model_providers."+magpieID, "capabilities", edit.Inline{{Path: "remote_compaction", Value: "v2"}}); err != nil {
				return err
			}
			st.Cap = true
		case !want && st.Cap:
			if err := edit.DelTOMLKey(path, "model_providers."+magpieID, "capabilities"); err != nil {
				return err
			}
			st.Cap = false
		}
	}

	if st != was {
		if st == (codexAsyncStash{}) {
			forget(key)
		} else {
			b, _ := json.Marshal(st)
			stash(map[string]string{key: string(b)})
		}
	}
	return nil
}

// codexProviderCaps is the capabilities magpie's provider table has, as an
// Inline to write it back with when the table is written again: one the
// user gave it, or magpie's for the background compaction, isn't lost.
func codexProviderCaps(path string) edit.Inline {
	_, provider, err := codexAsyncNow(path)
	if err != nil {
		return nil
	}
	caps, _ := provider["capabilities"].(map[string]any)
	var out edit.Inline
	for _, k := range sortedKeys(caps) {
		switch v := caps[k].(type) {
		case string, bool, int64, float64:
			out = append(out, edit.KV{Path: k, Value: v})
		}
	}
	return out
}
