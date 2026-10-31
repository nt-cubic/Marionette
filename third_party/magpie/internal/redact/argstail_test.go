package redact

import (
	"encoding/json"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"
)

// What the local model really found in a Codex tool call's arguments
// (the privacy filter run on them, 10-29): a password ending in ! or % is
// one token with the quote after it, so its span ran over the closing
// quote, and an address's over its full stop and the quote.
var argsTailModel = &fakeModel{values: map[string]string{
	`Hunt3r!"`:                    "secret",
	`Abc!23456%"`:                 "secret",
	`221B Baker Street, London."`: "private_address",
	`pwd=Qq123456@#"`:             "secret",
}}

// A value the masking finds at the very end of a tool call's arguments
// ends at its string's closing quote: the "} after it go as written, the
// arguments are still JSON, and the value comes back in them as the
// agent wrote it. Masked with the quote, {"cmd":"export
// DB_PASSWORD=Hunt3r!"} went as {"cmd":"export DB_PASSWORD={{SECRET_…}}},
// the vendor turned away every turn that replayed the call, and Codex
// reconnected over and over (xiaojiang091499 on X). The same for each
// protocol whose arguments are JSON text, and for a user's rule that
// reads to the next space.
func TestArgumentsValueAtEndKeepsQuote(t *testing.T) {
	o := Options{Secrets: true, Personal: true, Model: argsTailModel.find,
		Rules: []Rule{{Kind: "TOK", Regex: `tok-\S+`}}}
	cases := []struct{ args, gone string }{
		{`{"cmd":"export DB_PASSWORD=Hunt3r!"}`, "Hunt3r"},
		{`{"cmd":["bash","-lc","echo 密码是 Abc!23456%"],"workdir":"/tmp"}`, "Abc!23456"},
		{`{"command":"pwd=Qq123456@#"}`, "Qq123456"},
		{`{"text":"ship to 221B Baker Street, London."}`, "Baker Street"},
		{`{"cmd":"curl -H 'X: tok-abcdef123'"}`, "tok-abcdef"},
		{`{"cmd":"curl -H tok-abcdef123"}`, "tok-abcdef"},
		{`{"a":["tok-abcdef123","x"],"b":1}`, "tok-abcdef"},
		{`{"cmd": "echo \"tok-abcdef123\""}`, "tok-abcdef"},
		{`{"cmd":"echo 13800138000"}`, "13800138000"},
	}
	for _, c := range cases {
		a, _ := json.Marshal(c.args)
		for _, body := range []string{
			// Responses, as Codex replays the call
			`{"model":"gpt-5.5","input":[{"type":"function_call","call_id":"call_1","name":"exec_command","arguments":` + string(a) + `}]}`,
			// Chat Completions
			string(chatWithCall(c.args)),
		} {
			out, n := MaskJSON([]byte(body), o)
			got := argumentsIn(t, out)
			if n == 0 || !json.Valid([]byte(got)) || strings.Contains(got, c.gone) {
				t.Errorf("%s\n masked %d: %s", c.args, n, got)
				continue
			}
			if back := Restore(got, true); back != c.args {
				t.Errorf("%s\n came back %s", c.args, back)
			}
		}
	}
}

// A value that is a whole bare value of the arguments, a number a user's
// rule finds, is masked as a string: written bare, its placeholder left
// them JSON no longer.
func TestArgumentsBareValueQuoted(t *testing.T) {
	o := Options{Secrets: true, Rules: []Rule{{Kind: "ACCT", Regex: `\b\d{11}\b`}}}
	out, n := MaskJSON(chatWithCall(`{"user":"app","acct": 12345678901,"ids":[12345678902]}`), o)
	got := argsOf(t, out)[0]
	if n != 2 || !regexp.MustCompile(`^\{"user":"app","acct": "\{\{ACCT_[a-z2-7]{8}\}\}","ids":\["\{\{ACCT_[a-z2-7]{8}\}\}"\]\}$`).MatchString(got) {
		t.Fatalf("%d: %s", n, got)
	}
	if back := Restore(got, true); back != `{"user":"app","acct": "12345678901","ids":["12345678902"]}` {
		t.Fatalf("came back %s", back)
	}
}

// argumentsIn are the arguments of the one tool call in a Responses or
// Chat Completions request.
func argumentsIn(t *testing.T, body []byte) string {
	t.Helper()
	var v struct {
		Input []struct {
			Arguments string `json:"arguments"`
		} `json:"input"`
	}
	if err := json.Unmarshal(body, &v); err != nil {
		t.Fatalf("body no longer JSON: %s", body)
	}
	if len(v.Input) > 0 {
		return v.Input[0].Arguments
	}
	return argsOf(t, body)[0]
}

