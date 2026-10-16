package gateway

// A rule that names a group in the group puts the group's whole plan
// first, in the order the group's own routing planned it (#2407): ranked
// by the group's listed members, a rotate group's own order was erased and
// every request the rule put first was served by its first member.

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// rotateModelsUp answers with the model it was asked for, so every member
// of the nested layout is tellable apart.
func rotateModelsUp(t *testing.T, id string, models ...string) {
	t.Helper()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q struct {
			Model string `json:"model"`
		}
		json.Unmarshal(b, &q)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served-by-`+q.Model+`"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(up.Close)
	p := provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Chat: up.URL + "/v1", Models: models}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
}

func servedModel(body string) string {
	if i := strings.Index(body, "served-by-"); i >= 0 {
		rest := body[i+len("served-by-"):]
		if j := strings.Index(rest, `"`); j >= 0 {
			return rest[:j] + " "
		}
	}
	return ""
}

// The layout of the report: an outer smart group whose rule names
// group/rot, whose members are a model, a group in it and another model.
// Each fresh conversation through the rule takes the rotate group's next
// member, as asking the group directly does.
func TestRuleNamingARotateGroupKeepsItsTurns(t *testing.T) {
	fresh(t)
	rotateModelsUp(t, "pa", "modelA", "modelB", "modelC")
	rotateModelsUp(t, "pb", "modelB")
	rotateModelsUp(t, "pc", "modelB")
	if err := provider.SaveGroup(provider.Group{Name: "nord", Members: []string{"pa/modelA", "pa/modelB", "pb/modelB", "pc/modelB"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "rot", Members: []string{"pa/modelA", "group/nord", "pa/modelC"}, Routing: provider.Rotate}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "outer", Members: []string{"group/rot"}, Routing: "",
		Rules: []provider.Rule{{Use: "group/rot", Tokens: 1}}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	served := ""
	for i := range 6 {
		_, body := postAs(t, s, fmt.Sprintf("via-%d", i), `{"model":"group/outer","messages":[{"role":"user","content":"please reply ok to this routing probe message"}]}`)
		served += servedModel(body)
	}
	if want := "modelA modelB modelC modelA modelB modelC "; served != want {
		t.Fatalf("the rotate group in the group never advanced: %s (want %s)", served, want)
	}
}
