package provider

import (
	"encoding/json"
	"slices"
	"strings"
	"testing"
)

func shareHome(t *testing.T, ps ...Provider) {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, p := range ps {
		if err := Save(p); err != nil {
			t.Fatal(err)
		}
	}
}

func sharedAt(t *testing.T, id string) Group {
	t.Helper()
	g, _, ok := FindGroup(GroupPrefix + id)
	if !ok {
		t.Fatalf("no group %q", id)
	}
	return g
}

// laincatx on X: a routing group is shared as text and pasted into
// another magpie. What goes is the group's routing, rules and members,
// never a key; pasted back, it is the same group under a name of its own.
func TestSharedGroupRoundTrip(t *testing.T) {
	const secret = "sk-shared-secret-0123456789"
	shareHome(t,
		Provider{ID: "a", Name: "A", Key: secret, Chat: "http://127.0.0.1:1/v1", Models: []string{"x", "small"}},
		Provider{ID: "b", Name: "B", Key: secret + "-b", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}},
	)
	g := Group{Name: "Smart", Members: []string{"a/x:high", "b/x", "a/small"}, Routing: Ordered, Affinity: "session",
		Patience: 90, Off: []string{"b/x"}, Classifier: "a/small", Levels: []string{"low", "high"},
		Rules: []Rule{
			{Use: "a/small", Compact: true},
			{Use: "a/small", Intent: "a quick question"},
			{Use: "a/x:high", Tokens: 50000, Agents: []string{"codex"}},
			{Use: "b/x", Time: &TimeWindow{From: "22:00", To: "08:00", Days: []string{"fri"}}},
			{Use: "a/small", Folders: []string{"~/work/acme-secret"}},
			{Use: "a/small", Pause: true, ExceptFolders: []string{"~/work/big-secret"}},
		}}
	if err := SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	text, err := ShareGroup("smart")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(text, secret) || strings.Contains(text, "127.0.0.1") || strings.Contains(text, "acme-secret") || strings.Contains(text, "big-secret") {
		t.Fatalf("the shared text carries a provider's key or address, or the sharer's folder: %s", text)
	}
	if !strings.Contains(text, `"magpie":"routing-group"`) || !strings.Contains(text, `"version":1`) {
		t.Fatalf("the shared text doesn't say what it is: %s", text)
	}

	// pasted from a chat, with words and a code fence around it
	res, err := ImportGroup("here's mine:\n```json\n" + text + "\n```\n")
	if err != nil {
		t.Fatal(err)
	}
	if res.ID != "smart-2" || res.Name != "Smart 2" || len(res.Missing) != 0 || len(res.Instead) != 0 || res.Rules != 0 {
		t.Fatalf("imported %+v, want Smart 2 whole", res)
	}
	src, got := sharedAt(t, "smart"), sharedAt(t, "smart-2")
	if !slices.Equal(got.Members, src.Members) || !slices.Equal(got.Off, src.Off) || !slices.Equal(got.Levels, src.Levels) ||
		got.Routing != src.Routing || got.Affinity != src.Affinity || got.Patience != src.Patience || got.Classifier != src.Classifier {
		t.Fatalf("imported %+v\nfrom %+v", got, src)
	}
	// all but the rules by the sharer's folders
	a, _ := json.Marshal(src.Rules[:len(src.Rules)-2])
	b, _ := json.Marshal(got.Rules)
	if string(a) != string(b) {
		t.Fatalf("rules %s, want %s", b, a)
	}
	// the one shared is left as it was
	if len(Groups()) < 2 || sharedAt(t, "smart").Name != "Smart" {
		t.Fatal("the shared group changed")
	}
}

// A member whose provider the importer doesn't have is the same model
// under one they have, else left out and named, with the rules that send
// to it; a rule by intent goes when its classifier does.
func TestSharedGroupMissingMembers(t *testing.T) {
	shareHome(t, Provider{ID: "mine", Name: "Mine", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}})
	text, _ := json.Marshal(SharedGroup{Magpie: ShareMark, Version: ShareVersion, Group: Group{
		ID: "theirs", Name: "Theirs", Members: []string{"relay/x", "gone/y", "group/nested"}, Routing: Ordered,
		Off: []string{"gone/y"}, Classifier: "gone/judge",
		Rules: []Rule{
			{Use: "gone/y", Tokens: 1000},
			{Use: "relay/x", Agents: []string{"claude"}},
			{Use: "relay/x", Intent: "hard"},
			{Use: "relay/x", Folders: []string{"/home/them/proj"}},
			{Use: "relay/x", Pause: true, ExceptFolders: []string{"/home/them/big"}},
		},
	}})
	res, err := ImportGroup(string(text))
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"gone/y", "group/nested", "gone/judge"}; !slices.Equal(res.Missing, want) {
		t.Errorf("missing %v, want %v", res.Missing, want)
	}
	if want := []string{"relay/x → mine/x"}; !slices.Equal(res.Instead, want) {
		t.Errorf("instead %v, want %v", res.Instead, want)
	}
	if res.Rules != 4 {
		t.Errorf("rules left out %d, want 4: the one to gone/y, the one by intent and the two by their folders", res.Rules)
	}
	g := sharedAt(t, res.ID)
	if !slices.Equal(g.Members, []string{"mine/x"}) || len(g.Off) != 0 || g.Classifier != "" || len(g.Rules) != 1 || g.Rules[0].Use != "mine/x" {
		t.Fatalf("imported %+v", g)
	}

	// nothing of it here: nothing is added, and the reader is told why
	none, _ := json.Marshal(SharedGroup{Magpie: ShareMark, Version: ShareVersion, Group: Group{Name: "N", Members: []string{"gone/y"}}})
	n := len(Groups())
	if _, err := ImportGroup(string(none)); err == nil || !strings.Contains(err.Error(), "gone/y") {
		t.Fatalf("none here: %v", err)
	}
	if len(Groups()) != n {
		t.Fatal("a group was added with none of its models")
	}
}

func TestSharedGroupBadText(t *testing.T) {
	shareHome(t, Provider{ID: "a", Name: "A", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}})
	for _, c := range []struct{ text, says string }{
		{"", "not a routing group shared from magpie"},
		{"hello {not json}", "not a routing group shared from magpie"},
		{`{"magpie":"something-else","version":1,"group":{"members":["a/x"]}}`, "not a routing group shared from magpie"},
		{`{"magpie":"routing-group","version":2,"group":{"members":["a/x"]}}`, "newer magpie (format 2)"},
		{`{"magpie":"routing-group","group":{"members":["a/x"]}}`, "no format"},
		{`{"magpie":"routing-group","version":1,"group":{"members":"a/x"}}`, "can't be read"},
	} {
		if _, err := ImportGroup(c.text); err == nil || !strings.Contains(err.Error(), c.says) {
			t.Errorf("ImportGroup(%q) = %v, want an error saying %q", c.text, err, c.says)
		}
	}
	if len(Groups()) != 0 {
		t.Fatalf("bad text added %v", Groups())
	}
}
