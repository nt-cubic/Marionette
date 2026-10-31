package provider

import (
	"slices"
	"strings"
	"testing"
)

// effortHome is a machine of its own with providers whose model ids have
// colons of their own: OpenRouter's :free, Ollama's :7b, Bedrock's :0,
// and one whose id ends in a level's name.
func effortHome(t *testing.T) {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, p := range []Provider{
		{ID: "a", Name: "A", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m", "n"}},
		{ID: "or", Name: "OR", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"deepseek/deepseek-r1:free", "odd:high"}},
		{ID: "ol", Name: "Ollama", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"qwen:7b"}},
		{ID: "bd", Name: "Bedrock", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"anthropic.claude-opus-4-v1:0"}},
	} {
		if err := Save(p); err != nil {
			t.Fatal(err)
		}
	}
}

// A trailing :<level> is an effort; any other colon is the model's own, as
// is a level's name the model's own id ends in.
func TestMemberEffortParsing(t *testing.T) {
	effortHome(t)
	for in, want := range map[string][2]string{
		"a/m":                                  {"a/m", ""},
		"a/m:low":                              {"a/m", "low"},
		"a/m:XHigh":                            {"a/m", "xhigh"},
		"a/m:none":                             {"a/m", "none"},
		"a/m:max":                              {"a/m", "max"},
		"a/m:turbo":                            {"a/m:turbo", ""},
		"or/deepseek/deepseek-r1:free":         {"or/deepseek/deepseek-r1:free", ""},
		"or/deepseek/deepseek-r1:free:medium":  {"or/deepseek/deepseek-r1:free", "medium"},
		"deepseek/deepseek-r1:free":            {"deepseek/deepseek-r1:free", ""},
		"ol/qwen:7b":                           {"ol/qwen:7b", ""},
		"qwen:7b":                              {"qwen:7b", ""},
		"ol/qwen:7b:high":                      {"ol/qwen:7b", "high"},
		"bd/anthropic.claude-opus-4-v1:0":      {"bd/anthropic.claude-opus-4-v1:0", ""},
		"bd/anthropic.claude-opus-4-v1:0:high": {"bd/anthropic.claude-opus-4-v1:0", "high"},
		"or/odd:high":                          {"or/odd:high", ""}, // the model's own id
		"or/odd:high:low":                      {"or/odd:high", "low"},
		"group/fast:low":                       {"group/fast", "low"}, // for SaveGroup to refuse
		":low":                                 {":low", ""},
	} {
		m, e := MemberEffort(in)
		if m != want[0] || e != want[1] {
			t.Errorf("MemberEffort(%q) = %q %q, want %q %q", in, m, e, want[0], want[1])
		}
	}
	if WithMemberEffort("a/m", "") != "a/m" || WithMemberEffort("a/m", "low") != "a/m:low" {
		t.Error("WithMemberEffort")
	}
}

// Members at efforts are kept as typed, their effort lowercase; the same
// model at two efforts is two members, the same one twice is one; a
// group in the group takes none.
func TestSaveGroupMemberEfforts(t *testing.T) {
	effortHome(t)
	err := SaveGroup(Group{Name: "Fast", Members: []string{"a/m:LOW", "a/m:high", "a/m", "a/m:low", "ol/qwen:7b", "or/deepseek/deepseek-r1:free"},
		Rules: []Rule{{Use: "a/m:HIGH", Tokens: 10}}})
	if err != nil {
		t.Fatal(err)
	}
	g, ms, ok := FindGroup("group/fast")
	if !ok {
		t.Fatal("no group")
	}
	if want := []string{"a/m:low", "a/m:high", "a/m", "ol/qwen:7b", "or/deepseek/deepseek-r1:free"}; !slices.Equal(g.Members, want) {
		t.Fatalf("members %v, want %v", g.Members, want)
	}
	if g.Rules[0].Use != "a/m:high" {
		t.Fatalf("rule use %q", g.Rules[0].Use)
	}
	var got []string
	for _, m := range ms {
		got = append(got, m.ID+"="+m.Provider.ID+"|"+m.Model+"|"+m.Effort)
	}
	if s := strings.Join(got, " "); s != "a/m:low=a|m|low a/m:high=a|m|high a/m=a|m| ol/qwen:7b=ol|qwen:7b| or/deepseek/deepseek-r1:free=or|deepseek/deepseek-r1:free|" {
		t.Fatal(s)
	}
	// a stored group reads back the same (as a backup or a sync brings it)
	stored := slices.IndexFunc(Groups(), func(x Group) bool { return x.ID == "fast" })
	if stored < 0 || !slices.Equal(Groups()[stored].Members, g.Members) {
		t.Fatalf("stored: %+v", Groups())
	}
	if err := SaveGroup(Group{Name: "Top", Members: []string{"a/n", "group/fast:low"}}); err == nil || !strings.Contains(err.Error(), "is a group") {
		t.Fatalf("group at an effort: %v", err)
	}
}

