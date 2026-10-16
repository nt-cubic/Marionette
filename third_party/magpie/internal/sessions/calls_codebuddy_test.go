package sessions

import (
	"testing"
	"time"
)

// CodeBuddy Code's direct calls reach the usage ledger (#1779). The two
// sessions in testdata/codebuddy are CodeBuddy Code 2.164.0's own files, from
// two `codebuddy -p` runs against a local OpenAI fake: one on a model the user
// added for another vendor (fake-m1), whose two replies (a Read tool call,
// then text) are calls of the agent's own; one on a model magpie wired
// (deepseek/pro, vendor "magpie" in models.json), whose replies the gateway
// logs itself and are not read here as well.
func TestCodeBuddyCalls(t *testing.T) {
	setupAgents(t)
	var got []Call
	for _, c := range Calls(time.Time{}) {
		if c.Agent == "codebuddy" {
			got = append(got, c)
		}
	}
	if len(got) != 2 {
		t.Fatalf("want CodeBuddy Code's 2 direct calls, got %d: %+v", len(got), got)
	}
	// each reply: 27017 in, of them 26496 read from the cache; 115 out
	want := Tokens{Input: 27017 - 26496, Output: 115, CacheRead: 26496}
	// newest first: the text reply, then the tool call before it
	for i, w := range []struct {
		msg string
		at  int64
	}{{"01a12fea4ecc74979fe1b9832df42ddb", 1791805247199}, {"01a12fea4e6378c680e219148e8547be", 1791805247141}} {
		c := got[i]
		if c.Msg != w.msg || !c.Time.Equal(ms(w.at)) || c.Model != "fake-m1" || c.Tokens != want || c.Session != "01a12fea-4cae-78ac-8b2b-a1c0f9da9b1c" {
			t.Fatalf("call %d: %+v", i, c)
		}
	}
}

// WorkBuddy's calls are still WorkBuddy's: the two agents share a reader,
// and each is read against its own models.json. deepseek/pro is magpie's in
// CodeBuddy's models.json and not in WorkBuddy's, so the same id there
// would be the user's own.
func TestCodeBuddyGatewayIsItsOwn(t *testing.T) {
	setupAgents(t)
	if !buddyGateway(CodeBuddyDir(), "custom-local:deepseek/pro") {
		t.Fatal("magpie's model in CodeBuddy's models.json is not read as the gateway's")
	}
	if buddyGateway(CodeBuddyDir(), "custom-local:fake-m1") {
		t.Fatal("the user's own model in CodeBuddy's models.json is read as the gateway's")
	}
	if workbuddyGateway("custom-local:deepseek/pro") {
		t.Fatal("CodeBuddy's models.json was read for WorkBuddy")
	}
}
