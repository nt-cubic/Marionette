package otlp

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

func TestOTelGenerationCosts(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.CachePath(), []byte(`{"anthropic":{"id":"anthropic","models":{"claude-sonnet-5":{"id":"claude-sonnet-5","cost":{"input":3,"output":15,"cache_read":0.3,"cache_write":3.75}}}}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	price := func(in, out, read, write float64) settings.ModelPrice {
		return settings.ModelPrice{Input: &in, Output: &out, CacheRead: &read, CacheWrite: &write}
	}
	config := settings.Settings{ModelPrices: map[string]settings.ModelPrice{
		"relay/claude-sonnet-5": price(2, 8, 0.2, 2.5),
		"free/*":                price(0, 0, 0, 0),
	}}
	if err := settings.Save(config); err != nil {
		t.Fatal(err)
	}
	base := Record{Time: time.Now(), Provider: "relay", Model: "claude-sonnet-5", Input: 1000, Output: 200, CacheRead: 4000, CacheWrite: 500, Reasoning: 100, Status: 200}
	exporter := &otelExporter{}
	for _, tc := range []struct {
		name, provider, model string
		context               *OTelSpan
		status                int
		rejected, omit        bool
		input, output         float64
	}{
		{name: "legacy generation", input: 0.00405, output: 0.0016},
		{name: "gateway attempt", context: &OTelSpan{}, input: 0.00405, output: 0.0016},
		{name: "billed failure", context: &OTelSpan{}, status: 502, input: 0.00405, output: 0.0016},
		{name: "local session generation", provider: usage.UnknownProvider, context: &OTelSpan{Session: true, Type: "generation"}, input: 0.006075, output: 0.003},
		{name: "free model", provider: "free"},
		{name: "unknown price", model: "magpie-unpriced-test-model", omit: true},
		{name: "gateway parent", context: &OTelSpan{Root: true}, omit: true},
		{name: "conversation parent", context: &OTelSpan{Session: true, Type: "agent"}, omit: true},
		{name: "tool", context: &OTelSpan{Session: true, Type: "tool"}, omit: true},
		{name: "local rejection", rejected: true, omit: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r := base
			r.Rejected = tc.rejected
			if tc.provider != "" {
				r.Provider = tc.provider
			}
			if tc.model != "" {
				r.Model = tc.model
			}
			if tc.status != 0 {
				r.Status = tc.status
			}
			// Decode the serialized OTLP envelope, including the nested JSON
			// string Langfuse expects for costs, rather than testing helpers.
			body, err := json.Marshal(exporter.traces([]otelItem{{record: r, span: tc.context}}))
			if err != nil {
				t.Fatal(err)
			}
			var envelope struct {
				Resources []struct {
					Scopes []struct {
						Spans []struct {
							Attributes []otelAttribute `json:"attributes"`
						} `json:"spans"`
					} `json:"scopeSpans"`
				} `json:"resourceSpans"`
			}
			if err := json.Unmarshal(body, &envelope); err != nil {
				t.Fatal(err)
			}
			var costs []string
			for _, attr := range envelope.Resources[0].Scopes[0].Spans[0].Attributes {
				if attr.Key == "langfuse.observation.cost_details" {
					costs = append(costs, attr.Value["stringValue"].(string))
				}
			}
			if tc.omit {
				if len(costs) != 0 {
					t.Fatalf("unexpected costs: %v", costs)
				}
				return
			}
			if len(costs) != 1 {
				t.Fatalf("want one cost attribute, got %v", costs)
			}
			var got map[string]float64
			if err := json.Unmarshal([]byte(costs[0]), &got); err != nil {
				t.Fatal(err)
			}
			if len(got) != 3 {
				t.Fatalf("cost breakdown: %v", got)
			}
			for key, want := range map[string]float64{"input": tc.input, "output": tc.output, "total": tc.input + tc.output} {
				if value, ok := got[key]; !ok || math.Abs(value-want) > 1e-12 {
					t.Errorf("%s cost = %v, want %v", key, value, want)
				}
			}
		})
	}
	// A later batch must pick up changed settings rather than retain tariffs
	// from the exporter's first batch.
	config.ModelPrices["relay/claude-sonnet-5"] = price(0, 0, 0, 0)
	if err := settings.Save(config); err != nil {
		t.Fatal(err)
	}
	body, err := json.Marshal(exporter.traces([]otelItem{{record: base}}))
	if err != nil {
		t.Fatal(err)
	}
	var got map[string]any
	if err := json.Unmarshal(body, &got); err != nil {
		t.Fatal(err)
	}
	resources := got["resourceSpans"].([]any)
	scopes := resources[0].(map[string]any)["scopeSpans"].([]any)
	spans := scopes[0].(map[string]any)["spans"].([]any)
	for _, a := range spans[0].(map[string]any)["attributes"].([]any) {
		attr := a.(map[string]any)
		if attr["key"] == "langfuse.observation.cost_details" {
			if attr["value"].(map[string]any)["stringValue"] != `{"input":0,"output":0,"total":0}` {
				t.Fatalf("stale tariff: %v", attr)
			}
			return
		}
	}
	t.Fatal("missing updated costs")
}
