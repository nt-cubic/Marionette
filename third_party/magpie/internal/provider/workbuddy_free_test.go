package provider

import (
	"encoding/json"
	"testing"
)

// A model WorkBuddy's picker shows at x0.00 credits is marked free; any
// other rate, or none, is not.
func TestWBFreeCredits(t *testing.T) {
	var cfg wbProductConfig
	if err := json.Unmarshal([]byte(`{"agents":[{"name":"cli","models":["deepseek-v4.1-flash","deepseek-v4.1-flash-sg","gpt-5.5","glm-5.3","hy3"]}],
	 "models":[{"id":"deepseek-v4.1-flash","name":"Deepseek-V4.1-Flash","credits":"x0.00"},
	  {"id":"deepseek-v4.1-flash-sg","name":"Deepseek-V4.1-Flash","credits":"x0.03"},
	  {"id":"gpt-5.5","name":"GPT-5.5","credits":"x1.00"},
	  {"id":"glm-5.3","name":"GLM-5.3","credits":0},
	  {"id":"hy3","name":"Hy3"}]}`), &cfg); err != nil {
		t.Fatal(err)
	}
	want := map[string]bool{"deepseek-v4.1-flash": true, "glm-5.3": true}
	for _, m := range cfg.cliModels() {
		if m.Free != want[m.ID] {
			t.Errorf("%s free = %v", m.ID, m.Free)
		}
	}
}

// Each model carries the credits WorkBuddy's picker shows by it, so the
// lists can say it (01huadalang on Discord: pick the cheap ones without
// opening WorkBuddy): x0.03 is 0.03, x1.00 is 1; free and unreadable
// rates give none.
func TestWBCreditsRate(t *testing.T) {
	var cfg wbProductConfig
	if err := json.Unmarshal([]byte(`{"agents":[{"name":"cli","models":["deepseek-v4.1-flash","deepseek-v4.1-flash-sg","gpt-5.5","kimi-k3","hy3","odd","inf"]}],
	 "models":[{"id":"deepseek-v4.1-flash","credits":"x0.00"},
	  {"id":"deepseek-v4.1-flash-sg","credits":"x0.03"},
	  {"id":"gpt-5.5","credits":"X1.00"},
	  {"id":"kimi-k3","credits":1.5},
	  {"id":"hy3"},
	  {"id":"odd","credits":"lots"},
	  {"id":"inf","credits":"x+Inf"}]}`), &cfg); err != nil {
		t.Fatal(err)
	}
	want := map[string]float64{"deepseek-v4.1-flash-sg": 0.03, "gpt-5.5": 1, "kimi-k3": 1.5}
	ms := cfg.cliModels()
	if len(ms) != 7 {
		t.Fatalf("models %+v", ms)
	}
	for _, m := range ms {
		if m.Rate != want[m.ID] || m.Free != (m.ID == "deepseek-v4.1-flash") {
			t.Errorf("%s: Rate = %v, Free = %v", m.ID, m.Rate, m.Free)
		}
	}
}