// use= names a member at its effort, or without it when only one member
// is of that model.
func TestGroupMemberWithEffort(t *testing.T) {
	g := Group{ID: "fast", Members: []string{"a/m:low", "a/m:high", "b/n:medium"}}
	for in, want := range map[string]string{"a/m:high": "a/m:high", "m:low": "a/m:low", "b/n": "b/n:medium", "n": "b/n:medium", "A/M:HIGH": "a/m:high"} {
		if got, err := GroupMember(g, in); err != nil || got != want {
			t.Errorf("GroupMember(%q) = %q %v, want %q", in, got, err, want)
		}
	}
	if _, err := GroupMember(g, "a/m"); err == nil || !strings.Contains(err.Error(), "name one") {
		t.Errorf("a/m at two efforts: %v", err)
	}
}

// The levels a group lists are those every member without an effort of
// its own has in common; members fixed at one don't narrow them. A group
// of fixed members alone lists the efforts they are fixed at.
func TestGroupLevelsWithFixedMembers(t *testing.T) {
	effortHome(t)
	if err := SetModelEfforts("a/m", []string{"low", "medium", "high"}); err != nil {
		t.Fatal(err)
	}
	if err := SetModelEfforts("a/n", []string{"medium", "high", "xhigh"}); err != nil {
		t.Fatal(err)
	}
	for name, c := range map[string]struct {
		members []string
		want    []string
	}{
		"plain":   {[]string{"a/m", "a/n"}, []string{"medium", "high"}},
		"one":     {[]string{"a/m", "a/n:low"}, []string{"low", "medium", "high"}},
		"fixed":   {[]string{"a/m:xhigh", "a/n:low"}, []string{"low", "xhigh"}},
		"twice":   {[]string{"a/n:low", "a/n:high", "a/n"}, []string{"medium", "high", "xhigh"}},
		"no deal": {[]string{"a/m:low", "a/m:high"}, []string{"low", "high"}},
	} {
		if err := SaveGroup(Group{Name: "g-" + strings.ReplaceAll(name, " ", "-"), Members: c.members}); err != nil {
			t.Fatal(err)
		}
		id := GroupPrefix + "g-" + strings.ReplaceAll(name, " ", "-")
		i := slices.IndexFunc(Catalog(), func(e Entry) bool { return e.ID == id })
		if i < 0 {
			t.Fatalf("%s: not listed", name)
		}
		if got := Catalog()[i].Efforts; !slices.Equal(got, c.want) {
			t.Errorf("%s: levels %v, want %v", name, got, c.want)
		}
	}
}

