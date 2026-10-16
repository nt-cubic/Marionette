package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The router strategy point's groups with plugins/groups on and off: on, a
// group of the user's own is listed and answers; off, in the process
// serving the gateway, it is neither, while the group magpie found for a
// model two providers serve is listed and answers either way.
func TestGroupsOnAndOff(t *testing.T) {
	fresh(t)
	v := &keyed{}
	serveOn(t, "pa", "ka", []string{"m", "x"}, v)
	serveOn(t, "pb", "kb", []string{"m"}, v)
	if err := provider.SaveGroup(provider.Group{Name: "Mine", Members: []string{"pa/x", "pb/m"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	check := func(when string, mine bool) {
		t.Helper()
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("GET", "/v1/models", nil))
		list := rec.Body.String()
		if strings.Contains(list, `"group/mine"`) != mine || !strings.Contains(list, `"group/auto-m"`) {
			t.Fatalf("%s: models %s", when, list)
		}
		code, body := postAs(t, s, "", `{"model":"group/mine","messages":[{"role":"user","content":"hi"}]}`)
		if (code == 200) != mine {
			t.Fatalf("%s: group/mine %d %s", when, code, body)
		}
		if code, body := postAs(t, s, "", `{"model":"group/auto-m","messages":[{"role":"user","content":"hi"}]}`); code != 200 {
			t.Fatalf("%s: group/auto-m %d %s", when, code, body)
		}
	}
	check("on", true)
	t.Run("off", func(t *testing.T) {
		withoutGroups(t)
		check("off", false)
	})
	check("on again", true)
}
