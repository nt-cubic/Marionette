package gateway

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// cdredfox on X: in a Smart group of WorkBuddy accounts, the one whose
// credits left run out first goes first, since what it has left is lost
// then; the Credits window has no reset, so before they all ranked alike
// and kept their order. Its share used still decides first: one all but
// spent waits behind, whatever its credits' end.
func TestSmartSpendsWorkBuddyCreditsRunningOutFirst(t *testing.T) {
	old := allowances
	defer func() { allowances = old }()
	now := time.Now()
	share := map[string]provider.Allowance{}
	allowances = func(string) map[string]provider.Allowance { return share }
	p := provider.Provider{ID: "workbuddy-ai", Account: &provider.Account{Agent: "workbuddy-ai", User: "a@x.com"}}
	acct := func(user string) candidate {
		return candidate{p: provider.Provider{ID: "workbuddy-ai", Account: &provider.Account{Agent: "workbuddy-ai", User: user}}, model: "hy3", rest: "workbuddy-ai#" + user}
	}
	credits := func(used float64, in time.Duration) provider.Allowance {
		l := provider.Limit{Used: used, Amount: used * 22.5, Of: 2250, Unit: "credits"}
		if in > 0 {
			l.RunsOut = now.Add(in)
		}
		return provider.Allowance{l}
	}
	order := func(cs ...candidate) string {
		got, _ := weigh(p, cs, "hy3", provider.Chat)
		var s []string
		for _, c := range got {
			s = append(s, c.p.Account.User[:1])
		}
		return strings.Join(s, "")
	}

	share["a@x.com"] = credits(10, 18*24*time.Hour)
	share["b@x.com"] = credits(30, 30*time.Hour)
	if got := order(acct("a@x.com"), acct("b@x.com")); got != "ba" {
		t.Fatalf("credits ending sooner first: %s", got)
	}
	// one not known (an older plugin) after those known
	share["a@x.com"] = credits(10, 0)
	if got := order(acct("a@x.com"), acct("b@x.com")); got != "ba" {
		t.Fatalf("not known last: %s", got)
	}
	// ending the same hour: their order, for the cache
	hour := now.Truncate(time.Hour).Add(31 * time.Hour).Sub(now)
	share["a@x.com"] = credits(10, hour+40*time.Minute)
	share["b@x.com"] = credits(30, hour+10*time.Minute)
	if got := order(acct("a@x.com"), acct("b@x.com")); got != "ab" {
		t.Fatalf("same hour keeps order: %s", got)
	}
	// all but spent: behind, though its credits end first
	share["a@x.com"] = credits(10, 18*24*time.Hour)
	share["b@x.com"] = credits(99, 30*time.Hour)
	if got := order(acct("b@x.com"), acct("a@x.com")); got != "ab" {
		t.Fatalf("spent last: %s", got)
	}
}
