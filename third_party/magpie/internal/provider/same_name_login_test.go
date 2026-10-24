package provider

import (
	"encoding/json"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// Two ZCode accounts of one name, the ZCode app's own removed in magpie
// (hidden) and one signed in through magpie beside it: Remove on the one
// listed takes that one, by its slot, and leaves the hidden one as it was
// (dev on Discord). The own account's saved id is the mover's "ZCode",
// named after the app's sign-in now, so the two read alike unless the
// slot tells them apart.
func TestRemoveListedAccountLeavesHiddenNamesake(t *testing.T) {
	movedPlugin(t, "zcode", map[string]map[string]any{
		"zcode":        {"type": "oauth", "access": "a", "refresh": `{"source":"zcode"}`, "expires": 0, "accountId": "ZCode"},
		"zcode#l47q19": {"type": "oauth", "access": "b", "refresh": "r", "expires": 0, "accountId": "dev@z"},
	})
	pluginOwnUser = func(id string) string {
		if id == "zcode" {
			return "dev@z"
		}
		return ""
	}
	writeFile(t, loginsPath(), []savedLogin{
		{Agent: "plugin:zcode", User: "dev@z", Home: "zcode", Hidden: hiddenNoMark},
		{Agent: "plugin:zcode", User: "dev@z", Home: "zcode#l47q19", On: true},
	})
	ls := Logins("zcode")
	if len(ls) != 1 || ls[0].Own {
		t.Fatalf("listed %+v, want the one magpie signed in", ls)
	}
	if err := ForgetLogin("zcode", ls[0].User); err != nil {
		t.Fatal(err)
	}
	auths := plugin.Auths("zcode")
	if _, ok := auths["zcode#l47q19"]; ok {
		t.Error("the account removed is still signed in")
	}
	if _, ok := auths["zcode"]; !ok {
		t.Error("the hidden ZCode app's account was signed out in its place")
	}
	hidden := false
	for _, l := range readLogins() {
		if l.Agent == "plugin:zcode" {
			if l.Home != "zcode" {
				t.Errorf("still saved: %+v", l)
			}
			hidden = hidden || l.Hidden != ""
		}
	}
	if !hidden {
		t.Error("the ZCode app's account is no longer kept hidden")
	}
	if got := listed("zcode"); got != "" {
		t.Errorf("listed after the remove: %q", got)
	}
}

// Two accounts that share a name and differ only in the slot, the one
// listed and one that would read alike: pluginLabels keeps them apart
// after the agent's own is named as the agent is signed in.
func TestPluginOwnNameKeepsSlotApart(t *testing.T) {
	movedPlugin(t, "zcode", map[string]map[string]any{
		"zcode":        {"type": "oauth", "access": "a", "refresh": `{"source":"zcode"}`, "expires": 0, "accountId": "ZCode"},
		"zcode#l47q19": {"type": "oauth", "access": "b", "refresh": "r", "expires": 0, "accountId": "dev@z"},
	})
	pluginOwnUser = func(id string) string {
		if id == "zcode" {
			return "dev@z"
		}
		return ""
	}
	if got := listed("zcode"); got != "*dev@z (own),dev@z (l47q19)" {
		t.Fatalf("accounts %s", got)
	}
}

// The built-in path: an agent's own account hidden in magpie and one of
// magpie's of the same name, the latter listed. Removing it removes it,
// not the hidden one.
func TestForgetSideLoginTakesTheListedOne(t *testing.T) {
	claudeHome(t)
	writeFile(t, loginsPath(), []savedLogin{
		{Agent: "side", User: "a@x", Hidden: hiddenNoMark},
		{Agent: "side", User: "a@x", Auth: json.RawMessage(`{"t":1}`), On: true},
	})
	usable := func(savedLogin) bool { return true }
	ls := sideLogins("side", "a@x", usable)
	if len(ls) != 1 || ls[0].saved.own() {
		t.Fatalf("listed %+v, want magpie's", ls)
	}
	var gone savedLogin
	if err := forgetSideLogin("side", "a@x", ls, func(l savedLogin) { gone = l }); err != nil {
		t.Fatal(err)
	}
	if len(gone.Auth) == 0 {
		t.Errorf("told of %+v going, not magpie's", gone)
	}
	var left []savedLogin
	for _, l := range readLogins() {
		if l.Agent == "side" {
			left = append(left, l)
		}
	}
	if len(left) != 1 || !left[0].own() || left[0].Hidden == "" {
		t.Errorf("left %+v, want the agent's own, hidden", left)
	}
}
