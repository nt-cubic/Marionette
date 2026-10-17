package provider

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/zed"
)

// fakeMover moves a made-up built-in, "fakeco", whose accounts keep a
// token in logins.json, onto the test plugin: the token is the plugin's
// refresh token.
func fakeMover(t *testing.T) {
	t.Helper()
	movers["fakeco"] = &mover{
		pkg:    "fake",
		agents: []string{"fakeco"},
		out: func() ([]Moving, error) {
			var out []Moving
			for _, l := range sideLogins("fakeco", "", func(l savedLogin) bool { return fakeToken(l) != "" }) {
				out = append(out, Moving{User: l.User, First: l.Active, On: l.On, Lapsed: l.saved.Lapsed != "", Plan: l.Plan, Auth: map[string]any{
					"type": "oauth", "refresh": fakeToken(l.saved), "access": "a", "expires": 9e15, "accountId": l.User,
				}})
			}
			return out, nil
		},
		back: func(ls []savedLogin, user string, auth map[string]any) ([]savedLogin, string, error) {
			if user == "" {
				user = str(auth["accountId"])
			}
			i := backInto(&ls, "fakeco", user)
			ls[i].Auth = json.RawMessage(jsonText(map[string]string{"token": str(auth["refresh"])}))
			return ls, ls[i].User, nil
		},
	}
	oldInstall, oldImport := installPlugin, importAuth
	installPlugin = func(context.Context, string, string) error { return nil }
	// a sign-in "r-noimport" is one the plugin won't keep, failing the
	// move part way, after the accounts before it went over
	importAuth = func(ctx context.Context, id string, auth map[string]any) (string, error) {
		if auth["refresh"] == "r-noimport" {
			return "", errors.New("FakeCo won't keep this sign-in")
		}
		return plugin.Import(ctx, id, auth)
	}
	t.Cleanup(func() {
		delete(movers, "fakeco")
		installPlugin, importAuth = oldInstall, oldImport
	})
}

func fakeToken(l savedLogin) string {
	var a struct{ Token string }
	_ = json.Unmarshal(l.Auth, &a)
	return a.Token
}

func fakeLogin(user, token string, first, on bool) savedLogin {
	return savedLogin{Agent: "fakeco", User: user, First: first, On: on, Auth: json.RawMessage(jsonText(map[string]string{"token": token}))}
}

func fakeSaved(t *testing.T) map[string]savedLogin {
	t.Helper()
	out := map[string]savedLogin{}
	for _, l := range readLogins() {
		if l.Agent == "fakeco" {
			out[l.User] = l
		}
	}
	return out
}

