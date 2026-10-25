package gateway

import (
	"testing"
)

// TestAdaptiveOnOutputEffortModelAsksItsLevel: DSH asks a model that takes
// its level in output_config.effort (zcode's GLM-5.3) adaptively, so that
// its Max is not clamped to a 16384 budget (01huadalang on Discord, 10-24).
// The level goes upstream as asked, max as max, thinking still adaptive
// (Z.ai took it so, live), and the Routing log and usage say the level. A
// budget alone is still read as before, 16384 as high.
func TestAdaptiveOnOutputEffortModelAsksItsLevel(t *testing.T) {
	t.Setenv("FAKE_ZCODE", "1")
	up := &adaptiveVendor{}
	pid := besideFake(t, "zcode", up)
	for _, tc := range []struct {
		thinking, effort string
		maxTokens        int
		want, kind       string
	}{
		{`{"type":"adaptive","display":"summarized"}`, "max", 128000, "max", "adaptive"},
		{`{"type":"adaptive","display":"summarized"}`, "high", 128000, "high", "adaptive"},
		{`{"type":"adaptive","display":"summarized"}`, "low", 128000, "low", "adaptive"},
		{`{"type":"adaptive","display":"summarized"}`, "max", 8000, "max", "adaptive"},
		{`{"type":"enabled","budget_tokens":16384,"display":"summarized"}`, "", 128000, "high", "enabled"},
	} {
		oc := ""
		if tc.effort != "" {
			oc = `"output_config":{"effort":"` + tc.effort + `"},`
		}
		body := `{"model":"` + pid + `/GLM-5.3","max_tokens":` + itoa(tc.maxTokens) + `,"thinking":` + tc.thinking + `,` + oc + `"stream":true,"messages":[{"role":"user","content":"hi"}]}`
		rt := postProto(t, New(), "/v1/messages", body)
		got := up.last()
		th, _ := got["thinking"].(map[string]any)
		ocu, _ := got["output_config"].(map[string]any)
		if rt.Effort != tc.want || rt.Tries[len(rt.Tries)-1].Effort != tc.want || ocu["effort"] != tc.want {
			t.Errorf("%s %s: route %q, try %q, upstream output_config %v; want %s", tc.thinking, tc.effort, rt.Effort, rt.Tries[len(rt.Tries)-1].Effort, got["output_config"], tc.want)
		}
		if th["type"] != tc.kind || got["max_tokens"] != float64(tc.maxTokens) {
			t.Errorf("%s %s: upstream thinking %v at max_tokens %v, want %s at %d", tc.thinking, tc.effort, got["thinking"], got["max_tokens"], tc.kind, tc.maxTokens)
		}
		if u := lastUsage(t).Effort; u != tc.want {
			t.Errorf("%s %s: usage effort %q, want %s", tc.thinking, tc.effort, u, tc.want)
		}
	}
}
