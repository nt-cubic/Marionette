package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// shin4 on plugins#316: an OpenAI client's effort for MiniMax Code's
// M3.1-Flash reached the plugin as a thinking budget alone, and under an
// 8192 cap medium to max were all budget 7168, so the level was lost. It
// goes in output_config.effort too, from Responses, Chat and an Anthropic
// client's budget, on both MiniMax Code providers. The requests are the
// reporter's, with the effort and the model's provider swapped in.
func TestMiniMaxPluginGetsTheNativeEffort(t *testing.T) {
	t.Setenv("FAKE_MINIMAX", "1")
	for _, id := range []string{"minimax-code", "minimax-code-global"} {
		t.Run(id, func(t *testing.T) {
			up := &adaptiveVendor{}
			pid := besideFake(t, id, up)
			for _, model := range []string{"MiniMax-M3.1-Flash", "MiniMax-M3.1-Flash-Preview"} {
				for _, c := range []struct {
					effort string
					budget float64
				}{{"low", 4096}, {"medium", 7168}, {"high", 7168}, {"xhigh", 7168}, {"max", 7168}} {
					for _, ask := range []struct{ path, body string }{
						{"/v1/responses", `{"model":"` + pid + `/` + model + `","input":"Reply exactly OK.","reasoning":{"effort":"` + c.effort + `"},"max_output_tokens":8192,"stream":false}`},
						{"/v1/chat/completions", `{"model":"` + pid + `/` + model + `","messages":[{"role":"user","content":"Reply exactly OK."}],"reasoning_effort":"` + c.effort + `","max_tokens":8192,"stream":false}`},
					} {
						postProto(t, New(), ask.path, ask.body)
						got := up.last()
						oc, _ := got["output_config"].(map[string]any)
						th, _ := got["thinking"].(map[string]any)
						if oc["effort"] != c.effort {
							t.Errorf("%s %s %s: output_config %v, thinking %v", ask.path, model, c.effort, got["output_config"], th)
						}
						if th["type"] != "enabled" || th["budget_tokens"] != c.budget {
							t.Errorf("%s %s %s: thinking %v, want a budget of %v beside the effort", ask.path, model, c.effort, th, c.budget)
						}
					}
				}
			}
			// an Anthropic client's budget, as Claude Code sends medium
			postProto(t, New(), "/v1/messages", `{"model":"`+pid+`/MiniMax-M3.1-Flash","max_tokens":32000,"thinking":{"type":"enabled","budget_tokens":10000},"messages":[{"role":"user","content":"Reply exactly OK."}]}`)
			if oc, _ := up.last()["output_config"].(map[string]any); oc["effort"] != "medium" {
				t.Errorf("/v1/messages budget 10000: output_config %v", up.last()["output_config"])
			}
			// M3, which thinks or doesn't and has no levels of its own,
			// keeps the budget alone
			postProto(t, New(), "/v1/responses", `{"model":"`+pid+`/MiniMax-M3","input":"Reply exactly OK.","reasoning":{"effort":"high"},"max_output_tokens":8192,"stream":false}`)
			if got := up.last(); got["output_config"] != nil {
				t.Errorf("MiniMax-M3 asked output_config %v", got["output_config"])
			}
		})
	}
}

// What goes to a Claude and to another vendor's model on an Anthropic
// endpoint is as before: the budget alone, no output_config.
func TestNativeEffortLeavesOthersAlone(t *testing.T) {
	fresh(t)
	up := &adaptiveVendor{}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Anthropic: srv.URL,
		Models: []string{"claude-sonnet-4-5", "kimi-k3", "MiniMax-M3"}}); err != nil {
		t.Fatal(err)
	}
	for _, model := range []string{"claude-sonnet-4-5", "kimi-k3", "MiniMax-M3"} {
		for _, ask := range []struct{ path, body string }{
			{"/v1/responses", `{"model":"relay/` + model + `","input":"Reply exactly OK.","reasoning":{"effort":"high"},"max_output_tokens":8192,"stream":false}`},
			{"/v1/chat/completions", `{"model":"relay/` + model + `","messages":[{"role":"user","content":"Reply exactly OK."}],"reasoning_effort":"high","max_tokens":8192,"stream":false}`},
		} {
			postProto(t, New(), ask.path, ask.body)
			got := up.last()
			b, _ := json.Marshal(map[string]any{"thinking": got["thinking"], "output_config": got["output_config"]})
			if want := `{"output_config":null,"thinking":{"budget_tokens":7168,"type":"enabled"}}`; string(b) != want {
				t.Errorf("%s %s: %s", ask.path, model, b)
			}
		}
	}
}
