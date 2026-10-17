package provider

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/zed"
)

// fakePlugin readies the test plugin's host for a move of fakeco, with
// the plugin not yet installed: the move's install adds it.
func fakePlugin(t *testing.T) (context.Context, string, *int) {
	t.Helper()
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	t.Cleanup(cancel)
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	fakeMover(t)
	movers["fakeco"].pkg = abs
	installs := 0
	installPlugin = func(ctx context.Context, pkg, _ string) error {
		installs++
		if pluginListed(pkg) {
			return nil
		}
		_, err := plugin.Add(ctx, pkg)
		return err
	}
	return ctx, abs, &installs
}

func setFakeLogins(t *testing.T, ls ...savedLogin) {
	t.Helper()
	loginsMu.Lock()
	defer loginsMu.Unlock()
	if err := writeLogins(ls); err != nil {
		t.Fatal(err)
	}
}

// Every account lapsed: the built-in is retired, so the plugin is
// installed and they go along marked, to sign in again through it.
func TestMoveEveryAccountLapsed(t *testing.T) {
	ctx, _, installs := fakePlugin(t)
	l := fakeLogin("a@fake", "r-a", true, true)
	l.Lapsed = "expired"
	setFakeLogins(t, l)
	if err := Move(ctx, "fakeco"); err != nil || !Moved("fakeco") || *installs != 1 {
		t.Fatalf("Move with every account lapsed = %v, moved %v, %d installs", err, Moved("fakeco"), *installs)
	}
	if ls := pluginLogins(mustPlugin(t)); len(ls) != 1 || ls[0].Lapsed == "" {
		t.Fatalf("the lapsed account on the plugin: %+v", ls)
	}
}

// A move that installed the plugin and then failed takes it away again,
// leaving no plugin provider saying it's signed out; one that got through
// keeps it. A plugin the user had installed stays either way.
func TestMoveTidiesItsPlugin(t *testing.T) {
	ctx, abs, _ := fakePlugin(t)
	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true), fakeLogin("n@fake", "r-noimport", false, true))
	if err := Move(ctx, "fakeco"); err == nil || !strings.Contains(err.Error(), "won't keep") {
		t.Fatalf("Move with a sign-in the plugin won't keep = %v", err)
	}
	if pluginListed(abs) {
		t.Fatal("a failed move left the plugin it installed")
	}
	if s := fakeSaved(t); fakeToken(s["a@fake"]) != "r-a" {
		t.Fatalf("the built-in's account after a failed move: %+v", s)
	}

	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true))
	if err := Move(ctx, "fakeco"); err != nil {
		t.Fatal(err)
	}
	if m, _ := MigrationOf("fakeco"); !m.Installed || !pluginListed(abs) {
		t.Fatalf("after moving: installed %v, listed %v", m.Installed, pluginListed(abs))
	}
	if err := RemovePlugin(ctx, abs); err != nil {
		t.Fatal(err)
	}
	_ = plugin.SignOut(ctx, "fakeco", "")
	os.Remove(migrationsPath())

	// the user's own plugin: a failed move leaves it
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true), fakeLogin("n@fake", "r-noimport", false, true))
	if err := Move(ctx, "fakeco"); err == nil || !pluginListed(abs) {
		t.Fatalf("a failed move onto the user's plugin: %v, listed %v", err, pluginListed(abs))
	}
	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true))
	os.Remove(migrationsPath())
	if err := Move(ctx, "fakeco"); err != nil {
		t.Fatal(err)
	}
	if m, _ := MigrationOf("fakeco"); m.Installed || !pluginListed(abs) {
		t.Fatalf("moved onto the user's plugin: installed %v, listed %v", m.Installed, pluginListed(abs))
	}
}

