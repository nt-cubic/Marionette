package provider

import "testing"

// Nous Portal (#392) is a vendor preset at its OpenAI-compatible inference
// API, chat completions only: its /v1/messages serves just the anthropic/*
// models, so it is no Anthropic endpoint for Claude Code's other picks.
func TestNousPortalPreset(t *testing.T) {
	p, err := FromPreset("nous")
	if err != nil {
		t.Fatal(err)
	}
	if p.Chat != "https://inference-api.nousresearch.com/v1" || p.Responses != "" || p.Anthropic != "" {
		t.Fatalf("endpoints: %q %q %q", p.Chat, p.Responses, p.Anthropic)
	}
	if p.Name != "Nous Portal" || p.Icon != "nousresearch" || p.KeysURL != "https://portal.nousresearch.com" {
		t.Fatalf("preset: %+v", p)
	}
	pr := Preset("nous")
	if pr.Kind != KindVendor || !pr.Hosts || pr.NoKey {
		t.Fatalf("kind %q hosts %v noKey %v", pr.Kind, pr.Hosts, pr.NoKey)
	}
	// an entry another app has at the Portal (Hermes Agent's base_url) is the preset
	im, why := imported("Nous", "k", endpoints{chat: "https://inference-api.nousresearch.com/v1/"}, nil)
	if why != "" || im.Preset != "nous" || im.Icon != "nousresearch" || im.Anthropic != "" {
		t.Fatalf("imported: %q %+v", why, im)
	}
}
