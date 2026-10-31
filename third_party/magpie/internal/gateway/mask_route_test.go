package gateway

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// jessyoungyu on X: 「{{百度}}这家公司怎么样」 can't be answered by a model
// that gets the placeholder. With a model set for requests masking would
// change (settings.RedactRouteTo), one whose provider the user set to go
// unmasked on this machine or the network, such a request goes there as
// written and never to the vendor; one masking leaves alone goes where it
// was asked; unset, as by default, it is masked as before.

const routeWord = "百度"

// maskRouteSetup is the vendor ("fake", Messages, masked) and a local
// model ("near/qwen", Chat, set to go unmasked); routeTo is the setting.
func maskRouteSetup(t *testing.T, routeTo string, near func(*provider.Provider)) (vendor, local *fake) {
	t.Helper()
	vendor = &fake{ctype: "application/json", reply: `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"vendor"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`}
	setup(t, provider.Anthropic, vendor)
	// a Chat provider is asked to stream whether the agent streams or not
	local = &fake{reply: sse(`data: {"id":"c1","object":"chat.completion.chunk","model":"qwen","choices":[{"index":0,"delta":{"role":"assistant","content":"local"}}]}`,
		`data: {"id":"c1","object":"chat.completion.chunk","model":"qwen","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`,
		`data: [DONE]`)}
	up := httptest.NewServer(local)
	t.Cleanup(up.Close)
	p := provider.Provider{ID: "near", Name: "Near", Key: "k", Models: []string{"qwen"}, Chat: up.URL + "/v1", Unredacted: true}
	if near != nil {
		near(&p)
	}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true, RedactWords: []string{routeWord}, RedactRouteTo: routeTo}); err != nil {
		t.Fatal(err)
	}
	return vendor, local
}

func messagesBody(text string, stream bool) string {
	s := ""
	if stream {
		s = `"stream":true,`
	}
	return `{"model":"fake/m1",` + s + `"max_tokens":10,"messages":[{"role":"user","content":"` + text + `"}]}`
}

func TestMaskedRequestGoesToTheModelSet(t *testing.T) {
	ask := routeWord + "这家公司怎么样"
	t.Run("unset, as by default: masked to the vendor", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "", nil)
		if code, reply := post(t, "/v1/messages", messagesBody(ask, false)); code != 200 || !strings.Contains(reply, "vendor") {
			t.Fatalf("%d %s", code, reply)
		}
		if strings.Contains(string(vendor.got), routeWord) || !strings.Contains(string(vendor.got), "{{TERM_") || local.calls != 0 {
			t.Fatalf("vendor got %s, local asked %d times", vendor.got, local.calls)
		}
	})
	t.Run("set: as written to the local model, nothing to the vendor", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", nil)
		code, reply := post(t, "/v1/messages", messagesBody(ask, false))
		if code != 200 || !strings.Contains(reply, "local") {
			t.Fatalf("%d %s", code, reply)
		}
		if vendor.calls != 0 || !strings.Contains(string(local.got), ask) || strings.Contains(string(local.got), "{{") || !strings.Contains(string(local.got), `"qwen"`) {
			t.Fatalf("vendor asked %d times; local got %s", vendor.calls, local.got)
		}
	})
	t.Run("set, streamed", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", nil)
		code, reply := post(t, "/v1/messages", messagesBody(ask, true))
		if code != 200 || !strings.Contains(reply, "local") || !strings.Contains(reply, "message_stop") {
			t.Fatalf("%d %s", code, reply)
		}
		if vendor.calls != 0 || !strings.Contains(string(local.got), ask) {
			t.Fatalf("vendor asked %d times; local got %s", vendor.calls, local.got)
		}
	})
	t.Run("set, nothing to mask: to the model asked for", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", nil)
		if code, reply := post(t, "/v1/messages", messagesBody("hello there", false)); code != 200 || !strings.Contains(reply, "vendor") {
			t.Fatalf("%d %s", code, reply)
		}
		if local.calls != 0 || vendor.calls != 1 {
			t.Fatalf("local asked %d, vendor %d", local.calls, vendor.calls)
		}
	})
	t.Run("set, the local model fails: the request fails, nothing to the vendor", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", nil)
		local.code, local.ctype, local.reply = 500, "application/json", `{"error":{"message":"down"}}`
		if code, reply := post(t, "/v1/messages", messagesBody(ask, false)); code < 400 {
			t.Fatalf("%d %s", code, reply)
		}
		if vendor.calls != 0 || local.calls == 0 {
			t.Fatalf("vendor asked %d, local %d", vendor.calls, local.calls)
		}
	})
	t.Run("set, but it falls back to a vendor: masked as before", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", func(p *provider.Provider) { p.Fallback = []string{"fake/m1"} })
		if code, reply := post(t, "/v1/messages", messagesBody(ask, false)); code != 200 || !strings.Contains(reply, "vendor") {
			t.Fatalf("%d %s", code, reply)
		}
		if local.calls != 0 || strings.Contains(string(vendor.got), routeWord) {
			t.Fatalf("local asked %d; vendor got %s", local.calls, vendor.got)
		}
	})
	t.Run("set, but no longer set to go unmasked: masked as before", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", func(p *provider.Provider) { p.Unredacted = false })
		if code, reply := post(t, "/v1/messages", messagesBody(ask, false)); code != 200 || !strings.Contains(reply, "vendor") {
			t.Fatalf("%d %s", code, reply)
		}
		if local.calls != 0 || strings.Contains(string(vendor.got), routeWord) {
			t.Fatalf("local asked %d; vendor got %s", local.calls, vendor.got)
		}
	})
	t.Run("set: counted on the local model, as written", func(t *testing.T) {
		vendor, local := maskRouteSetup(t, "near/qwen", nil)
		code, reply := post(t, "/v1/messages/count_tokens", `{"model":"fake/m1","messages":[{"role":"user","content":"`+ask+`"}]}`)
		if code != 200 {
			t.Fatalf("%d %s", code, reply)
		}
		if vendor.calls != 0 {
			t.Fatalf("counted on the vendor: %s", vendor.got)
		}
		if !strings.Contains(string(local.got), ask) && local.calls != 0 {
			t.Fatalf("local counted %s", local.got)
		}
	})
}

