package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// Tool results identify a waiting process, but the router may have picked
// another account since that process made its call. The selected account
// must answer, including when the same account moved out of the agent's home.
func TestClaudeToolResultsKeepTheSelectedAccount(t *testing.T) {
	for _, ids := range []string{"exact", "remapped"} {
		for _, mode := range []string{"same owner", "another account", "another home"} {
			t.Run(ids+"/"+mode, func(t *testing.T) {
				claudeMadeFirst(t, false)
				h := newRemapHarness(t)
				received := make(chan bool, 1)
				h.first = func(run *subscriptionRun, req *Request) {
					run.owner = "claude\x00a@example.com\x00" + ownHome
					waiters := h.calls(run, true, shownCall{"call-a", "read", `{}`})
					go func() {
						_, ok := <-waiters[0]
						if ok {
							h.says(run, "kept run")
						}
						received <- ok
					}()
				}
				h.ask(remapUser("read"))
				p := provider.Provider{ID: "claude", Account: &provider.Account{Agent: "claude", User: "a@example.com"}}
				want := "kept run"
				if mode == "another account" {
					for _, saved := range p.AlsoOn() {
						if saved.Account.User == "b@example.com" {
							p = saved
							break
						}
					}
					if p.Account.User != "b@example.com" {
						t.Fatal("saved B is missing")
					}
					want = "tok-b turn 1"
				} else if mode == "another home" {
					if err := provider.SwitchLogin("claude", "b@example.com"); err != nil {
						t.Fatal(err)
					}
					b := provider.Provider{ID: "claude", Account: &provider.Account{Agent: "claude", User: "b@example.com"}}
					for _, saved := range b.AlsoOn() {
						if saved.Account.User == "a@example.com" {
							p = saved
							break
						}
					}
					if p.Account.AgentsOwn() {
						t.Fatal("A did not move to its saved home")
					}
					want = "tok-a turn 1"
				}
				callID := "call-a"
				if ids == "remapped" {
					callID = "client-a"
				}
				body := `{"model":"claude-sonnet-5","max_tokens":100,"tools":[{"name":"read","input_schema":{"type":"object"}}],"messages":[` + remapUser("read") + `,` + remapCalls(callID+"={}") + `,` + remapResults(callID+"=result") + `]}`
				rec := httptest.NewRecorder()
				code, msg := h.s.serveClaudeSubscription(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)), provider.Anthropic, p, "claude-sonnet-5", []byte(body), &Usage{})
				if code != 200 {
					t.Fatalf("%d %s", code, msg)
				}
				var reply struct{ Content []struct{ Text string } }
				if err := json.Unmarshal(rec.Body.Bytes(), &reply); err != nil || len(reply.Content) != 1 || reply.Content[0].Text != want {
					t.Fatalf("selected account replied %s; want %q (decode: %v)", rec.Body, want, err)
				}
				select {
				case got := <-received:
					if got != (mode == "same owner") {
						t.Fatalf("old process received the result: %v, mode %s", got, mode)
					}
				case <-time.After(2 * time.Second):
					t.Fatal("old process was neither resumed nor released")
				}
			})
		}
	}
}