// The model's arguments with the placeholder last, streamed in pieces
// that split it and its closing "}, come to the agent as the arguments
// with the value in them, escaped: in Responses' function_call_arguments
// deltas, Chat Completions' tool_calls and Anthropic's input_json_delta.
func TestWriterStreamArgumentsValueAtEnd(t *testing.T) {
	const value = `Hunt3r!"\x`
	// a value with a quote and a backslash in it, which go back escaped
	masked, _ := Mask("pw "+value, Options{Secrets: true, Rules: []Rule{{Kind: "PW", Regex: `Hunt3r\S+`}}})
	p := regexp.MustCompile(`\{\{PW_[a-z2-7]{8}\}\}`).FindString(masked)
	if p == "" {
		t.Fatalf("not masked: %s", masked)
	}
	args := `{"cmd":"export DB_PASSWORD=` + p + `"}`
	want, _ := json.Marshal(map[string]string{"cmd": "export DB_PASSWORD=" + value})
	protocols := []struct {
		name  string
		event func(piece string) string
		piece func(data []byte) string
	}{
		{"responses", func(s string) string {
			b, _ := json.Marshal(map[string]any{"type": "response.function_call_arguments.delta", "item_id": "fc_1", "output_index": 1, "delta": s})
			return "event: response.function_call_arguments.delta\ndata: " + string(b) + "\n\n"
		}, func(d []byte) string {
			var e struct{ Delta string }
			json.Unmarshal(d, &e)
			return e.Delta
		}},
		{"chat", func(s string) string {
			b, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{"index": 0, "delta": map[string]any{"tool_calls": []any{map[string]any{"index": 0, "function": map[string]any{"arguments": s}}}}}}})
			return "data: " + string(b) + "\n\n"
		}, func(d []byte) string {
			var e struct {
				Choices []struct {
					Delta struct {
						ToolCalls []struct{ Function struct{ Arguments string } } `json:"tool_calls"`
					}
				}
			}
			json.Unmarshal(d, &e)
			return e.Choices[0].Delta.ToolCalls[0].Function.Arguments
		}},
		{"anthropic", func(s string) string {
			b, _ := json.Marshal(map[string]any{"type": "content_block_delta", "index": 1, "delta": map[string]any{"type": "input_json_delta", "partial_json": s}})
			return "event: content_block_delta\ndata: " + string(b) + "\n\n"
		}, func(d []byte) string {
			var e struct {
				Delta struct {
					PartialJSON string `json:"partial_json"`
				}
			}
			json.Unmarshal(d, &e)
			return e.Delta.PartialJSON
		}},
	}
	at := strings.Index(args, p)
	for _, pr := range protocols {
		// every place to split it: inside the placeholder, between its
		// braces, before the "} and between the " and the }
		for cut := at + 1; cut < len(args); cut++ {
			rec := httptest.NewRecorder()
			rec.Header().Set("Content-Type", "text/event-stream")
			w := NewWriter(rec)
			w.WriteHeader(200)
			w.Write([]byte(pr.event(args[:cut])))
			w.Write([]byte(pr.event(args[cut:])))
			w.Finish()
			var js strings.Builder
			for _, e := range strings.Split(rec.Body.String(), "\n\n") {
				if _, d, ok := strings.Cut(e, "data: "); ok {
					js.WriteString(pr.piece([]byte(d)))
				}
			}
			if js.String() != string(want) {
				t.Errorf("%s, cut at %d: %s, want %s", pr.name, cut, js.String(), want)
			}
		}
	}
}

// A whole body's arguments delta whose "delta" comes before its "type" is
// put back escaped, as the stream's is.
func TestRestoreJSONDeltaBeforeType(t *testing.T) {
	masked, _ := Mask("pw Hunt3r!\"\\x", Options{Secrets: true, Rules: []Rule{{Kind: "PW", Regex: `Hunt3r\S+`}}})
	ph := regexp.MustCompile(`\{\{PW_[a-z2-7]{8}\}\}`).FindString(masked)
	if ph == "" {
		t.Fatalf("not masked: %s", masked)
	}
	args := `{"cmd":"export DB_PASSWORD=` + ph + `"}`
	in, _ := json.Marshal(map[string]any{"type": "response.function_call_arguments.delta", "delta": args})
	var ev map[string]string
	if err := json.Unmarshal(RestoreJSON(in), &ev); err != nil {
		t.Fatal(err)
	}
	want, _ := json.Marshal(map[string]string{"cmd": "export DB_PASSWORD=Hunt3r!\"\\x"})
	if ev["delta"] != string(want) {
		t.Fatalf("came back %s, want %s", ev["delta"], want)
	}
}