// A built-in moves onto its plugin only when every account works through
// it and it serves every model in use; anything short of that puts the
// built-in back as it was, with the tokens the plugin renewed meanwhile.
// Moved, the plugin has the accounts in the user's order; moved back, the
// built-in has them again, fresh tokens and order included.
func TestMoveToPlugin(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	fakeMover(t)
	movers["fakeco"].pkg = abs
	reset := func(ls ...savedLogin) {
		t.Helper()
		_ = plugin.SignOut(ctx, "fakeco", "")
		os.Remove(migrationsPath())
		loginsMu.Lock()
		if err := writeLogins(ls); err != nil {
			t.Fatal(err)
		}
		loginsMu.Unlock()
	}
	pluginUsers := func() []string {
		pp, _ := pluginOfAgent("plugin:fakeco")
		pp.Accounts = nil
		for _, p := range plugin.Cached() {
			if p.ID == "fakeco" {
				pp = p
			}
		}
		var out []string
		for _, l := range pluginLogins(pp) {
			s := l.User
			if l.On {
				s += "+"
			}
			out = append(out, s)
		}
		return out
	}

	// an account the vendor refuses goes along: the built-in is retired,
	// so nothing an account does holds the move back
	reset(fakeLogin("a@fake", "rot-a", false, true), fakeLogin("c@fake", "r-dead", false, true))
	if err := Move(ctx, "fakeco"); err != nil || !Moved("fakeco") {
		t.Fatalf("Move with a refused account = %v", err)
	}
	if got := strings.Join(pluginUsers(), " "); got != "a@fake+ c@fake+" {
		t.Fatalf("the plugin's accounts after moving a refused one: %s", got)
	}

	// a sign-in the plugin won't keep: nothing moves, and the one that went
	// over before it comes back
	reset(fakeLogin("a@fake", "rot-a", false, true), fakeLogin("c@fake", "r-noimport", false, true))
	err := Move(ctx, "fakeco")
	if err == nil || !strings.Contains(err.Error(), "c@fake") {
		t.Fatalf("Move with a sign-in the plugin won't keep = %v", err)
	}
	if m, _ := MigrationOf("fakeco"); m.State != MoveFailed || Moved("fakeco") {
		t.Fatalf("after a failed move: %+v", m)
	}
	if s := fakeSaved(t); !strings.HasPrefix(fakeToken(s["a@fake"]), "rot-a") || fakeToken(s["c@fake"]) != "r-noimport" {
		t.Fatalf("the built-in's accounts after a failed move: %+v", s)
	}
	if plugin.SignedIn("fakeco") {
		t.Fatal("the plugin kept an account of a failed move")
	}

	// an account signed in through the plugin already gets back what it had
	reset(fakeLogin("a@fake", "r-a", false, true), fakeLogin("c@fake", "r-noimport", false, true))
	if _, err := plugin.Import(ctx, "fakeco", map[string]any{"type": "oauth", "refresh": "r-mine", "access": "a", "expires": 9e15, "accountId": "a@fake"}); err != nil {
		t.Fatal(err)
	}
	if err := Move(ctx, "fakeco"); err == nil {
		t.Fatal("moved a sign-in the plugin won't keep")
	}
	if a := plugin.Auths("fakeco"); len(a) != 1 || a["fakeco"]["refresh"] != "r-mine" {
		t.Fatalf("the plugin's own account after a failed move: %v", a)
	}

	// a move through: b first, a on behind it, d off; a lapsed one goes
	// along untried
	reset(fakeLogin("a@fake", "rot-a", false, true), fakeLogin("b@fake", "r-b", true, true), func() savedLogin { l := fakeLogin("d@fake", "r-d", false, false); l.Plan = "Fake Max"; return l }(),
		func() savedLogin { l := fakeLogin("e@fake", "r-dead", false, false); l.Lapsed = "expired"; return l }())
	if err := Move(ctx, "fakeco"); err != nil {
		t.Fatal(err)
	}
	if !Moved("fakeco") || PluginID("fakeco") != "fakeco" {
		t.Fatal("not moved")
	}
	if s := fakeSaved(t); len(s) != 0 {
		t.Fatalf("the built-in kept its accounts in logins.json: %v", s)
	}
	if m, _ := MigrationOf("fakeco"); len(m.Backup) != 4 || len(m.Accounts) != 4 {
		t.Fatalf("the move's record: %+v", m)
	}
	if got := strings.Join(pluginUsers(), " "); got != "b@fake+ a@fake+ d@fake e@fake" {
		t.Fatalf("the plugin's accounts: %s", got)
	}
	// each keeps the plan it showed, and a refused one stays marked so
	for _, l := range pluginLogins(mustPlugin(t)) {
		if (l.User == "d@fake") != (l.Plan == "Fake Max") || (l.User == "e@fake") != (l.Lapsed != "") {
			t.Fatalf("moved %s: plan %q, lapsed %q", l.User, l.Plan, l.Lapsed)
		}
	}
	if err := Move(ctx, "fakeco"); err != nil {
		t.Fatalf("moving again: %v", err)
	}
	// a built-in with no usage card of its own (Devin) keeps the plugin's
	cards := 0
	qs := fetchSubscriptionUsage(context.Background())
	for _, q := range qs {
		if q.Provider == "fakeco" {
			cards++
		}
	}
	if cards == 0 {
		t.Fatalf("the moved plugin's accounts show no usage: moved %v, plugins %d, logins %d, cards %+v",
			Moved("fakeco"), len(plugin.Cached()), len(pluginUsageLogins(mustPlugin(t))), qs)
	}

}

