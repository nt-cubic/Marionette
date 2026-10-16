package gateway

import (
	"bufio"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A reply from a group says which member answered it (#822): its
// X-Magpie-Provider and X-Magpie-Model headers always, relayed or
// translated, streamed or not, after a failover too; its body's model the
// vendor's own name, unless MemberModel (or the request's
// X-Magpie-Response-Model: member) asks for magpie's provider/model id
// there — never for Claude Code or Codex, which read it themselves.
func TestReplyNamesMember(t *testing.T) {
	bodies := map[string]string{
		"/v1/messages":         `{"model":"group/auto","max_tokens":100,"messages":[{"role":"user","content":"hi"}]`,
		"/v1/chat/completions": `{"model":"group/auto","messages":[{"role":"user","content":"hi"}]`,
		"/v1/responses":        `{"model":"group/auto","input":"hi"`,
	}
	paths := []struct {
		name  string
		proto provider.Protocol // the vendor's
		agent string            // the agent's endpoint
	}{
		{"anthropic", provider.Anthropic, "/v1/messages"},
		{"chat", provider.Chat, "/v1/chat/completions"},
		{"responses", provider.Responses, "/v1/responses"},
		{"chat from anthropic", provider.Anthropic, "/v1/chat/completions"},
		{"anthropic from chat", provider.Chat, "/v1/messages"},
		{"responses from chat", provider.Chat, "/v1/responses"},
	}
	const served = "glm-5.3-flash-202606"
	for _, pa := range paths {
		for _, stream := range []bool{false, true} {
			for _, c := range []struct {
				name   string
				on     bool   // MemberModel
				header string // X-Magpie-Response-Model
				token  string // the agent's key
				member bool   // the body names the member
			}{
				{"default", false, "", "", false},
				{"on", true, "", "", true},
				{"asked", false, "member", "", true},
				{"on, vendor asked", true, "vendor", "", false},
				{"on, codex", true, "", TokenFor("codex"), false},
				{"on, claude code", true, "", TokenFor("claude"), false},
			} {
				name := pa.name + "/" + c.name
				if stream {
					name += "/stream"
				}
				t.Run(name, func(t *testing.T) {
					fresh(t)
					// a fails, so b answers after a failover
					down := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
						http.Error(w, `{"error":{"message":"overloaded"}}`, 529)
					}))
					t.Cleanup(down.Close)
					up := httptest.NewServer(servedBy(pa.proto, served))
					t.Cleanup(up.Close)
					for _, p := range []provider.Provider{
						{ID: "a", Name: "A", Key: "k", Models: []string{"glm-5.3-flash"}},
						{ID: "b", Name: "B", Key: "k", Models: []string{"glm-5.3-flash"}},
					} {
						url := down.URL
						if p.ID == "b" {
							url = up.URL
						}
						switch pa.proto {
						case provider.Chat:
							p.Chat = url + "/v1"
						case provider.Responses:
							p.Responses = url + "/v1"
						case provider.Anthropic:
							p.Anthropic = url
						}
						if err := provider.Save(p); err != nil {
							t.Fatal(err)
						}
					}
					if err := provider.SaveGroup(provider.Group{Name: "auto", Routing: provider.Ordered, Members: []string{"a/glm-5.3-flash", "b/glm-5.3-flash"}}); err != nil {
						t.Fatal(err)
					}
					if c.on {
						if err := settings.Save(settings.Settings{MemberModel: true}); err != nil {
							t.Fatal(err)
						}
					}
					body := bodies[pa.agent]
					if stream {
						body += `,"stream":true`
					}
					req := httptest.NewRequest("POST", pa.agent, strings.NewReader(body+"}"))
					if c.header != "" {
						req.Header.Set(responseModelHeader, c.header)
					}
					if c.token != "" {
						req.Header.Set("Authorization", "Bearer "+c.token)
					}
					rec := httptest.NewRecorder()
					s := New()
					s.Handler().ServeHTTP(rec, req)
					if rec.Code != 200 || !strings.Contains(rec.Body.String(), "hi") {
						t.Fatalf("%d %s", rec.Code, rec.Body)
					}
					if r := lastRoute(s); len(r.Tries) != 2 {
						t.Fatalf("tries %+v", r.Tries)
					}
					if p, m := rec.Header().Get(providerHeader), rec.Header().Get(modelHeader); p != "b" || m != "b/glm-5.3-flash" {
						t.Errorf("headers say %q %q", p, m)
					}
					want := served
					if c.member {
						want = "b/glm-5.3-flash"
					}
					names := modelsNamed(rec.Body.String(), stream)
					if len(names) == 0 {
						t.Fatalf("no model named in %s", rec.Body)
					}
					for _, n := range names {
						if n != want {
							t.Fatalf("the reply names %q, want %q:\n%s", n, want, rec.Body)
						}
					}
				})
			}
		}
	}
}

// modelsNamed are the models a reply's body, or each of its events, names.
func modelsNamed(body string, stream bool) []string {
	var datas []string
	if !stream {
		datas = []string{body}
	} else {
		sc := bufio.NewScanner(strings.NewReader(body))
		sc.Buffer(nil, 1<<20)
		for sc.Scan() {
			if d, ok := strings.CutPrefix(sc.Text(), "data:"); ok {
				datas = append(datas, strings.TrimSpace(d))
			}
		}
	}
	var out []string
	for _, d := range datas {
		for _, path := range memberPaths {
			if v := gjson.Get(d, path); v.Type == gjson.String {
				out = append(out, v.Str)
			}
		}
	}
	return out
}

// An event's model is put in place, the rest of the line as it was; a
// line that names none, or isn't JSON, is untouched.
func TestMemberLine(t *testing.T) {
	for in, want := range map[string]string{
		"data: {\"model\":\"glm\",\"x\":1}\n":                   "data: {\"model\":\"b/glm\",\"x\":1}\n",
		"data:{\"message\":{\"model\":\"glm\"}}\r\n":            "data:{\"message\":{\"model\":\"b/glm\"}}\r\n",
		"data: {\"response\":{\"id\":\"r\",\"model\":\"glm\"}}": "data: {\"response\":{\"id\":\"r\",\"model\":\"b/glm\"}}",
		"data: {\"modelVersion\":\"glm\"}\n":                    "data: {\"modelVersion\":\"b/glm\"}\n",
		"data: [DONE]\n":                                        "data: [DONE]\n",
		"event: message_start\n":                                "event: message_start\n",
		"data: {\"delta\":{\"text\":\"\\\"model\\\":\"}}\n":     "data: {\"delta\":{\"text\":\"\\\"model\\\":\"}}\n",
	} {
		if got := string(memberLine([]byte(in), "b/glm")); got != want {
			t.Errorf("%q: got %q, want %q", in, got, want)
		}
	}
}
