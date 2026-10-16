package provider

import (
	"slices"
	"testing"
)

// Amazon Bedrock's Mantle endpoint (@marktoown on X): its own preset, its
// own regions, its models listed, and a provider added at a Mantle address
// taken for Bedrock's too.
func TestBedrockMantlePreset(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	p, err := FromPreset("bedrock-mantle")
	if err != nil {
		t.Fatal(err)
	}
	if p.Chat != "https://bedrock-mantle.us-east-1.api.aws/v1" || p.Responses != p.Chat ||
		p.Anthropic != "https://bedrock-mantle.us-east-1.api.aws/anthropic" {
		t.Fatalf("endpoints: %q %q %q", p.Chat, p.Responses, p.Anthropic)
	}
	if !p.IsBedrock() || !p.IsBedrockMantle() || p.Icon != "bedrock-color" {
		t.Fatalf("not taken for Bedrock Mantle: %+v", p)
	}
	pr := Preset("bedrock-mantle")
	if pr.NoList || len(pr.Models) != 0 || !p.lists() {
		t.Fatalf("Mantle lists its models at /v1/models: NoList %v, models %v", pr.NoList, pr.Models)
	}
	if pr.Regions[0].Chat != pr.Chat || pr.Regions[0].Anthropic != pr.Anthropic {
		t.Fatalf("the first region isn't the default: %+v", pr.Regions[0])
	}
	i := slices.IndexFunc(pr.Regions, func(r Region) bool { return r.ID == "ap-southeast-1" })
	if i < 0 || pr.Regions[i].Chat != "https://bedrock-mantle.ap-southeast-1.api.aws/v1" ||
		pr.Regions[i].Responses != pr.Regions[i].Chat ||
		pr.Regions[i].Anthropic != "https://bedrock-mantle.ap-southeast-1.api.aws/anthropic" {
		t.Fatalf("Singapore: %+v", pr.Regions)
	}
	// eu-west-3 has no Mantle host (it didn't resolve on 2026-10-16)
	if slices.ContainsFunc(pr.Regions, func(r Region) bool { return r.ID == "eu-west-3" }) {
		t.Fatalf("a region without Mantle: %+v", pr.Regions)
	}
	// the runtime's preset is left as it was
	if rt := Preset("bedrock"); !rt.NoList {
		t.Fatal("the runtime preset lists now")
	}
	rt, _ := FromPreset("bedrock")
	if rt.IsBedrockMantle() {
		t.Fatal("the runtime taken for Mantle")
	}

	// Claude on Messages alone; OpenAI's GPTs on Responses and chat;
	// the rest on chat
	for model, want := range map[string][]Protocol{
		"anthropic.claude-opus-4-7": {Anthropic},
		"openai.gpt-5.5":            {Responses, Chat},
		"openai.gpt-oss-120b":       {Chat},
		"google.gemma-3-27b-it":     {Chat},
	} {
		if got := p.APIs(model); !slices.Equal(got, want) {
			t.Errorf("%s: APIs %v, want %v", model, got, want)
		}
	}
	// the key as x-api-key alone, as Mantle's own examples send it
	p.Key = "ABSK-test"
	if h := AuthHeaders(p, Anthropic); len(h) != 1 || h["x-api-key"] != "ABSK-test" {
		t.Errorf("Messages headers: %v", h)
	}

	// a model's test request is asked at the prefix that serves it
	for _, tc := range []struct {
		proto      Protocol
		model, url string
	}{
		{Chat, "openai.gpt-oss-120b", "https://bedrock-mantle.us-east-1.api.aws/v1/chat/completions"},
		{Chat, "openai.gpt-5.5", "https://bedrock-mantle.us-east-1.api.aws/openai/v1/chat/completions"},
		{Responses, "openai.gpt-5.5", "https://bedrock-mantle.us-east-1.api.aws/openai/v1/responses"},
		{Responses, "xai.grok-4.3", "https://bedrock-mantle.us-east-1.api.aws/openai/v1/responses"},
		{Anthropic, "anthropic.claude-opus-4-7", "https://bedrock-mantle.us-east-1.api.aws/anthropic/v1/messages"},
	} {
		if u, _ := tinyBody(p, tc.proto, tc.model); u != tc.url {
			t.Errorf("%s on %s: %s, want %s", tc.model, tc.proto, u, tc.url)
		}
	}
}

