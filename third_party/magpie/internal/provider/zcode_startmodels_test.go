package provider

import (
	"context"
	"encoding/json"
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
)

func zcodeIDs(ms []catalog.Model) []string {
	var out []string
	for _, m := range ms {
		out = append(out, m.ID)
	}
	return out
}

// 悠悠哥's move to the plugin was held back by GLM-5.3 on a Start Plan
// (体验套餐) account: a Z.ai account with a key made and no Coding Plan
// bought, its requests going to the Start Plan, was shown the Coding
// Plan's models until a request found where it goes, and the list fetched
// last (the Coding Plan's) after that, so GLM-5.3 was listed and could be
// ticked, though neither ZCode nor the built-in serves it there. Now such an
// account neither lists nor exposes it, picked or not; a Coding Plan one
// still does. And the move doesn't count it as one the plugin drops.
func TestZCodeStartPlanHasNoGLM53(t *testing.T) {
	signIn(t)
	old := zcodeBuiltinFiles
	zcodeBuiltinFiles = func() []string { return nil } // not this machine's ZCode
	t.Cleanup(func() { zcodeBuiltinFiles = old })
	zcodeRoutes.Lock()
	zcodeRoutes.m = map[string]zcodeRoute{}
	zcodeRoutes.Unlock()
	jwt := zcodeTestJWT(time.Now().Add(24 * time.Hour))
	start := zcodeKey{Key: "made.key", Base: ZCodeZaiBase, JWT: jwt}
	saveLogins(t, savedLogin{Agent: "zcode", User: "trial@x", Plan: "Start Plan", First: true, On: true, Auth: json.RawMessage(jsonText(start))})

	has53 := func(ms []catalog.Model) bool { return slices.Contains(zcodeIDs(ms), "GLM-5.3") }
	p, ok := zcodeAccount()
	if !ok {
		t.Fatal("no ZCode account")
	}
	// before any request: its plan's name says the Start Plan
	if ms := p.Available(); has53(ms) || len(ms) == 0 {
		t.Fatalf("a Start Plan account before its route is found lists %v", zcodeIDs(ms))
	}
	p.Models = []string{"GLM-5.3", "GLM-5.2"}
	if ms := p.Exposed(); has53(ms) || !slices.Contains(zcodeIDs(ms), "GLM-5.2") {
		t.Fatalf("a Start Plan account with GLM-5.3 ticked exposes %v", zcodeIDs(ms))
	}
	// with the Coding Plan's list fetched last
	if err := catalog.SaveLive("zcode", ZCodeZaiBase, zcodeModels); err != nil {
		t.Fatal(err)
	}
	p.Models = nil
	if ms := p.Available(); has53(ms) || len(ms) == 0 {
		t.Fatalf("a Start Plan account under the Coding Plan's list lists %v", zcodeIDs(ms))
	}
	p.Models = []string{"GLM-5.3", "GLM-5.2"}
	if ms := p.Exposed(); has53(ms) || !slices.Contains(zcodeIDs(ms), "GLM-5.2") {
		t.Fatalf("a Start Plan account under the Coding Plan's list exposes %v", zcodeIDs(ms))
	}

	// the move: GLM-5.3 the built-in didn't serve on it, GLM-5.2 it did
	a := Moving{User: "trial@x", Plan: "Start Plan"}
	if b := movers["zcode"].builtin; b(context.Background(), a, "GLM-5.3") || !b(context.Background(), a, "GLM-5.2") {
		t.Fatal("the move counts GLM-5.3 as served on a Start Plan account")
	}

	// a Coding Plan account still has it
	coding := zcodeKey{Key: "plan.key", Base: ZCodeZaiBase, JWT: jwt}
	saveLogins(t, savedLogin{Agent: "zcode", User: "pro@x", Plan: "GLM Coding Pro", First: true, On: true, Auth: json.RawMessage(jsonText(coding))})
	p, _ = zcodeAccount()
	p.Models = []string{"GLM-5.3"}
	if !has53(p.Available()) || !has53(p.Exposed()) {
		t.Fatalf("a Coding Plan account lists %v", zcodeIDs(p.Available()))
	}
	if !movers["zcode"].builtin(context.Background(), Moving{User: "pro@x", Plan: "GLM Coding Pro"}, "GLM-5.3") {
		t.Fatal("the move counts GLM-5.3 as not served on a Coding Plan account")
	}
}