// There is no way back to a built-in, as each is going. One an older
// magpie moved back is moved again by itself. The plugin turned off keeps
// it moved, off with it; the plugin removed leaves its accounts with the
// plugin's sign-ins, nothing moving or installing it again, and the plugin
// installed again (or the editor's Move) has it at once, accounts and all.
func TestNoWayBack(t *testing.T) {
	ctx, abs, installs := fakePlugin(t)
	retiring = func() []string { return []string{"fakeco"} }
	t.Cleanup(func() { retiring = MovableIDs })
	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true))
	_ = setMigration("fakeco", func(m *Migration) { *m = Migration{State: MovedBack, Package: abs} })
	if errs := MoveRetiring(ctx); errs["fakeco"] != nil || !Moved("fakeco") {
		t.Fatalf("one moved back by an older magpie: %v, moved %v", errs, Moved("fakeco"))
	}
	signedIn := func(when string) {
		t.Helper()
		if len(fakeSaved(t)) != 0 || len(plugin.Auths("fakeco")) != 1 {
			t.Fatalf("%s: built-in's %+v, plugin's %v", when, fakeSaved(t), plugin.Auths("fakeco"))
		}
	}

	if err := SetPluginOff(ctx, abs, true); err != nil {
		t.Fatal(err)
	}
	if !Moved("fakeco") {
		t.Fatal("turning the plugin off moved it back")
	}
	signedIn("plugin off")
	if err := SetPluginOff(ctx, abs, false); err != nil {
		t.Fatal(err)
	}

	if err := RemovePlugin(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if m, _ := MigrationOf("fakeco"); m.State != MoveRemoved || Moved("fakeco") || pluginListed(abs) || len(m.Accounts) != 1 {
		t.Fatalf("plugin removed: %+v, listed %v", m, pluginListed(abs))
	}
	signedIn("plugin removed")
	was := *installs
	keepMovedCurrent(ctx)
	if errs := MoveRetiring(ctx); len(errs) != 0 || *installs != was || pluginListed(abs) {
		t.Fatalf("magpie put back a plugin the user removed: %v, %d installs", errs, *installs-was)
	}

	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if out := HandOver(ctx, false); out["fakeco"] != nil || !Moved("fakeco") {
		t.Fatalf("installed again: %v, moved %v", out, Moved("fakeco"))
	}
	if m, _ := MigrationOf("fakeco"); len(m.Accounts) != 1 || m.Installed {
		t.Fatalf("installed again by the user: %+v", m)
	}
	signedIn("installed again")

	// removed again, then the editor's Move: Adopt, the accounts kept
	if err := RemovePlugin(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if err := Adopt(ctx, "fakeco"); err != nil || !Moved("fakeco") || !pluginListed(abs) {
		t.Fatalf("adopted after removing: %v, moved %v", err, Moved("fakeco"))
	}
	if m, _ := MigrationOf("fakeco"); len(m.Accounts) != 1 || !m.Installed {
		t.Fatalf("adopted after removing: %+v", m)
	}
	signedIn("adopted")
}

// The move tries each account through the vendor, and none holds it
// back: one whose usage read the vendor refuses goes along marked, as a
// lapsed one does; one the plugin couldn't reach its vendor for (its
// models hook keeping a list of its own) goes along unmarked, for the
// plugin's own reads to judge.
func TestMoveTriesEachAccount(t *testing.T) {
	ctx, abs, _ := fakePlugin(t)
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	setFakeLogins(t, fakeLogin("z@fake", "r-gone", true, true))
	if err := Move(ctx, "fakeco"); err != nil || !Moved("fakeco") {
		t.Fatalf("Move with its one account refused = %v", err)
	}
	if ls := pluginLogins(mustPlugin(t)); len(ls) != 1 || ls[0].Lapsed == "" {
		t.Fatalf("the refused account on the plugin: %+v", ls)
	}

	_ = plugin.SignOut(ctx, "fakeco", "")
	os.Remove(migrationsPath())
	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true), fakeLogin("z@fake", "r-unreachable", false, true), fakeLogin("g@fake", "r-gone", false, true))
	if err := Move(ctx, "fakeco"); err != nil || !Moved("fakeco") {
		t.Fatalf("Move with the vendor unreachable for one account = %v", err)
	}
	users := map[string]string{}
	for _, l := range pluginLogins(mustPlugin(t)) {
		users[l.User] = l.Lapsed
	}
	if len(users) != 3 || users["a@fake"] != "" || users["z@fake"] != "" || users["g@fake"] == "" {
		t.Fatalf("moved accounts, user → lapsed: %v", users)
	}
}

