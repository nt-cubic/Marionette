package provider

import (
	"slices"
	"testing"
)

// #499: a provider added later can be put first. The order is the list's
// and the one they are tried in: a bare model id goes to the first that
// serves it, and the group magpie finds of a model several serve tries
// the first one first. One added after stays last; a rename keeps its place.
func TestProviderOrder(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, id := range []string{"a", "b", "c"} {
		if err := Save(Provider{ID: id, Name: id, Key: "sk-" + id, Chat: "https://" + id + ".example.com/v1", Models: []string{"m"}}); err != nil {
			t.Fatal(err)
		}
	}
	ids := func() []string {
		var out []string
		for _, p := range All() {
			out = append(out, p.ID)
		}
		return out
	}
	if got := ids(); !slices.Equal(got, []string{"a", "b", "c"}) {
		t.Fatalf("as added: %v", got)
	}
	if err := SetOrder([]string{"c", "a"}); err != nil {
		t.Fatal(err)
	}
	if got := ids(); !slices.Equal(got, []string{"c", "a", "b"}) {
		t.Fatalf("after SetOrder(c, a): %v", got)
	}
	if p, _, ok := Resolve("m"); !ok || p.ID != "c" {
		t.Fatalf("m resolves to %q, not c", p.ID)
	}
	g := slices.IndexFunc(autoGroups(providerEntries(), nil), func(g Group) bool { return g.ID == AutoGroupID("m") })
	if g < 0 {
		t.Fatal("no group found of m")
	}
	if ms := autoGroups(providerEntries(), nil)[g].Members; !slices.Equal(ms, []string{"c/m", "a/m", "b/m"}) {
		t.Fatalf("group of m tries %v", ms)
	}
	if err := Save(Provider{ID: "d", Name: "d", Key: "sk-d", Chat: "https://d.example.com/v1", Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	if got := ids(); !slices.Equal(got, []string{"c", "a", "b", "d"}) {
		t.Fatalf("d added: %v", got)
	}
	if err := Rename("c", "cc"); err != nil {
		t.Fatal(err)
	}
	if got := ids(); !slices.Equal(got, []string{"cc", "a", "b", "d"}) {
		t.Fatalf("c renamed: %v", got)
	}
	for _, bad := range [][]string{{"a", "a"}, {"nope"}} {
		if err := SetOrder(bad); err == nil {
			t.Fatalf("SetOrder(%v) taken", bad)
		}
	}
}

// Sync's order (MirrorOrder): the other computer's ids first, as it has
// them, then those named here alone (an account only this computer has)
// in the order they have here; none from the other leaves this one's.
func TestMirrorOrder(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, id := range []string{"a", "b", "c", "d"} {
		if err := Save(Provider{ID: id, Name: id, Key: "sk-" + id, Chat: "https://" + id + ".example.com/v1"}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SetOrder([]string{"d", "a", "c", "b"}); err != nil {
		t.Fatal(err)
	}
	if err := MirrorOrder(nil); err != nil {
		t.Fatal(err)
	}
	if got, _ := StoredOrder(); !slices.Equal(got, []string{"d", "a", "c", "b"}) {
		t.Fatalf("none from the other: %v", got)
	}
	if err := MirrorOrder([]string{"c", "a", "elsewhere"}); err != nil {
		t.Fatal(err)
	}
	if got, _ := StoredOrder(); !slices.Equal(got, []string{"c", "a", "elsewhere", "d", "b"}) {
		t.Fatalf("mirrored: %v", got)
	}
	var listed []string
	for _, p := range All() {
		listed = append(listed, p.ID)
	}
	if !slices.Equal(listed, []string{"c", "a", "d", "b"}) {
		t.Fatalf("listed: %v", listed)
	}
}

// #1940 (Sun1090): WorkBuddy and WorkBuddy AI were put first on one
// computer, and the order synced to another kept them where they were. A
// subscription served by its plugin is workbuddy-plugin where the built-in
// isn't moved onto it and workbuddy where it is, and that is each
// computer's own, so the order synced names it by the other computer's id.
// It keeps its place by either.
func TestOrderKeepsAPluginsPlaceByEitherID(t *testing.T) {
	ps := func(ids ...string) []Provider {
		var out []Provider
		for _, id := range ids {
			out = append(out, Provider{ID: id})
		}
		return out
	}
	ids := func(ps []Provider) []string {
		var out []string
		for _, p := range ps {
			out = append(out, p.ID)
		}
		return out
	}
	// put in order where both were moved: workbuddy-ai, workbuddy
	order := []string{WorkBuddyAIID, "workbuddy", "zhipu", "deepseek"}
	// here neither is moved, so the plugin signed in under its own id
	got := ids(ordered(ps("zhipu", "deepseek", "workbuddy-plugin", WorkBuddyAIID+"-plugin"), order))
	if want := []string{WorkBuddyAIID + "-plugin", "workbuddy-plugin", "zhipu", "deepseek"}; !slices.Equal(got, want) {
		t.Fatalf("not moved here: %v, want %v", got, want)
	}
	// and the other way round
	order = []string{"workbuddy-plugin", WorkBuddyAIID + "-plugin", "zhipu"}
	got = ids(ordered(ps("zhipu", WorkBuddyAIID, "workbuddy"), order))
	if want := []string{"workbuddy", WorkBuddyAIID, "zhipu"}; !slices.Equal(got, want) {
		t.Fatalf("moved here: %v, want %v", got, want)
	}
	// an id the order names itself wins over its counterpart's place
	got = ids(ordered(ps("zhipu", "workbuddy", "workbuddy-plugin"), []string{"workbuddy-plugin", "zhipu", "workbuddy"}))
	if want := []string{"workbuddy-plugin", "zhipu", "workbuddy"}; !slices.Equal(got, want) {
		t.Fatalf("both here: %v, want %v", got, want)
	}
	// a custom provider whose id ends in -plugin is only itself
	got = ids(ordered(ps("zhipu", "my-plugin", "my"), []string{"my", "zhipu"}))
	if want := []string{"my", "zhipu", "my-plugin"}; !slices.Equal(got, want) {
		t.Fatalf("custom: %v, want %v", got, want)
	}
}

// #1940 (Sun1090), on v0.1.1225: WorkBuddy AI is moved onto its plugin on
// one computer (workbuddy-ai) and signed in to the plugin under its own id
// on the other (workbuddy-ai-plugin), and put first there. The order sync
// brings names workbuddy-ai-plugin first, and MirrorOrder puts this
// computer's own workbuddy-ai, which it doesn't name, after the rest: the
// order names both, and its place is the earlier one, not its own id's.
// Nor is it among those sync says aren't signed in here.
func TestSyncedOrderKeepsAPluginsPlaceNamedBoth(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	plug := WorkBuddyAIID + "-plugin"
	// this computer's order, as it had it: WorkBuddy AI last
	if err := MirrorOrder([]string{"zhipu", "deepseek", WorkBuddyAIID}); err != nil {
		t.Fatal(err)
	}
	// the other computer put it first, under its plugin's id
	if err := MirrorOrder([]string{plug, "zhipu", "deepseek"}); err != nil {
		t.Fatal(err)
	}
	order, _ := StoredOrder()
	if want := []string{plug, "zhipu", "deepseek", WorkBuddyAIID}; !slices.Equal(order, want) {
		t.Fatalf("mirrored: %v, want %v", order, want)
	}
	var got []string
	for _, p := range ordered([]Provider{{ID: "zhipu"}, {ID: "deepseek"}, {ID: WorkBuddyAIID}}, order) {
		got = append(got, p.ID)
	}
	if want := []string{WorkBuddyAIID, "zhipu", "deepseek"}; !slices.Equal(got, want) {
		t.Fatalf("listed: %v, want %v", got, want)
	}
	// where both are providers here, each keeps its own place
	got = nil
	for _, p := range ordered([]Provider{{ID: "zhipu"}, {ID: "deepseek"}, {ID: WorkBuddyAIID}, {ID: plug}}, order) {
		got = append(got, p.ID)
	}
	if want := []string{plug, "zhipu", "deepseek", WorkBuddyAIID}; !slices.Equal(got, want) {
		t.Fatalf("both here: %v, want %v", got, want)
	}
	synced := []Provider{{ID: plug, Models: []string{"m"}}, {ID: "commandcode-plan-plugin", Models: []string{"m"}}}
	if left := notSignedIn(synced, map[string]bool{WorkBuddyAIID: true}); !slices.Equal(left, []string{"commandcode-plan-plugin"}) {
		t.Fatalf("not signed in here: %v, want only commandcode-plan-plugin", left)
	}
}