// A move cut short (magpie quit mid-way) is put back before it is tried
// again, and a failed one waits before magpie tries it by itself again.
func TestMoveCutShort(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	fakeMover(t)
	movers["fakeco"].pkg = abs
	retiring = func() []string { return []string{"fakeco"} }
	t.Cleanup(func() { retiring = MovableIDs })
	loginsMu.Lock()
	_ = writeLogins([]savedLogin{fakeLogin("a@fake", "rot-a", false, true)})
	loginsMu.Unlock()

	// cut short after a's sign-in went over and the plugin renewed it
	key, err := plugin.Import(ctx, "fakeco", map[string]any{"type": "oauth", "refresh": "rot-a+", "access": "a", "expires": 9e15, "accountId": "a@fake"})
	if err != nil {
		t.Fatal(err)
	}
	// as a move records it: the sign-in sent, which the built-in still has
	out, _ := movers["fakeco"].out()
	_ = setMigration("fakeco", func(m *Migration) {
		*m = Migration{State: MoveMoving, At: time.Now(), Accounts: []movedAccount{{Key: key, User: "a@fake", Sent: signinHash(out[0].Auth)}}}
	})
	if movingNow("fakeco") && len(pluginAccounts()) != 0 {
		t.Fatal("a plugin provider mid-move is listed")
	}
	// the move fails now (a sign-in the plugin won't keep): put back,
	// with the renewed token
	loginsMu.Lock()
	_ = writeLogins(append(readLogins(), fakeLogin("n@fake", "r-noimport", false, true)))
	loginsMu.Unlock()
	if errs := MoveRetiring(ctx); errs["fakeco"] == nil {
		t.Fatal("moved a sign-in the plugin won't keep")
	}
	if s := fakeSaved(t); !strings.HasPrefix(fakeToken(s["a@fake"]), "rot-a+") || plugin.SignedIn("fakeco") {
		t.Fatalf("after putting back a move cut short: %+v", s)
	}
	// failed just now: not tried again by itself until moveRetry has gone
	loginsMu.Lock()
	_ = writeLogins(slices.DeleteFunc(readLogins(), func(l savedLogin) bool { return l.User == "n@fake" }))
	loginsMu.Unlock()
	if errs := MoveRetiring(ctx); len(errs) != 0 || Moved("fakeco") {
		t.Fatalf("tried again at once: %v", errs)
	}
	_ = setMigration("fakeco", func(m *Migration) { m.At = time.Now().Add(-moveRetry - time.Minute) })
	if errs := MoveRetiring(ctx); errs["fakeco"] != nil || !Moved("fakeco") {
		t.Fatalf("after moveRetry: %v", errs)
	}
	// a magpie that stopped between the record and logins.json: tidied
	loginsMu.Lock()
	_ = writeLogins(append(readLogins(), fakeLogin("late@fake", "r-l", false, true)))
	loginsMu.Unlock()
	MoveRetiring(ctx)
	if m, _ := MigrationOf("fakeco"); len(fakeSaved(t)) != 0 || len(m.Backup) != 2 {
		t.Fatalf("not tidied: %v, %+v", fakeSaved(t), m.Backup)
	}
}

func mustPlugin(t *testing.T) plugin.Provider {
	t.Helper()
	for _, p := range plugin.Cached() {
		if p.ID == "fakeco" {
			return p
		}
	}
	t.Fatal("no fakeco plugin provider")
	return plugin.Provider{}
}

