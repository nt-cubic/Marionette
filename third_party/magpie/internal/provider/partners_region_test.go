package provider

import (
	"context"
	"os"
	"sync/atomic"
	"testing"
)

// A new provider of a partner whose regions are China's and Global's
// starts at the one where the user is (Kimi, 10-27: "伙伴卡编辑器默认区为
// China，建议按用户区域分流"): Kimi Code's Global plan abroad, its China plan
// in mainland China, with that plan's key page, site and catalog. A zone
// that can't be read keeps the list's first, a partner whose regions are
// no places (APIKEY.FAN's Default and Direct) keeps its own endpoints, and
// a provider added before keeps its region when the user moves. The feed
// is usemagpie.ai/api/partners as served on 2026-10-27.
func TestPartnerStartsAtTheUsersRegion(t *testing.T) {
	feed, err := os.ReadFile("testdata/partners-2026-10-27.json")
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	was, wasIcon := inMainlandChina, fetchPartnerIcon
	t.Cleanup(func() { inMainlandChina, fetchPartnerIcon = was, wasIcon; resetPartners() })
	fetchPartnerIcon = func(context.Context, string) (string, error) { return "", os.ErrNotExist }
	var body atomic.Value
	body.Store(string(feed))
	t.Setenv("MAGPIE_PARTNERS", partnerFeedServer(t, &body).URL)

	at := func(where int) Provider {
		t.Helper()
		inMainlandChina = func() int { return where }
		resetPartners()
		fetchPartnersNow()
		p, err := FromPreset("kimi")
		if err != nil {
			t.Fatal(err)
		}
		return p
	}
	type want struct{ chat, anthropic, keys, site, siteHost, catalog string }
	cn := want{"https://api.kimi.com/coding/v1", "https://api.kimi.com/coding", "https://usemagpie.ai/go/kimi/keys/cn", "https://usemagpie.ai/go/kimi/site/cn", "www.kimi.com", "kimi-code-plan-cn"}
	global := want{"https://api.kimi.ai/coding/v1", "https://api.kimi.ai/coding", "https://usemagpie.ai/go/kimi/keys/global", "https://usemagpie.ai/go/kimi/site/global", "www.kimi.ai", "kimi-code-plan-global"}
	for _, c := range []struct {
		name  string
		where int
		want  want
	}{
		{"abroad", -1, global},
		{"in mainland China", 1, cn},
		{"zone unknown", 0, cn},
	} {
		p := at(c.where)
		pr := Preset("kimi")
		got := want{p.Chat, p.Anthropic, p.KeysURL, p.Website, pr.SiteHost, p.Catalog}
		if got != c.want {
			t.Errorf("%s: a new Kimi Code provider is at %+v, want %+v", c.name, got, c.want)
		}
		// the editor shows the plan picked by the preset's endpoints
		if r := pr.Regions[homeRegion(pr.Regions, c.where)]; r.Chat != pr.Chat {
			t.Errorf("%s: the preset's endpoints %q aren't its home region's %q", c.name, pr.Chat, r.Chat)
		}
	}

	// a provider added in China stays at China's plan once the user is
	// abroad and the list is fetched again
	added := normalize(at(1))
	at(-1)
	if added = normalize(added); added.Chat != cn.chat || added.KeysURL != cn.keys || added.Catalog != cn.catalog {
		t.Errorf("an added provider moved to %q (%s, %s)", added.Chat, added.KeysURL, added.Catalog)
	}

	// regions that are no places: its own endpoints wherever the user is
	for _, where := range []int{-1, 1} {
		at(where)
		if p, _ := FromPreset("apikeyfan"); p.Chat != "https://api.apikey.fan/v1" {
			t.Errorf("APIKEY.FAN (%d) starts at %q", where, p.Chat)
		}
		// Kimi's China and Global accounts don't cross, so its picker
		// says to pick by the account (Kimi, 10-27: "并在编辑器加一句「按你
		// 的 Kimi 账号所在区域选择」"); no other partner's regions are both
		for _, pr := range Partners() {
			if pr.SplitAccounts != (pr.ID == "kimi") {
				t.Errorf("%s (%d): SplitAccounts %v", pr.ID, where, pr.SplitAccounts)
			}
		}
	}
}

// Only a first region that is China's or Global's gives way, and only to
// the other: yylx's Auto first, a plan named "Pay as you go · China", or
// a list without the other place stay at the first.
func TestHomeRegion(t *testing.T) {
	kimi := []Region{{ID: "cn", Name: "China"}, {ID: "global", Name: "Global"}, {ID: "api-cn", Name: "Pay as you go · China"}, {ID: "api", Name: "Pay as you go · Global"}}
	yylx := []Region{{ID: "auto", Name: "Auto"}, {ID: "global", Name: "Global"}, {ID: "cn", Name: "China Mainland"}}
	globalFirst := []Region{{ID: "intl", Name: "International"}, {ID: "cn", Name: "China Mainland"}}
	plans := []Region{{ID: "api-cn", Name: "Pay as you go · China"}, {ID: "api", Name: "Pay as you go · Global"}}
	chinaOnly := []Region{{ID: "cn", Name: "China"}, {ID: "hk", Name: "Hong Kong"}}
	for _, c := range []struct {
		name    string
		rs      []Region
		inChina int
		want    int
	}{
		{"kimi abroad", kimi, -1, 1},
		{"kimi in China", kimi, 1, 0},
		{"kimi unknown", kimi, 0, 0},
		{"yylx abroad", yylx, -1, 0},
		{"yylx in China", yylx, 1, 0},
		{"global first, in China", globalFirst, 1, 1},
		{"global first, abroad", globalFirst, -1, 0},
		{"plans only", plans, -1, 0},
		{"no Global", chinaOnly, -1, 0},
	} {
		if got := homeRegion(c.rs, c.inChina); got != c.want {
			t.Errorf("%s: homeRegion = %d, want %d", c.name, got, c.want)
		}
	}
	for zone, want := range map[string]int{"Asia/Shanghai": 1, "Asia/Urumqi": 1, "PRC": 1, "China Standard Time": 1, "Asia/Hong_Kong": -1, "Asia/Taipei": -1, "Asia/Singapore": -1, "America/New_York": -1, "UTC": -1, "": 0} {
		if got := chinaZone(zone); got != want {
			t.Errorf("chinaZone(%q) = %d, want %d", zone, got, want)
		}
	}
}
