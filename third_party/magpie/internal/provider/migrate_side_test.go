package provider

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/update"
)

// moveAndBack does what Move and MoveBack do with a mover's accounts, but
// for the plugin in between: the accounts go out, the built-in's saved ones
// go aside, extra (sign-ins made through the plugin since) join them, and
// every one comes back. It gives the accounts that went out and the saved
// accounts before and after.
func moveAndBack(t *testing.T, id string, extra ...map[string]any) (out []Moving, before, after []savedLogin) {
	t.Helper()
	mv := movers[id]
	out, err := mv.out()
	if err != nil {
		t.Fatal(err)
	}
	ls := readLogins()
	before = slices.Clone(ls)
	var backup []savedLogin
	ls = slices.DeleteFunc(ls, func(l savedLogin) bool {
		if slices.Contains(mv.agents, l.Agent) {
			backup = append(backup, l)
			return true
		}
		return false
	})
	if err := writeLogins(ls); err != nil {
		t.Fatal(err)
	}
	// through the plugin, which keeps its sign-ins as JSON
	var auths []map[string]any
	for _, a := range out {
		if !a.Own {
			auths = append(auths, roundJSON(t, a.Auth))
		}
	}
	ls = readLogins()
	users := map[int]string{}
	for i, a := range out {
		if a.Own {
			continue
		}
		next, u, err := mv.back(ls, a.User, auths[0])
		auths = auths[1:]
		if err != nil {
			t.Fatalf("%s back: %v", a.User, err)
		}
		ls, users[i] = next, u
	}
	for _, a := range extra {
		next, _, err := mv.back(ls, "", roundJSON(t, a))
		if err != nil {
			t.Fatalf("back %v: %v", a, err)
		}
		ls = next
	}
	for _, b := range backup {
		if !slices.ContainsFunc(ls, func(l savedLogin) bool { return sameMoved(l, b) }) {
			ls = append(ls, b)
		}
	}
	for i, a := range out {
		if u := users[i]; u != "" && !strings.EqualFold(u, a.User) {
			t.Errorf("%s went back as %s", a.User, u)
		}
	}
	if err := writeLogins(ls); err != nil {
		t.Fatal(err)
	}
	return out, before, readLogins()
}

func roundJSON(t *testing.T, a map[string]any) map[string]any {
	t.Helper()
	var out map[string]any
	if err := json.Unmarshal([]byte(jsonText(a)), &out); err != nil {
		t.Fatal(err)
	}
	return out
}

func loginsOfAgent(ls []savedLogin, agent string) []savedLogin {
	return slices.DeleteFunc(slices.Clone(ls), func(l savedLogin) bool { return l.Agent != agent })
}

func TestMoveDevin(t *testing.T) {
	home := claudeHome(t)
	data := filepath.Join(home, "data")
	t.Setenv("XDG_DATA_HOME", data)
	os.MkdirAll(filepath.Join(data, "devin"), 0o700)
	os.WriteFile(DevinCredentialsPath(), devinCredentials("key-own", "", "", ""), 0o600)
	exe := filepath.Join(home, "devin")
	testenv.Program(t, exe, "#!/bin/sh\n"+devinSigned+"\n")
	fakeDevin(t, exe)
	// the CLI's account is read by running it, which a loaded machine may
	// take longer than a look's first wait for
	old := firstAsk
	firstAsk = time.Minute
	t.Cleanup(func() { firstAsk = old })
	two, _ := newDevinHome()
	os.WriteFile(devinCredentialsAt(two), devinCredentials("key-two", "https://eu.codeium.com", "", ""), 0o600)
	writeLogins([]savedLogin{{Agent: "devin", User: "two@example.com", Plan: "Devin Max", Home: two, On: true, First: true}})

	out, before, after := moveAndBack(t, "devin",
		// the CLI's account, its key since rotated in the CLI: still the CLI's
		map[string]any{"type": "api", "key": "key-stale", "metadata": map[string]any{"email": "dev@example.com", "cli": true}},
		map[string]any{"type": "api", "key": "key-three", "metadata": map[string]any{"email": "three@example.com", "plan": "Pro"}})
	if len(out) != 2 || out[0].User != "two@example.com" || !out[0].First || out[0].Own ||
		out[0].Auth["key"] != "key-two" || out[0].Auth["metadata"].(map[string]any)["server"] != "https://eu.codeium.com" ||
		out[0].Auth["metadata"].(map[string]any)["plan"] != "Devin Max" ||
		out[1].User != "dev@example.com" || !out[1].Own || out[1].Auth["key"] != "key-own" || out[1].Auth["metadata"].(map[string]any)["server"] != nil ||
		out[1].Auth["metadata"].(map[string]any)["cli"] != true || out[0].Auth["metadata"].(map[string]any)["cli"] != nil {
		t.Fatalf("out %+v", out)
	}
	ls := loginsOfAgent(after, "devin")
	if len(ls) != len(loginsOfAgent(before, "devin"))+1 {
		t.Fatalf("back %+v\nbefore %+v", ls, before)
	}
	for _, l := range ls {
		switch l.User {
		case "two@example.com":
			if l.Home != two { // the home it had, not a new one
				t.Errorf("two's home %s, was %s", l.Home, two)
			}
		case "dev@example.com":
			if !l.own() {
				t.Errorf("own %+v", l)
			}
		case "three@example.com":
			if k, _, err := DevinAuthAt(l.Home); err != nil || k != "key-three" || l.Plan != "Pro" || !l.On {
				t.Errorf("three %+v %q %v", l, k, err)
			}
		default:
			t.Errorf("unexpected %+v", l)
		}
	}
	if k, s, _ := DevinAuthAt(two); k != "key-two" || s != "https://eu.codeium.com" {
		t.Fatalf("two's credentials %s %s", k, s)
	}
	if b, _ := os.ReadFile(DevinCredentialsPath()); !strings.Contains(string(b), "key-own") {
		t.Fatalf("the CLI's own changed: %s", b)
	}
}

