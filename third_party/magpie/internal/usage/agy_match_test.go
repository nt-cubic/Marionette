package usage

import (
	"testing"
	"time"

	"github.com/yetone/magpie/internal/sessions"
)

// #2097: an agy step that went through magpie is counted once. agy 1.2.16,
// run in a sandbox through a fake Gemini endpoint, sent no session header
// (only User-Agent google-genai-sdk/…, X-Goog-Api-Key), so the gateway's
// record names no session; its transcript's step began at 06:12:41Z, to
// the second, and counted input 702, cache read 300, output 9. A long step
// ends well past the 2 s every other session file's end time is held to.
func TestAgyStepThroughMagpieCountedOnce(t *testing.T) {
	began := time.Date(2026, 10, 14, 6, 12, 41, 0, time.UTC)
	rec := Record{Time: began.Add(990 * time.Millisecond), Agent: "agy", Provider: "relay", Model: "m",
		Input: 702, CacheRead: 300, Output: 9, Millis: 95_000, Status: 200}
	step := sessions.Call{Time: began, Agent: "agy", Session: "8ac63d53-9162-4a29-af29-bda3428931ae",
		Tokens: sessions.Tokens{Input: 702, CacheRead: 300, Output: 9}}
	direct := sessions.Call{Time: began.Add(10 * time.Minute), Agent: "agy", Session: step.Session,
		Tokens: sessions.Tokens{Input: 3013, CacheRead: 14973, Output: 1518}}

	m := gatewayMatches([]Record{rec}, []sessions.Call{step, direct})
	if !m[0] {
		t.Error("the step the gateway logged is counted again as agy's own call")
	}
	if m[1] {
		t.Error("a step agy made without magpie was taken for the gateway's")
	}

	// a step the gateway's call ended before is not that call
	early := step
	early.Time = rec.Time.Add(time.Duration(rec.Millis)*time.Millisecond + 3*time.Second)
	if gatewayMatches([]Record{rec}, []sessions.Call{early})[0] {
		t.Error("a step begun after the gateway's call ended was matched to it")
	}
	// another agent's session-less record is no agy step's
	other := rec
	other.Agent = "codex"
	if gatewayMatches([]Record{other}, []sessions.Call{step})[0] {
		t.Error("an agy step matched another agent's record")
	}
}