// A provider added by its address at a Mantle host is Mantle's: Bedrock's
// quirks, and the APIs it wasn't given filled in.
func TestBedrockMantleByAddress(t *testing.T) {
	for _, tc := range []struct {
		name                 string
		in                   Provider
		chat, resp, messages string
	}{
		{"chat at /v1", Provider{ID: "m", Chat: "https://bedrock-mantle.eu-west-1.api.aws/v1"},
			"https://bedrock-mantle.eu-west-1.api.aws/v1", "https://bedrock-mantle.eu-west-1.api.aws/v1", "https://bedrock-mantle.eu-west-1.api.aws/anthropic"},
		{"chat at /openai/v1", Provider{ID: "m", Chat: "https://bedrock-mantle.eu-west-1.api.aws/openai/v1/"},
			"https://bedrock-mantle.eu-west-1.api.aws/v1", "https://bedrock-mantle.eu-west-1.api.aws/v1", "https://bedrock-mantle.eu-west-1.api.aws/anthropic"},
		{"messages alone", Provider{ID: "m", Anthropic: "https://bedrock-mantle.us-west-2.api.aws/anthropic"},
			"https://bedrock-mantle.us-west-2.api.aws/v1", "https://bedrock-mantle.us-west-2.api.aws/v1", "https://bedrock-mantle.us-west-2.api.aws/anthropic"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			p := normalize(tc.in)
			if !p.IsBedrock() || !p.IsBedrockMantle() {
				t.Fatalf("not taken for Mantle: %+v", p)
			}
			if p.Chat != tc.chat || p.Responses != tc.resp || p.Anthropic != tc.messages {
				t.Fatalf("endpoints: %q %q %q", p.Chat, p.Responses, p.Anthropic)
			}
		})
	}
	for _, other := range []Provider{
		{ID: "o", Chat: "https://bedrock-mantle.example.com/v1"},
		{ID: "o", Chat: "https://api.aws/v1"},
		{ID: "o", Chat: "https://relay.example.com/bedrock-mantle.us-east-1.api.aws/v1"},
	} {
		if p := normalize(other); p.IsBedrockMantle() || p.IsBedrock() || p.Anthropic != "" {
			t.Errorf("%s taken for Mantle: %+v", other.Chat, p)
		}
	}
	// another preset at a Mantle address keeps its own APIs
	if p := normalize(Provider{ID: "c", Preset: "openrouter", Chat: "https://bedrock-mantle.us-east-1.api.aws/v1"}); p.Anthropic != "" {
		t.Errorf("filled in for another preset: %+v", p)
	}
}

// Mantle serves each OpenAI-style model at one of two prefixes: OpenAI's
// closed GPTs and xAI's Grok at /openai/v1, every other at /v1.
func TestMantleBase(t *testing.T) {
	const v1, oa = "https://bedrock-mantle.us-east-1.api.aws/v1", "https://bedrock-mantle.us-east-1.api.aws/openai/v1"
	for _, tc := range []struct {
		base, model string
		other       bool
		want        string
	}{
		{v1, "openai.gpt-oss-120b", false, v1},
		{v1, "google.gemma-3-27b-it", false, v1},
		{v1, "openai.gpt-5.5", false, oa},
		{v1, "openai.gpt-6-sol", false, oa},
		{v1, "xai.grok-4.3", false, oa},
		{oa, "openai.gpt-oss-120b", false, v1},
		{oa + "/", "xai.grok-4.3", false, oa},
		// learned to be at the other one
		{v1, "newmaker.model-1", true, oa},
		{v1, "openai.gpt-5.5", true, v1},
		// a URL the user typed some other way is theirs
		{"https://proxy.example.com/mantle", "openai.gpt-5.5", false, "https://proxy.example.com/mantle"},
	} {
		if got := MantleBase(tc.base, tc.model, tc.other); got != tc.want {
			t.Errorf("MantleBase(%q, %q, %v) = %q, want %q", tc.base, tc.model, tc.other, got, tc.want)
		}
	}
}