// A Zed account's list goes to the plugin with it, and comes back with it:
// the plugin serves the account's own models until it reads them, not the
// families it declares for a sign-in with none.
func TestZedMoveCarriesItsModels(t *testing.T) {
	claudeHome(t)
	list := `{"models":[{"provider":"anthropic","id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5"}]}`
	setFakeLogins(t, savedLogin{Agent: "zed", User: "z@example.com", On: true, First: true,
		Auth: []byte(`{"userId":"u1","accessToken":"a","models":` + list + `}`)})
	out, err := movers["zed"].out()
	if err != nil || len(out) != 1 {
		t.Fatal(out, err)
	}
	var r struct {
		Models json.RawMessage `json:"models"`
	}
	if json.Unmarshal([]byte(str(out[0].Auth["refresh"])), &r) != nil || !strings.Contains(string(r.Models), "claude-sonnet-4-5") {
		t.Fatalf("the refresh carries no list: %v", out[0].Auth["refresh"])
	}
	ls, _, err := movers["zed"].back(nil, "z@example.com", out[0].Auth)
	if err != nil {
		t.Fatal(err)
	}
	c, _ := zedSaved(ls[0])
	if ms, err := zed.ParseModels(c.Models); err != nil || len(ms) != 1 || ms[0].ID != "claude-sonnet-4-5" {
		t.Fatalf("back without its list: %s %v", c.Models, err)
	}
}

// A moved provider shows the built-in's API host, which the move keeps,
// not plugin://<id>'s: "" for one that had none, as Zed's.
func TestMovedShowsBuiltinHost(t *testing.T) {
	claudeHome(t)
	pp := plugin.Provider{ID: "factory", Models: []plugin.Model{{ID: "m"}}}
	if h := pluginProvider(pp, pluginLogin{}).Host(); h != "factory" {
		t.Fatalf("a plugin's own provider: %q", h)
	}
	// the host the move kept, else (moved before it kept one) the built-in's
	for host, want := range map[string]string{"llm.factory.test": "llm.factory.test", "": HostOf(factoryAPI)} {
		_ = setMigration("factory", func(m *Migration) { *m = Migration{State: MovePlugin, Host: host} })
		p := pluginProvider(pp, pluginLogin{})
		if h := p.Host(); h != want || want == "" {
			t.Fatalf("moved: %q, want %q", h, want)
		}
		if w := p.Where(); w != want {
			t.Fatalf("moved calls go to %q, want %q", w, want)
		}
	}
	// one that showed no host shows none
	_ = setMigration("devin", func(m *Migration) { *m = Migration{State: MovePlugin} })
	if h := pluginProvider(plugin.Provider{ID: "devin"}, pluginLogin{}).Host(); h != "" {
		t.Fatalf("moved devin: %q", h)
	}
}

// A built-in with no account signed in goes onto its plugin as it is: the
// plugin installed and the subscription its from then on, so the first
// sign-in is the plugin's. One with an account is moved.
func TestAdoptBeforeSigningIn(t *testing.T) {
	ctx, abs, installs := fakePlugin(t)
	if err := Adopt(ctx, "fakeco"); err != nil {
		t.Fatal(err)
	}
	if m, _ := MigrationOf("fakeco"); m.State != MovePlugin || !m.Installed || !pluginListed(abs) || *installs != 1 {
		t.Fatalf("adopted: %+v, listed %v, %d installs", m, pluginListed(abs), *installs)
	}
	if PluginID("fakeco") != "fakeco" {
		t.Fatalf("the plugin's provider is %s", PluginID("fakeco"))
	}
	if err := Adopt(ctx, "fakeco"); err != nil || *installs != 1 {
		t.Fatalf("adopting again: %v, %d installs", err, *installs)
	}
	os.Remove(migrationsPath())
	setFakeLogins(t, fakeLogin("a@fake", "r-a", true, true))
	if err := Adopt(ctx, "fakeco"); err != nil {
		t.Fatal(err)
	}
	if !Moved("fakeco") || !plugin.SignedIn("fakeco") || len(fakeSaved(t)) != 0 {
		t.Fatalf("adopting one signed in didn't move it: moved %v, saved %+v", Moved("fakeco"), fakeSaved(t))
	}
}