// Putting back a move keeps the newer of the two sign-ins: the plugin's
// when it renewed one, the built-in's when that renewed it meanwhile (a
// request went to it), none when neither did.
func TestPutBackKeepsNewer(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	fakeMover(t)
	mv := movers["fakeco"]
	sent := func(tok, user string) string {
		return signinHash(map[string]any{"type": "oauth", "refresh": tok, "access": "a", "expires": 9e15, "accountId": user})
	}
	for _, c := range []struct {
		name, builtin, plugin, want string
	}{
		{"the plugin renewed it", "rot-a", "rot-a+", "rot-a+"},
		{"the built-in renewed it", "rot-b", "rot-a+", "rot-b"},
		{"neither did", "rot-a", "rot-a", "rot-a"},
	} {
		loginsMu.Lock()
		_ = writeLogins([]savedLogin{fakeLogin("a@fake", c.builtin, false, true)})
		loginsMu.Unlock()
		key, err := plugin.Import(ctx, "fakeco", map[string]any{"type": "oauth", "refresh": c.plugin, "access": "a", "expires": 9e15, "accountId": "a@fake"})
		if err != nil {
			t.Fatal(err)
		}
		_ = setMigration("fakeco", func(m *Migration) {
			*m = Migration{State: MoveMoving, At: time.Now(), Accounts: []movedAccount{{Key: key, User: "a@fake", Sent: sent("rot-a", "a@fake")}}}
		})
		putBack(ctx, "fakeco", mv, "")
		if s := fakeSaved(t); fakeToken(s["a@fake"]) != c.want || plugin.SignedIn("fakeco") {
			t.Fatalf("%s: %+v", c.name, s)
		}
		if m, _ := MigrationOf("fakeco"); m.State != MoveFailed {
			t.Fatalf("%s: %+v", c.name, m)
		}
	}
}

// A built-in moved onto its plugin shows no card of its own, even with
// an account of it still on this machine (the agent's own sign-in), and
// signs in no more: an account signed in to there would be served by
// nothing.
func TestMovedBuiltinQuiet(t *testing.T) {
	claudeHome(t)
	loginsMu.Lock()
	_ = writeLogins([]savedLogin{{Agent: "zed", User: "left@example.com", On: true, First: true, Auth: []byte(`{"userId":"u1","accessToken":"a"}`)}})
	loginsMu.Unlock()
	zedCloud = "http://127.0.0.1:1" // no network: the card shows its error
	t.Cleanup(func() { zedCloud = zed.CloudURL })
	cards := func() int {
		n := 0
		for _, q := range fetchSubscriptionUsage(context.Background()) {
			if q.Provider == "zed" {
				n++
			}
		}
		return n
	}
	if cards() != 1 {
		t.Fatal("the built-in's card isn't there to go")
	}
	_ = setMigration("zed", func(m *Migration) { *m = Migration{State: MovePlugin, At: time.Now()} })
	if n := cards(); n != 0 {
		t.Fatalf("%d cards of the moved built-in", n)
	}
	if _, err := StartSignIn("zed"); err == nil || !strings.Contains(err.Error(), "plugin") {
		t.Fatalf("a sign-in to the moved built-in: %v", err)
	}
}

// A moved built-in's plugin is updated to the version its move needs, as
// the built-in came up to date with magpie; one whose plugin the user
// removed isn't installed again.
func TestKeepMovedCurrent(t *testing.T) {
	claudeHome(t)
	fakeMover(t)
	movers["fakeco"].min = "0.2.0"
	var asked []string
	installPlugin = func(_ context.Context, pkg, min string) error {
		asked = append(asked, pkg+"@"+min)
		return nil
	}
	_ = setMigration("fakeco", func(m *Migration) { m.State = MoveRemoved })
	keepMovedCurrent(context.Background())
	if len(asked) != 0 {
		t.Fatalf("one whose plugin was removed was installed: %v", asked)
	}
	_ = setMigration("fakeco", func(m *Migration) { m.State = MovePlugin })
	keepMovedCurrent(context.Background())
	if strings.Join(asked, " ") != "fake@0.2.0" {
		t.Fatalf("asked %v, want fake@0.2.0", asked)
	}
}

