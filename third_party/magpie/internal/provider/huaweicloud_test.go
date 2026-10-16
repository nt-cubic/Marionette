package provider

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

func TestHuaweiCloudTokenPlan(t *testing.T) {
	p, err := FromPreset("huaweicloud")
	if err != nil {
		t.Fatal(err)
	}
	// the Token Plan's own endpoints by default: its quota isn't spent at
	// MaaS's pay-as-you-go ones; no Responses, which neither serves
	if p.Chat != "https://api.modelarts-maas.com/plan/v2" || p.Anthropic != "https://api.modelarts-maas.com/plan/anthropic" || p.Responses != "" {
		t.Fatalf("endpoints: %q %q %q", p.Chat, p.Responses, p.Anthropic)
	}
	pr := Preset("huaweicloud")
	if len(pr.Regions) != 2 || pr.Regions[0].Chat != pr.Chat || pr.Regions[0].Anthropic != pr.Anthropic ||
		pr.Regions[1].Chat != "https://api.modelarts-maas.com/openai/v1" || pr.Regions[1].Anthropic != "https://api.modelarts-maas.com/anthropic" {
		t.Fatalf("plans: %+v", pr.Regions)
	}
	for _, id := range pr.Models {
		if id != strings.ToLower(id) {
			t.Fatalf("model ids are lowercase, as its OpenClaw page has them: %q", id)
		}
	}
	// before its list is fetched: the plan's models
	if got := p.planModels(nil); len(got) != len(pr.Models) || got[0].ID != "glm-5.3" {
		t.Fatalf("plan's: %+v", got)
	}
	if got := p.planModels([]catalog.Model{{ID: "glm-5.1"}}); len(got) != 1 {
		t.Fatalf("listed: %+v", got)
	}
	// an entry imported from another app at the plan's endpoints is the preset
	im, _ := imported("Huawei", "k", endpoints{anthropic: "https://api.modelarts-maas.com/plan/anthropic"}, nil)
	if im.Preset != "huaweicloud" || im.Icon != "huaweicloud-color" {
		t.Fatalf("imported: %+v", im)
	}
}
