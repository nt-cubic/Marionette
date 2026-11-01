package middleware

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/gwmw"
)

// forgetRefKeys makes the next ref read its key from the file again, as a
// new start of magpie does.
func forgetRefKeys() {
	refKeys.Lock()
	clear(refKeys.m)
	refKeys.Unlock()
}

// what onProvider was told of the account after onAccount: each
// middleware's ctx.account, "none" when it had none
type told struct {
	A any    `json:"a"`
	B string `json:"b"`
}

func triedAfter(t *testing.T, r *Run, a gwmw.Account) (skip bool, by string, got told) {
	t.Helper()
	skip, by = r.Account(a)
	if !skip {
		json.Unmarshal(r.Tried([]byte(`{}`), gwmw.To{ID: a.ID, Model: a.Model}), &got)
	}
	return
}

// onAccount is told a ref for the account, never its email, the same on
// every request and after magpie starts again, and a different one for
// each middleware; {skip: true} passes the account over, naming the
// middleware, and anything else, a throw and null among them, uses it.
func TestOnAccount(t *testing.T) {
	forgetRefKeys()
	t.Cleanup(forgetRefKeys)
	a := file(t, `
export function onAccount(account, ctx) {
  if (account.provider === "throw") throw new Error("bad");
  if (account.provider === "null") return null;
  if (account.provider === "undef") return;
  if (account.provider === "skip") return {skip: true};
  if (account.provider === "no") return {skip: false};
  return {};
}
export function onProvider(body, ctx) { body.a = ctx.account === undefined ? "none" : ctx.account; return body; }`)
	b := file(t, `
export function onAccount(account) {}
export function onProvider(body, ctx) { body.b = ctx.account === undefined ? "none" : ctx.account.ref; return body; }`)
	use(t, a, b)
	me := gwmw.Account{Agent: "codex", User: "me@example.com", Plan: "plus", ID: "codex", Name: "Codex", Model: "gpt-5.5"}

	refs := func() (string, string) {
		t.Helper()
		r := Begin(info())
		if r == nil {
			t.Fatal("no run")
		}
		defer r.End()
		skip, _, got := triedAfter(t, r, me)
		if skip {
			t.Fatal("skipped")
		}
		raw, _ := json.Marshal(got)
		if strings.Contains(strings.ToLower(string(raw)), "example.com") || strings.Contains(string(raw), "plus") {
			t.Fatalf("the account's email or plan reached a middleware: %s", raw)
		}
		ac, _ := got.A.(map[string]any)
		if ac["provider"] != "codex" || ac["name"] != "Codex" || ac["kind"] != "codex" || ac["model"] != "gpt-5.5" {
			t.Fatalf("ctx.account %s", raw)
		}
		ref, _ := ac["ref"].(string)
		if _, err := hex.DecodeString(ref); err != nil || len(ref) != 32 {
			t.Fatalf("ref %q", ref)
		}
		return ref, got.B
	}
	refA, refB := refs()
	if refA == refB {
		t.Fatalf("two middlewares told one ref %s", refA)
	}
	// another request, and magpie started again: the same refs
	if a2, b2 := refs(); a2 != refA || b2 != refB {
		t.Fatalf("refs changed between requests: %s %s, then %s %s", refA, refB, a2, b2)
	}
	forgetRefKeys()
	if a2, b2 := refs(); a2 != refA || b2 != refB {
		t.Fatalf("refs changed after a restart: %s %s, then %s %s", refA, refB, a2, b2)
	}
	// the same account written in another case is the same account;
	// another user, or the plan changed, is another
	if got := accountRef(a.Spec, gwmw.Account{Agent: "codex", User: "ME@Example.com", Plan: "Plus"}); got != refA {
		t.Fatalf("case changed the ref: %s, %s", got, refA)
	}
	for _, o := range []gwmw.Account{
		{Agent: "codex", User: "spare@example.com", Plan: "plus"},
		{Agent: "codex", User: "me@example.com", Plan: "pro"},
		{Agent: "claude", User: "me@example.com", Plan: "plus"},
	} {
		if accountRef(a.Spec, o) == refA {
			t.Fatalf("%+v has me's ref", o)
		}
	}

	r := Begin(info())
	defer r.End()
	for _, c := range []struct {
		id   string
		skip bool
	}{{"skip", true}, {"no", false}, {"undef", false}, {"throw", false}, {"null", false}, {"ok", false}} {
		acct := me
		acct.ID = c.id
		skip, by, got := triedAfter(t, r, acct)
		if skip != c.skip {
			t.Fatalf("%s: skip %v", c.id, skip)
		}
		if skip && by != a.Spec {
			t.Fatalf("%s: passed over by %q, not %q", c.id, by, a.Spec)
		}
		if !skip && got.B != refB {
			t.Fatalf("%s: the next middleware wasn't told the account: %+v", c.id, got)
		}
	}
	if st := States()[a.Spec]; st.Failures != 2 || !strings.Contains(st.Last, "null") || !strings.Contains(strings.Join(st.Hooks, ","), "onAccount") {
		t.Fatalf("state %+v", st)
	}
	// a try on a key asks no onAccount, and the account before it is gone
	// from ctx
	if skip, _, got := triedAfter(t, r, gwmw.Account{ID: "skip", Model: "m1"}); skip || got.A != "none" || got.B != "none" {
		t.Fatalf("a key's try: skip %v, told %+v", skip, got)
	}
	var none *Run
	if skip, _ := none.Account(me); skip {
		t.Fatal("a nil Run skipped")
	}
}

// The key refs are made with is kept: made once, read after, and one that
// can't be read is never written over, as every ref a middleware kept
// would change with it.
func TestAccountRefKey(t *testing.T) {
	forgetRefKeys()
	t.Cleanup(forgetRefKeys)
	p := filepath.Join(appdir.Config(), "account-ref.key")
	os.Remove(p)
	acct := gwmw.Account{Agent: "codex", User: "me@example.com"}
	first := accountRef("m", acct)
	b, err := os.ReadFile(p)
	if err != nil || len(strings.TrimSpace(string(b))) != 64 {
		t.Fatalf("key file %q %v", b, err)
	}
	if fi, _ := os.Stat(p); fi != nil && fi.Mode().Perm()&0o077 != 0 && os.PathSeparator == '/' {
		t.Fatalf("key file mode %v", fi.Mode())
	}
	forgetRefKeys()
	if accountRef("m", acct) != first {
		t.Fatal("the kept key wasn't read back")
	}
	// a key of its own makes other refs: the file is what keeps them
	forgetRefKeys()
	os.Remove(p)
	if accountRef("m", acct) == first {
		t.Fatal("a new key made the same ref")
	}
	forgetRefKeys()
	os.WriteFile(p, []byte("not a key\n"), 0o600)
	accountRef("m", acct)
	if b, _ := os.ReadFile(p); string(b) != "not a key\n" {
		t.Fatalf("a key that couldn't be read was written over: %q", b)
	}
	if left, _ := filepath.Glob(filepath.Join(appdir.Config(), ".account-ref-*")); len(left) > 0 {
		t.Fatalf("temp files left: %v", left)
	}
	os.Remove(p)
}