// The plugin a move installs is installed at the spec the user pinned, not at
// <pkg>@latest. Add installs what it is given and appends @latest to a bare
// package name, so handing it a bare name replaced the pin with the newest
// version behind the user's back, and switched one they turned off back on.
func TestInstallPluginKeepsTheUsersPin(t *testing.T) {
	claudeHome(t)
	// where plugin.Load reads the list from: settings.Dir(), the plugins
	// folder's parent. The installed package's package.json is what
	// plugin.Version reads, so it is where the plugins folder keeps it.
	list := filepath.Join(filepath.Dir(plugin.Dir()), "plugins.json")
	pkg := filepath.Join(plugin.Dir(), "node_modules", "fake")
	writeList := func(body string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(list), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(list, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	writeInstalled := func(version string) {
		t.Helper()
		if err := os.MkdirAll(pkg, 0o700); err != nil {
			t.Fatal(err)
		}
		pj := `{"name":"fake","version":"` + version + `"}`
		if err := os.WriteFile(filepath.Join(pkg, "package.json"), []byte(pj), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	writeList(`{"plugins":[{"spec":"fake@0.1.5"}]}`)
	writeInstalled("0.1.5")
	oldAdd, oldUp := addPlugin, upgradePlugin
	t.Cleanup(func() { addPlugin, upgradePlugin = oldAdd, oldUp })
	var asked []string
	addPlugin = func(_ context.Context, spec string) (plugin.Entry, error) {
		asked = append(asked, "add "+spec)
		return plugin.Entry{}, nil
	}
	upgradePlugin = func(_ context.Context, name string) error {
		asked = append(asked, "upgrade "+name)
		return nil
	}

	// what is installed is the user's pin, and the move needs newer: the
	// pin stays, and the move says why it can't go on — rather than Add
	// quietly installing <pkg>@latest over it
	err := installPlugin(context.Background(), "fake", "0.2.0")
	if len(asked) != 0 {
		t.Fatalf("a pinned plugin was reinstalled: %v", asked)
	}
	if err == nil || !strings.Contains(err.Error(), "pinned at fake@0.1.5") || !strings.Contains(err.Error(), "0.2.0 or newer") {
		t.Fatalf("a pin older than the move needs: %v, want the pin and the version it needs named", err)
	}

	// one the user didn't pin still goes to npm's newest through Upgrade,
	// not bun's latest, which a minimumReleaseAge holds back without a
	// word (sweanng424 on Discord)
	writeList(`{"plugins":[{"spec":"fake@latest"}]}`)
	asked = nil
	_ = installPlugin(context.Background(), "fake", "0.2.0")
	if strings.Join(asked, " ") != "upgrade fake" {
		t.Fatalf("an unpinned plugin below the move's min: asked %v, want upgrade fake", asked)
	}

	// a git one is reinstalled from its own repository, not swapped for
	// npm's; the plugins folder's package.json is what names it
	writeList(`{"plugins":[{"spec":"github:someone/fake"}]}`)
	deps := `{"dependencies":{"fake":"github:someone/fake"}}`
	if err := os.WriteFile(filepath.Join(plugin.Dir(), "package.json"), []byte(deps), 0o600); err != nil {
		t.Fatal(err)
	}
	asked = nil
	_ = installPlugin(context.Background(), "fake", "0.2.0")
	if strings.Join(asked, " ") != "add github:someone/fake" {
		t.Fatalf("a git plugin below the move's min: asked %v, want its own spec added", asked)
	}

	// a plugin the user turned off is not switched back on either
	writeList(`{"plugins":[{"spec":"fake@0.1.5","off":true}]}`)
	asked = nil
	if err := installPlugin(context.Background(), "fake", "0.2.0"); err == nil {
		t.Fatal("a plugin the user turned off was installed")
	}
	if len(asked) != 0 {
		t.Fatalf("a plugin the user turned off was asked for: %v", asked)
	}
}