func TestMoveGrok(t *testing.T) {
	home := signIn(t)
	t.Setenv("GROK_HOME", filepath.Join(home, ".grok"))
	grokSignedIn(t, GrokHome(), "me@x.ai")
	extra, _ := newGrokHome()
	grokSignedIn(t, extra, "two@x.ai")
	if _, err := addGrokLogin(extra); err != nil {
		t.Fatal(err)
	}
	// a sign-in made through the plugin, in a home of the plugin's
	pluginHome := filepath.Join(home, ".cache", "opencode-grok-auth", "h1")
	grokSignedIn(t, pluginHome, "three@x.ai")

	out, before, after := moveAndBack(t, "grok",
		map[string]any{"type": "oauth", "refresh": GrokHome(), "access": "k", "expires": 0, "accountId": "me@x.ai"},
		map[string]any{"type": "oauth", "refresh": pluginHome, "access": "k-three@x.ai", "expires": 0, "accountId": "three@x.ai"})
	if len(out) != 2 || out[0].User != "me@x.ai" || !out[0].Own || out[0].Auth["refresh"] != GrokHome() ||
		out[1].User != "two@x.ai" || out[1].Own || out[1].Auth["refresh"] != extra || out[1].Auth["access"] != "k-two@x.ai" ||
		out[1].Auth["expires"].(int64) < time.Now().UnixMilli() {
		t.Fatalf("out %+v", out)
	}
	if _, err := os.Stat(filepath.Join(extra, "auth.json")); err != nil {
		t.Fatalf("the moved account's home is gone: %v", err)
	}
	ls := loginsOfAgent(after, "grok")
	if len(ls) != len(loginsOfAgent(before, "grok"))+1 {
		t.Fatalf("back %+v\nbefore %+v", ls, before)
	}
	got := map[string]string{}
	for _, l := range ls {
		got[l.User] = l.Home
	}
	if got["two@x.ai"] != extra || got["three@x.ai"] != pluginHome || got["me@x.ai"] != "" {
		t.Fatalf("homes %v", got)
	}
	if gl := grokLogins(); len(gl) != 3 {
		t.Fatalf("grok logins %+v", gl)
	}
}

func TestMoveCommandCode(t *testing.T) {
	home := claudeHome(t)
	writeFile(t, filepath.Join(home, ".commandcode", "auth.json"), cmdAuth{APIKey: "cc-own", UserID: "u1", UserName: "me"})
	writeLogins([]savedLogin{{Agent: CommandCodePlanID, User: "two", Plan: "Pro", On: true, Auth: []byte(jsonText(cmdAuth{APIKey: "cc-two", UserID: "u2", UserName: "two", KeyName: "laptop"}))}})

	out, before, after := moveAndBack(t, CommandCodePlanID,
		map[string]any{"type": "api", "key": "cc-stale", "metadata": map[string]any{"email": "me", "cli": true}},
		map[string]any{"type": "api", "key": "cc-three", "metadata": map[string]any{"email": "three", "userId": "u3"}})
	if len(out) != 2 || out[0].User != "me" || !out[0].Own || !out[0].First || out[0].Auth["key"] != "cc-own" ||
		out[1].User != "two" || out[1].Own || out[1].Auth["key"] != "cc-two" || out[1].Auth["metadata"].(map[string]any)["keyName"] != "laptop" ||
		out[0].Auth["metadata"].(map[string]any)["cli"] != true || out[1].Auth["metadata"].(map[string]any)["cli"] != nil {
		t.Fatalf("out %+v", out)
	}
	ls := loginsOfAgent(after, CommandCodePlanID)
	if len(ls) != len(loginsOfAgent(before, CommandCodePlanID))+1 {
		t.Fatalf("back %+v\nbefore %+v", ls, before)
	}
	for _, l := range ls {
		a, _ := cmdSaved(l)
		switch l.User {
		case "me":
			if !l.own() {
				t.Errorf("own %+v", l)
			}
		case "two":
			if a != (cmdAuth{APIKey: "cc-two", UserID: "u2", UserName: "two", KeyName: "laptop"}) || l.Plan != "Pro" {
				t.Errorf("two %+v %+v", l, a)
			}
		case "three":
			if a.APIKey != "cc-three" || a.UserID != "u3" || a.UserName != "three" {
				t.Errorf("three %+v", a)
			}
		default:
			t.Errorf("unexpected %+v", l)
		}
	}
	if l := cmdLogins(); len(l) != 3 {
		t.Fatalf("cmd logins %+v", l)
	}
	// 0.1.7 and before said every model took no pictures (the list says
	// nothing of them), which magpie takes over models.dev's answer
	if update.Newer("0.1.8", movers[CommandCodePlanID].min) {
		t.Fatalf("the move installs commandcode-auth %q, which marks every model text-only", movers[CommandCodePlanID].min)
	}
}

