package usage

import (
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/probelog"
	"github.com/yetone/magpie/internal/provider"
)

// A provider renamed is shown by the id it has now everywhere usage names
// it, its calls from before the rename too: the Requests page listed it
// twice, once by each id, its model-list rows under the old one, and the
// Sessions tab named it by the old id (cybercaius on Discord). An id it
// gave up and another provider took later is the new one's only from
// then on.
func TestRenamedProviderShownOnce(t *testing.T) {
	reportedHome(t)
	if err := provider.Save(provider.Provider{ID: "myrelay", Name: "My relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-9"}}); err != nil {
		t.Fatal(err)
	}
	then := time.Now().Add(-time.Hour)
	Append(Record{Time: then, Agent: "codex", Session: "s1", Provider: "myrelay", Model: "gpt-9", Input: 100, Output: 10, Status: 200})
	probelog.Sink(probelog.Row{Time: then.Add(time.Second), Kind: probelog.KindList, Provider: "myrelay", Host: "relay.example", Endpoint: "GET /v1/models", Status: 200, Millis: 30})
	// read once under the old id, so what is kept of it is built again
	if p := QueryPage(All, Filter{}, 0, 50); !slices.Equal(p.Providers, []string{"myrelay"}) {
		t.Fatalf("before: %v", p.Providers)
	}
	_ = Vias(time.Time{})
	if err := provider.Rename("myrelay", "newrelay"); err != nil {
		t.Fatal(err)
	}
	Append(Record{Time: then.Add(2 * time.Second), Agent: "codex", Session: "s1", Provider: "newrelay", Model: "gpt-9", Input: 100, Output: 10, Status: 200})

	page := QueryPage(All, Filter{}, 0, 50)
	if !slices.Equal(page.Providers, []string{"newrelay"}) {
		t.Fatalf("providers %v", page.Providers)
	}
	for _, r := range page.Rows {
		if r.Provider != "newrelay" {
			t.Fatalf("a %s row under %q", r.Source, r.Provider)
		}
	}
	if p := QueryPage(All, Filter{Provider: "newrelay"}, 0, 50); p.Total != 3 || p.Sum.Calls != 2 {
		t.Fatalf("filtered: total %d sum %+v", p.Total, p.Sum)
	}
	vs := Vias(time.Time{}).Of("codex|s1")
	if len(vs) != 1 || vs[0].Provider != "newrelay" || vs[0].Calls != 2 {
		t.Fatalf("vias %+v", vs)
	}

	// a provider added under the old id afterwards takes only its own calls
	if err := provider.Save(provider.Provider{ID: "myrelay", Name: "Another", Key: "k2", Chat: "http://127.0.0.1:2/v1", Models: []string{"gpt-9"}}); err != nil {
		t.Fatal(err)
	}
	Append(Record{Time: time.Now().Add(time.Second), Agent: "codex", Session: "s2", Provider: "myrelay", Model: "gpt-9", Input: 100, Output: 10, Status: 200})
	page = QueryPage(All, Filter{}, 0, 50)
	if !slices.Equal(page.Providers, []string{"myrelay", "newrelay"}) {
		t.Fatalf("providers after the id was taken %v", page.Providers)
	}
	got := map[string]int{}
	for _, r := range page.Rows {
		got[r.Provider]++
	}
	if got["newrelay"] != 3 || got["myrelay"] != 1 {
		t.Fatalf("rows by provider %v", got)
	}
	if vs := Vias(time.Time{}).Of("codex|s1"); len(vs) != 1 || vs[0].Provider != "newrelay" {
		t.Fatalf("vias of the renamed one %+v", vs)
	}
	if vs := Vias(time.Time{}).Of("codex|s2"); len(vs) != 1 || vs[0].Provider != "myrelay" {
		t.Fatalf("vias of the new one %+v", vs)
	}
	calls := map[string]int{}
	for _, g := range Summarize(All).Models {
		calls[g.ID] += g.Calls
	}
	if calls["newrelay/gpt-9"] != 2 || calls["myrelay/gpt-9"] != 1 {
		t.Fatalf("summary %v", calls)
	}
}
