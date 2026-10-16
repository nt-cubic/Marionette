package cli

import (
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// magpie group template lists the templates as the user's models make them,
// and adds one in one go: added twice, the second is numbered beside the
// first rather than written over it (yetone, 10-10).
func TestGroupTemplateCLI(t *testing.T) {
	groupsHome(t)
	if err := provider.Save(provider.Provider{ID: "c", Name: "C", Key: "kc", Models: []string{"gpt-5.4-mini"}, Chat: "http://127.0.0.1:1/v1"}); err != nil {
		t.Fatal(err)
	}
	out, err := stdoutOf(t, func() error { return groupCmd([]string{"group", "template"}) })
	if err != nil {
		t.Fatal(err)
	}
	for _, k := range provider.TemplateKinds {
		if !strings.Contains(out, k) {
			t.Errorf("list said %q, no %s", out, k)
		}
	}

	if _, err := stdoutOf(t, func() error { return groupCmd([]string{"group", "template", "steady"}) }); err != nil {
		t.Fatal(err)
	}
	g, err := findGroup("never-stuck")
	if err != nil || g.Name != "Never stuck" || g.Routing != provider.Ordered || len(g.Members) < 2 {
		t.Fatalf("steady: %v %+v", err, g)
	}
	out, err = stdoutOf(t, func() error { return groupCmd([]string{"group", "template", "never-stuck", "Steady"}) })
	if err != nil || !strings.Contains(out, "group/never-stuck-2") {
		t.Fatalf("again: %v %q", err, out)
	}
	if again, err := findGroup("never-stuck-2"); err != nil || again.Name != "Steady" || !slices.Equal(again.Members, g.Members) {
		t.Fatalf("again: %v %+v", err, again)
	}
	if g2, _ := findGroup("never-stuck"); g2.Name != "Never stuck" {
		t.Errorf("the first was written over: %+v", g2)
	}

	if _, err := stdoutOf(t, func() error { return groupCmd([]string{"group", "template", "smart"}) }); err != nil {
		t.Fatal(err)
	}
	s, err := findGroup("smart-split")
	if err != nil || len(s.Members) != 2 || s.Classifier == "" || !slices.ContainsFunc(s.Rules, func(r provider.Rule) bool { return r.Intent == provider.IntentHard }) {
		t.Fatalf("smart: %v %+v", err, s)
	}

	if err := groupCmd([]string{"group", "template", "nope"}); err == nil || !strings.Contains(err.Error(), "smart") {
		t.Errorf("no such template: %v", err)
	}
}