// While a request holds the providers, whether a member at an effort is a
// model's own id is looked up among them once, not at each of the
// group's expansions (XcodeFish, #2502: a third of /v1/models' CPU was
// Find and Available again for every such member); a provider saved
// meanwhile is looked at anew, and with nothing held every look is fresh.
func TestMemberEffortLooksOncePerHold(t *testing.T) {
	effortHome(t)
	looks := func(id string, n int) int64 {
		was := providersLooked.Load()
		for range n {
			if m, e := MemberEffort(id); m+":"+e != id {
				t.Fatalf("MemberEffort(%q) = %q %q", id, m, e)
			}
		}
		return providersLooked.Load() - was
	}
	release := Hold()
	if got := looks("a/m:low", 5); got != 1 {
		t.Fatalf("held: looked %d times, want 1", got)
	}
	if err := Save(Provider{ID: "a", Name: "A", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m", "m:low"}}); err != nil {
		t.Fatal(err)
	}
	if m, e := MemberEffort("a/m:low"); m != "a/m:low" || e != "" {
		t.Fatalf("saved while held: %q %q, want the model's own id", m, e)
	}
	release()
	if got := looks("a/n:high", 3); got != 3 {
		t.Fatalf("not held: looked %d times, want 3", got)
	}
}

// A member may carry several levels, joined by "+" (Garydesu on X: one
// model in a group at a few efforts, the agent picking): kept each once,
// lowest first, lowercase; read back as levels the member follows the
// agent among, not one it is fixed at; offered to agents as the group's
// levels for it; and one level alone is still a fixed effort, as a group
// saved before has it.
func TestMemberSeveralLevels(t *testing.T) {
	effortHome(t)
	for in, want := range map[string][2]string{
		"a/m:high+low":     {"a/m", "low+high"},
		"a/m:HIGH+low+low": {"a/m", "low+high"},
		"a/m:low+low":      {"a/m", "low"},
		"a/m:low+turbo":    {"a/m:low+turbo", ""},
		"or/odd:high+max":  {"or/odd", "high+max"},
	} {
		if m, e := MemberEffort(in); m != want[0] || e != want[1] {
			t.Errorf("MemberEffort(%q) = %q %q, want %q %q", in, m, e, want[0], want[1])
		}
	}
	if f, l := MemberLevels("low+high"); f != "" || !slices.Equal(l, []string{"low", "high"}) {
		t.Errorf("MemberLevels(low+high) = %q %v", f, l)
	}
	if f, l := MemberLevels("high"); f != "high" || l != nil {
		t.Errorf("MemberLevels(high) = %q %v", f, l)
	}
	if err := SetModelEfforts("a/m", []string{"low", "medium", "high", "xhigh"}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "Pick", Members: []string{"a/m:XHIGH+low", "a/n:medium"}, Rules: []Rule{{Use: "a/m:low+xhigh", Tokens: 10}}}); err != nil {
		t.Fatal(err)
	}
	g, ms, ok := FindGroup("group/pick")
	if !ok {
		t.Fatal("no group")
	}
	if want := []string{"a/m:low+xhigh", "a/n:medium"}; !slices.Equal(g.Members, want) {
		t.Fatalf("members %v, want %v", g.Members, want)
	}
	if ms[0].Effort != "" || !slices.Equal(ms[0].Levels, []string{"low", "xhigh"}) || ms[0].Spelled() != "a/m:low+xhigh" {
		t.Fatalf("member %+v", ms[0])
	}
	if ms[1].Effort != "medium" || ms[1].Levels != nil || ms[1].Spelled() != "a/n:medium" {
		t.Fatalf("fixed member %+v", ms[1])
	}
	i := slices.IndexFunc(Catalog(), func(e Entry) bool { return e.ID == "group/pick" })
	if i < 0 {
		t.Fatal("not listed")
	}
	if got := Catalog()[i].Efforts; !slices.Equal(got, []string{"low", "xhigh"}) {
		t.Fatalf("offered %v, want the member's own low and xhigh", got)
	}
	// beside a member that follows the agent, the group offers what both have
	if err := SetModelEfforts("a/n", []string{"low", "medium", "high"}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "Shared", Members: []string{"a/m:low+xhigh", "a/n"}}); err != nil {
		t.Fatal(err)
	}
	i = slices.IndexFunc(Catalog(), func(e Entry) bool { return e.ID == "group/shared" })
	if got := Catalog()[i].Efforts; !slices.Equal(got, []string{"low"}) {
		t.Fatalf("shared %v, want low", got)
	}
}