// A turn of Codex's on one of its own models, relayed on its ChatGPT
// sign-in, goes to the model set too, and the ChatGPT backend gets none.
func TestCodexSigninTurnGoesToTheModelSet(t *testing.T) {
	for _, routeTo := range []string{"", "near/qwen"} {
		t.Run("route "+routeTo, func(t *testing.T) {
			_, local := maskRouteSetup(t, routeTo, nil)
			chatgpt := &fake{reply: sse(`data: {"type":"response.created","response":{"id":"r1","model":"gpt-5.5"}}`,
				`data: {"type":"response.output_text.delta","delta":"chatgpt"}`,
				`data: {"type":"response.completed","response":{"id":"r1","usage":{"input_tokens":7,"output_tokens":1}}}`)}
			up := httptest.NewServer(chatgpt)
			t.Cleanup(up.Close)
			was := provider.CodexBase
			provider.CodexBase = up.URL + "/backend-api/codex"
			t.Cleanup(func() { provider.CodexBase = was })
			code, reply := codexPost(t, `{"model":"gpt-5.5","stream":true,"store":false,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"`+routeWord+`这家公司怎么样"}]}]}`)
			if code != http.StatusOK {
				t.Fatalf("%d %s", code, reply)
			}
			if routeTo == "" {
				if local.calls != 0 || chatgpt.calls != 1 || strings.Contains(string(chatgpt.got), routeWord) {
					t.Fatalf("local %d; ChatGPT got %s", local.calls, chatgpt.got)
				}
				return
			}
			if chatgpt.calls != 0 || !strings.Contains(string(local.got), routeWord) || !strings.Contains(reply, "local") {
				t.Fatalf("ChatGPT asked %d; local got %s; answered %s", chatgpt.calls, local.got, reply)
			}
		})
	}
}

// What the GUI lists and the setting takes: a model whose every route goes
// unmasked, not a vendor's, nor one that falls back to one.
func TestMaskRouteTakesOnlyUnmaskedRoutes(t *testing.T) {
	maskRouteSetup(t, "", nil)
	if !MaskRoute("near/qwen") {
		t.Fatal("near/qwen goes unmasked")
	}
	for _, id := range []string{"", "fake/m1", "nowhere/x"} {
		if MaskRoute(id) {
			t.Fatalf("%q taken", id)
		}
	}
	maskRouteSetup(t, "", func(p *provider.Provider) { p.Fallback = []string{"fake/m1"} })
	if MaskRoute("near/qwen") {
		t.Fatal("one falling back to a vendor taken")
	}
}

// Only an agent's own turn goes to the model set: magpie's own asks made
// on its behalf (a title, a search stand-in, a picture described) are
// masked as before, and so is a turn on a gateway key the model set is
// not one of the key's models.
func TestMaskRouteOnlyForTheAgentsTurn(t *testing.T) {
	maskRouteSetup(t, "near/qwen", nil)
	hit := maskHits{n: 1}
	r := httptest.NewRequest("POST", "/v1/messages", nil)
	if got := maskRouteFor(r, hit); got != "" {
		t.Fatalf("not an agent's turn: %q", got)
	}
	turn := agentTurn(r)
	if got := maskRouteFor(turn, hit); got != "near/qwen" {
		t.Fatalf("the agent's turn: %q", got)
	}
	if got := maskRouteFor(turn, maskHits{}); got != "" {
		t.Fatalf("nothing masked: %q", got)
	}
	if got := maskRouteFor(turn.WithContext(ownAsk(turn.Context())), hit); got != "" {
		t.Fatalf("magpie's own ask: %q", got)
	}
	keyed := func(models ...string) *http.Request {
		return turn.WithContext(access.WithIdentity(turn.Context(), access.Identity{KeyID: "k1", Models: models}))
	}
	if got := maskRouteFor(keyed("fake/*"), hit); got != "" {
		t.Fatalf("a key without it: %q", got)
	}
	if got := maskRouteFor(keyed("fake/*", "near/*"), hit); got != "near/qwen" {
		t.Fatalf("a key with it: %q", got)
	}
}