func TestMoveCursor(t *testing.T) {
	claudeHome(t)
	wasExe, wasOut, was := CursorExecutable, cursorSignedOut, cursorStatus
	t.Cleanup(func() { CursorExecutable, cursorSignedOut, cursorStatus = wasExe, wasOut, was })
	CursorExecutable = func() string { return "/bin/sh" }
	cursorSignedOut = func() bool { return false }
	cursorStatus = &cliIdentity{name: "cursor-test", exe: func() string { return "/bin/sh" }, ask: func() (string, string, bool, error) { return "me@example.com", "Pro", true, nil }}

	out, _, after := moveAndBack(t, "cursor",
		map[string]any{"type": "oauth", "access": "", "refresh": "cursor-agent", "expires": 0, "accountId": "me@example.com"})
	if len(out) != 1 || !out[0].Own || !out[0].First || out[0].User != "me@example.com" ||
		out[0].Auth["refresh"] != "cursor-agent" || out[0].Auth["access"] != "" || out[0].Auth["plan"] != "Pro" {
		t.Fatalf("out %+v", out)
	}
	if len(loginsOfAgent(after, "cursor")) != 0 {
		t.Fatalf("cursor saved %+v", after)
	}
	// a token the plugin signed in itself has nowhere to go
	if _, _, err := movers["cursor"].back(nil, "", map[string]any{"type": "oauth", "access": "a", "refresh": "r", "accountId": "x@example.com"}); err == nil {
		t.Fatal("a browser sign-in went back")
	}
	cursorSignedOut = func() bool { return true }
	if out, _ := movers["cursor"].out(); len(out) != 0 {
		t.Fatalf("signed out: %+v", out)
	}
}

// The Cursor plugin lists Cursor's picker, a model once per context size,
// and runs the ids the built-in listed as the variants they stood for: a
// move doesn't fail on those (#1656). The list is what the plugin's 0.2.7
// lists for its own test picker (context.test.mjs PICKER); each id said
// served here is one that plugin's request() resolves to a variant of
// Cursor's (grok-4.7-fast to grok-4.7-high-fast, cursor-grok-4.6 to
// cursor-grok-4.6-high), run against it.
func TestCursorMoverServesBuiltinIDs(t *testing.T) {
	listed := []string{"claude-opus-5-5@300k", "claude-opus-5-5@1m", "claude-haiku-5-5@300k", "claude-haiku-5-5@1m", "grok-4.7@256k", "grok-4.7@500k", "grok-4.6", "composer-2.5", "auto", "max-only"}
	served := movers["cursor"].served
	if served == nil {
		t.Fatal("the Cursor mover has no served hook")
	}
	for _, id := range []string{
		"grok-4.7", "grok-4.7-fast", "grok-4.7-low", "grok-4.7-high-fast", // a family, fast, a stale effort
		"claude-opus-5-5", "claude-opus-5-5-fast", "claude-opus-5-5-high",
		"claude-haiku-5-5-thinking", "claude-haiku-5-5-thinking-low",
		"cursor-grok-4.6", "cursor-grok-4.6-high", // Cursor's own prefix the picker's name hasn't
		"composer-2.5-fast", "default", "auto", "max-only",
	} {
		if !served(id, listed) {
			t.Errorf("%s: not served, though the plugin runs it", id)
		}
	}
	// a family the picker hasn't got is still refused
	for _, id := range []string{"claude-fable-5-thinking-no-zdr", "codex-5.3-fast", "grok-4.5", "gpt-5.5-high", "claude-opus-5", "fast", "-fast"} {
		if served(id, listed) {
			t.Errorf("%s: served, though the plugin lists nothing of it", id)
		}
	}
	// Auto only when the plugin lists it
	if served("default", []string{"grok-4.7@256k"}) {
		t.Error("default served by a list without Auto")
	}
}
