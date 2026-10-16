package gateway

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// codexShellTools are Codex's exec_command and write_stdin as it offers
// them: the integers it reads (u64, usize, i32) declared "number".
const codexShellTools = `"tools":[
	{"type":"function","name":"exec_command","description":"Runs a command in a PTY, returning output or a session ID for ongoing interaction.","strict":false,"parameters":{"type":"object","properties":{
		"cmd":{"type":"string","description":"Shell command to execute."},
		"workdir":{"type":"string","description":"Optional working directory to run the command in; defaults to the turn cwd."},
		"yield_time_ms":{"type":"number","description":"How long to wait (in milliseconds) for output before yielding."},
		"max_output_tokens":{"type":"number","description":"Maximum number of tokens to return. Excess output will be truncated."}},
		"required":["cmd"],"additionalProperties":false}},
	{"type":"function","name":"write_stdin","description":"Writes characters to an existing unified exec session and returns recent output.","strict":false,"parameters":{"type":"object","properties":{
		"session_id":{"type":"number","description":"Identifier of the running unified exec session."},
		"chars":{"type":"string","description":"Bytes to write to stdin (may be empty to poll)."},
		"yield_time_ms":{"type":"number","description":"How long to wait (in milliseconds) for output before yielding."}},
		"required":["session_id"],"additionalProperties":false}}]`

// codexShellTurn is a Codex turn asking model, with codexShellTools.
func codexShellTurn(model string, stream bool) string {
	return `{"model":"` + model + `","instructions":"You are Codex.","stream":` + strconv.FormatBool(stream) + `,"store":false,
		"include":["reasoning.encrypted_content"],"tool_choice":"auto","parallel_tool_calls":true,` + codexShellTools + `,
		"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"for i in 1..5 echo i, sleep 2"}]}]}`
}

// chatShellTurn is the same turn as a Chat client's.
func chatShellTurn(model string, stream bool) string {
	return `{"model":"` + model + `","stream":` + strconv.FormatBool(stream) + `,"tools":[{"type":"function","function":{"name":"exec_command","parameters":{"type":"object","properties":{"cmd":{"type":"string"},"yield_time_ms":{"type":"number"}}}}}],
		"messages":[{"role":"user","content":"for i in 1..5 echo i, sleep 2"}]}`
}

// messagesShellTurn is the same turn as a Messages client's.
func messagesShellTurn(model string, stream bool) string {
	return `{"model":"` + model + `","max_tokens":1000,"stream":` + strconv.FormatBool(stream) + `,"tools":[{"name":"exec_command","input_schema":{"type":"object","properties":{"cmd":{"type":"string"},"yield_time_ms":{"type":"number"}}}}],
		"messages":[{"role":"user","content":"for i in 1..5 echo i, sleep 2"}]}`
}

// rolloutCall is a function_call Codex recorded in its rollout and the
// output it gave the model for it.
type rolloutCall struct {
	Name, Arguments, Output string
}

// rollout reads a case from testdata/grok_integral_args: the reporter's
// Codex rollout lines (#1431), a function_call and the function_call_output
// of its call_id.
func rollout(t *testing.T, file string) rolloutCall {
	t.Helper()
	f, err := os.Open(filepath.Join("testdata", "grok_integral_args", file))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	type payload struct {
		Type      string `json:"type"`
		Name      string `json:"name"`
		Arguments string `json:"arguments"`
		CallID    string `json:"call_id"`
		Output    string `json:"output"`
	}
	var calls, outputs []payload
	sc := bufio.NewScanner(f)
	sc.Buffer(nil, 1<<20)
	for sc.Scan() {
		var line struct {
			Payload payload `json:"payload"`
		}
		if err := json.Unmarshal(sc.Bytes(), &line); err != nil {
			t.Fatalf("%s: %v", file, err)
		}
		switch line.Payload.Type {
		case "function_call":
			calls = append(calls, line.Payload)
		case "function_call_output":
			outputs = append(outputs, line.Payload)
		}
	}
	if len(calls) != 1 || len(outputs) != 1 || calls[0].CallID != outputs[0].CallID {
		t.Fatalf("%s: want one call and its output, got %v %v", file, calls, outputs)
	}
	return rolloutCall{Name: calls[0].Name, Arguments: calls[0].Arguments, Output: outputs[0].Output}
}

// serdeFloat is the error Codex gave a call whose integer came as a float.
var serdeFloat = regexp.MustCompile("invalid type: floating point `([^`]+)`, expected (\\w+) at line 1 column (\\d+)")

// refusedField is the field of args that Codex's error names: the number
// written as the error quotes it and ending at the column it gives, with
// the Rust type Codex wanted.
func refusedField(t *testing.T, args, output string) (field, want string) {
	t.Helper()
	m := serdeFloat.FindStringSubmatch(output)
	if m == nil {
		t.Fatalf("not a float refused: %s", output)
	}
	col, _ := strconv.Atoi(m[3])
	dec := json.NewDecoder(strings.NewReader(args))
	dec.UseNumber()
	if tok, err := dec.Token(); err != nil || tok != json.Delim('{') {
		t.Fatalf("arguments %s", args)
	}
	for dec.More() {
		k, _ := dec.Token()
		v, err := dec.Token()
		if err != nil {
			t.Fatal(err)
		}
		if n, ok := v.(json.Number); ok && string(n) == m[1] && int(dec.InputOffset()) == col {
			return k.(string), m[2]
		}
	}
	t.Fatalf("no field of %s is %s at column %d", args, m[1], col)
	return "", ""
}

// takes is whether Rust's serde reads lit as typ.
func takes(lit, typ string) bool {
	var err error
	switch typ {
	case "u64", "usize":
		_, err = strconv.ParseUint(lit, 10, 64)
	case "i32":
		_, err = strconv.ParseInt(lit, 10, 32)
	case "i64":
		_, err = strconv.ParseInt(lit, 10, 64)
	default:
		return false
	}
	return err == nil
}

// fieldsOf is each value of a call's arguments as written: a number as its
// literal, a string quoted, an array or object as its JSON.
func fieldsOf(t *testing.T, args string) map[string]string {
	t.Helper()
	var m map[string]json.RawMessage
	if err := json.Unmarshal([]byte(args), &m); err != nil {
		t.Fatalf("%v: %s", err, args)
	}
	out := map[string]string{}
	for k, v := range m {
		out[k] = string(v)
	}
	return out
}

// integralCase is a call Grok makes and what the client is to read in it.
type integralCase struct {
	name, tool, args string
	want             map[string]string // the arguments' fields as the client reads them
	refused          string            // Codex's error over args, for a recorded case
}

// integralCases are the reporter's recorded calls and the rule's edges.
func integralCases(t *testing.T) []integralCase {
	t.Helper()
	var cs []integralCase
	for _, rc := range []struct {
		file string
		want map[string]string
	}{
		{"grok_4_6_exec_command.jsonl", map[string]string{"cmd": `"for i in 1 2 3 4 5; do echo $i; sleep 2; done"`, "yield_time_ms": "2500", "max_output_tokens": "4000"}},
		{"grok_4_7_exec_command.jsonl", map[string]string{"cmd": `"for i in 1 2 3 4 5; do echo $i; sleep 2; done"`, "yield_time_ms": "2500", "max_output_tokens": "4000"}},
		{"grok_4_6_write_stdin.jsonl", map[string]string{"session_id": "17277", "chars": `""`, "yield_time_ms": "120000"}},
	} {
		c := rollout(t, rc.file)
		cs = append(cs, integralCase{name: rc.file, tool: c.Name, args: c.Arguments, want: rc.want, refused: c.Output})
	}
	return append(cs, integralCase{name: "edges", tool: "exec_command",
		args: `{"cmd":"sleep 2.0 \"3.0\" \\","yield_time_ms":2500.5,"max_output_tokens":"4000.0","n":-3.00,"list":[1.0,2.0,0.0],"e":2.5e3,"z":1.0e3,"f":2.50,"o":{"1.0":7.0},"ok":true,"no":null}`,
		want: map[string]string{"cmd": `"sleep 2.0 \"3.0\" \\"`, "yield_time_ms": "2500.5", "max_output_tokens": `"4000.0"`, "n": "-3",
			"list": "[1,2,0]", "e": "2.5e3", "z": "1.0e3", "f": "2.50", "o": `{"1.0":7}`, "ok": "true", "no": "null"}})
}

// check checks the arguments got, read at where, are what c wants.
func (c integralCase) check(t *testing.T, where, got string) {
	t.Helper()
	fields := fieldsOf(t, got)
	if len(fields) != len(c.want) {
		t.Fatalf("%s: arguments %s, want %v", where, got, c.want)
	}
	for k, v := range c.want {
		if fields[k] != v {
			t.Fatalf("%s: %s is %s in %s, want %s", where, k, fields[k], got, v)
		}
	}
	// what Codex refused it over it now takes
	if c.refused != "" {
		field, typ := refusedField(t, c.args, c.refused)
		if !takes(fields[field], typ) {
			t.Fatalf("%s: Codex reads %s as %s and is given %s", where, field, typ, fields[field])
		}
	}
}

// cutsOf are the ways a stream may cut args into deltas: whole, at every
// byte, and in two at each point.
func cutsOf(args string) map[string][]string {
	out := map[string][]string{"whole": {args}}
	var bytes []string
	for i := range len(args) {
		bytes = append(bytes, args[i:i+1])
	}
	out["by byte"] = bytes
	for i := 1; i < len(args); i++ {
		out["at "+strconv.Itoa(i)] = []string{args[:i], args[i:]}
	}
	return out
}

// someCuts is a few of cutsOf, for the tests that go through the gateway:
// whole, by byte, and in two after each '.' and after each zero after one.
func someCuts(args string) map[string][]string {
	all := cutsOf(args)
	out := map[string][]string{"whole": all["whole"], "by byte": all["by byte"]}
	for i := 1; i < len(args); i++ {
		if args[i-1] == '.' || args[i-1] == '0' && strings.Contains(args[max(0, i-3):i], ".") {
			out["at "+strconv.Itoa(i)] = all["at "+strconv.Itoa(i)]
		}
	}
	return out
}

func quoted(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}

// The scanner gives the same arguments however they are cut, and drops
// only a fraction of zeros from a number.
func TestWholeNumsAnyCut(t *testing.T) {
	for in, want := range map[string]string{
		`{"yield_time_ms":2500.0,"max_output_tokens":4000.0}`: `{"yield_time_ms":2500,"max_output_tokens":4000}`,
		`{"a":-3.00,"b":[1.0,2.0],"c":0.0,"d":-0.0}`:          `{"a":-3,"b":[1,2],"c":0,"d":-0}`,
		`{"a":2.5,"b":2.50,"c":2.05,"d":2.500e1}`:             `{"a":2.5,"b":2.50,"c":2.05,"d":2.500e1}`,
		`{"a":1.0e3,"b":1.0E-3,"c":2.5e3,"d":1e3,"e":10}`:     `{"a":1.0e3,"b":1.0E-3,"c":2.5e3,"d":1e3,"e":10}`,
		`{"1.0":"2.0","s":"a\"1.0\\","t":"\\\\"," n":3.0 }`:   `{"1.0":"2.0","s":"a\"1.0\\","t":"\\\\"," n":3 }`,
		`{"ok":true,"no":null,"x":false}`:                     `{"ok":true,"no":null,"x":false}`,
		`[100.000, 7.0]`:                                      `[100, 7]`,
	} {
		if got := wholeArgs(in); got != want {
			t.Errorf("wholeArgs(%s) = %s, want %s", in, got, want)
		}
		for name, pieces := range cutsOf(in) {
			var n wholeNums
			var got string
			for _, p := range pieces {
				got += n.feed(p)
			}
			if got != want {
				t.Errorf("%s cut %s: %s, want %s", in, name, got, want)
			}
		}
	}
}

// responsesCall is a Responses upstream answering every request with one
// call to name with args, streamed in the deltas pieces, or whole.
func responsesCall(name, args string, pieces []string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/responses") {
			http.NotFound(w, r)
			return
		}
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		call := `{"type":"function_call","id":"fc_1","call_id":"call_1","name":"` + name + `","arguments":` + quoted(args) + `,"status":"completed"}`
		done := `{"id":"resp_1","object":"response","status":"completed","model":"grok-4.6","output":[` + call + `],"usage":{"input_tokens":5,"output_tokens":3}}`
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, done)
			return
		}
		lines := []string{
			`event: response.created` + "\n" + `data: {"type":"response.created","sequence_number":0,"response":{"id":"resp_1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.output_item.added` + "\n" + `data: {"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"` + name + `","arguments":"","status":"in_progress"}}`,
		}
		for _, p := range pieces {
			lines = append(lines, `event: response.function_call_arguments.delta`+"\n"+`data: {"type":"response.function_call_arguments.delta","sequence_number":2,"item_id":"fc_1","output_index":0,"delta":`+quoted(p)+`}`)
		}
		lines = append(lines,
			`event: response.function_call_arguments.done`+"\n"+`data: {"type":"response.function_call_arguments.done","sequence_number":3,"item_id":"fc_1","output_index":0,"arguments":`+quoted(args)+`}`,
			`event: response.output_item.done`+"\n"+`data: {"type":"response.output_item.done","sequence_number":4,"output_index":0,"item":`+call+`}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","sequence_number":5,"response":`+done+`}`)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(lines...))
	}
}

// chatCall is a Chat upstream answering with one call to name with args,
// streamed in the pieces, or whole.
func chatCall(name, args string, pieces []string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/chat/completions") {
			http.NotFound(w, r)
			return
		}
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, `{"id":"c1","object":"chat.completion","model":"grok-4.6","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"`+name+`","arguments":`+quoted(args)+`}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}`)
			return
		}
		lines := []string{`data: {"id":"c1","model":"grok-4.6","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"` + name + `","arguments":""}}]}}]}`}
		for _, p := range pieces {
			lines = append(lines, `data: {"id":"c1","model":"grok-4.6","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":`+quoted(p)+`}}]}}]}`)
		}
		lines = append(lines,
			`data: {"id":"c1","model":"grok-4.6","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}`,
			`data: {"id":"c1","model":"grok-4.6","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}`,
			`data: [DONE]`)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(lines...))
	}
}

// messagesCall is a Messages upstream answering with one tool_use of name
// with args, streamed in the pieces, or whole.
func messagesCall(name, args string, pieces []string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/messages") {
			http.NotFound(w, r)
			return
		}
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"grok-4.6","content":[{"type":"tool_use","id":"toolu_1","name":"`+name+`","input":`+args+`}],"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5}}`)
			return
		}
		lines := []string{
			`event: message_start` + "\n" + `data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"grok-4.6","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}`,
			`event: content_block_start` + "\n" + `data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"` + name + `","input":{}}}`,
		}
		for _, p := range pieces {
			lines = append(lines, `event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":`+quoted(p)+`}}`)
		}
		lines = append(lines,
			`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":0}`,
			`event: message_delta`+"\n"+`data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}`,
			`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(lines...))
	}
}

// sseData is each data line's JSON of a stream.
func sseData(t *testing.T, body []byte) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, line := range strings.Split(string(body), "\n") {
		data, ok := strings.CutPrefix(line, "data:")
		data = strings.TrimSpace(data)
		if !ok || data == "[DONE]" || data == "" {
			continue
		}
		var m map[string]any
		if err := json.Unmarshal([]byte(data), &m); err != nil {
			t.Fatalf("%v: %s", err, data)
		}
		out = append(out, m)
	}
	return out
}

// responsesRead is the arguments of the one function_call a Responses
// client reads in a reply, by where it read them: the stream's deltas
// added up, arguments.done, output_item.done and completed, or a whole
// reply's output.
func responsesRead(t *testing.T, body []byte, stream bool) map[string]string {
	t.Helper()
	read := map[string]string{}
	calls := func(where string, items []any) {
		for _, it := range items {
			if m, _ := it.(map[string]any); m["type"] == "function_call" {
				if _, dup := read[where]; dup {
					t.Fatalf("%s: two calls: %s", where, body)
				}
				read[where], _ = m["arguments"].(string)
			}
		}
	}
	if !stream {
		var res map[string]any
		if err := json.Unmarshal(body, &res); err != nil {
			t.Fatalf("%v: %s", err, body)
		}
		out, _ := res["output"].([]any)
		calls("output", out)
		return read
	}
	var deltas strings.Builder
	for _, ev := range sseData(t, body) {
		switch ev["type"] {
		case "response.function_call_arguments.delta":
			d, _ := ev["delta"].(string)
			deltas.WriteString(d)
		case "response.function_call_arguments.done":
			read["arguments.done"], _ = ev["arguments"].(string)
		case "response.output_item.done":
			calls("output_item.done", []any{ev["item"]})
		case "response.completed":
			res, _ := ev["response"].(map[string]any)
			out, _ := res["output"].([]any)
			calls("completed", out)
		}
	}
	read["deltas"] = deltas.String()
	return read
}

// chatRead is a Chat reply's one call's arguments: its deltas added up, or
// the message's.
func chatRead(t *testing.T, body []byte, stream bool) string {
	t.Helper()
	if !stream {
		var res struct {
			Choices []struct {
				Message struct {
					ToolCalls []struct {
						Function struct{ Arguments string }
					} `json:"tool_calls"`
				}
			}
		}
		if err := json.Unmarshal(body, &res); err != nil || len(res.Choices) != 1 || len(res.Choices[0].Message.ToolCalls) != 1 {
			t.Fatalf("%v: %s", err, body)
		}
		return res.Choices[0].Message.ToolCalls[0].Function.Arguments
	}
	var args strings.Builder
	for _, ev := range sseData(t, body) {
		for _, c := range anyList(ev["choices"]) {
			d, _ := c.(map[string]any)["delta"].(map[string]any)
			for _, tc := range anyList(d["tool_calls"]) {
				fn, _ := tc.(map[string]any)["function"].(map[string]any)
				a, _ := fn["arguments"].(string)
				args.WriteString(a)
			}
		}
	}
	return args.String()
}

// messagesRead is a Messages reply's one tool_use input: its
// partial_json added up, or the block's.
func messagesRead(t *testing.T, body []byte, stream bool) string {
	t.Helper()
	if !stream {
		var res struct {
			Content []struct {
				Type  string
				Input json.RawMessage
			}
		}
		if err := json.Unmarshal(body, &res); err != nil {
			t.Fatalf("%v: %s", err, body)
		}
		for _, c := range res.Content {
			if c.Type == "tool_use" {
				return string(c.Input)
			}
		}
		t.Fatalf("no tool_use: %s", body)
	}
	var args strings.Builder
	for _, ev := range sseData(t, body) {
		if d, _ := ev["delta"].(map[string]any); ev["type"] == "content_block_delta" && d["type"] == "input_json_delta" {
			p, _ := d["partial_json"].(string)
			args.WriteString(p)
		}
	}
	return args.String()
}

func anyList(v any) []any {
	l, _ := v.([]any)
	return l
}

// askGrok posts body to path on a fresh gateway.
func askGrok(t *testing.T, path, body string) []byte {
	t.Helper()
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	return rec.Body.Bytes()
}

// checkResponses checks every place a Responses client reads the call,
// and that the deltas add up to what the done events say.
func checkResponses(t *testing.T, c integralCase, body []byte, stream bool) {
	t.Helper()
	read := responsesRead(t, body, stream)
	places := []string{"output"}
	if stream {
		places = []string{"deltas", "arguments.done", "output_item.done", "completed"}
	}
	for _, where := range places {
		got, ok := read[where]
		if !ok {
			t.Fatalf("%s: no call read: %s", where, body)
		}
		c.check(t, where, got)
	}
	if stream && read["deltas"] != read["arguments.done"] {
		t.Fatalf("deltas add up to %s, done says %s", read["deltas"], read["arguments.done"])
	}
}

// A Grok model writes a whole number in a call's arguments as 2500.0, which
// Codex refuses ("invalid type: floating point `2500.0`, expected u64",
// #1431, through a relay in front of grok-4.6 and grok-4.7). Relayed from
// a Responses API, streamed in deltas cut anywhere or whole, Codex reads
// integers everywhere it reads the call, and the deltas add up to the
// done events.
func TestGrokCallsGiveCodexIntegers(t *testing.T) {
	for _, c := range integralCases(t) {
		for cut, pieces := range someCuts(c.args) {
			for _, stream := range []bool{true, false} {
				if !stream && cut != "whole" {
					continue
				}
				t.Run(c.name+" "+cut+map[bool]string{true: " stream", false: ""}[stream], func(t *testing.T) {
					fresh(t)
					up := httptest.NewServer(responsesCall(c.tool, c.args, pieces))
					defer up.Close()
					if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"grok-4.6"}, Responses: up.URL + "/v1"}); err != nil {
						t.Fatal(err)
					}
					checkResponses(t, c, askGrok(t, "/v1/responses", codexShellTurn("relay/grok-4.6", stream)), stream)
				})
			}
		}
	}
}

// A relay with only Chat for grok-4.6 is translated for: Codex reads the
// same integers in the Responses reply magpie writes.
func TestGrokCallsGiveCodexIntegersTranslated(t *testing.T) {
	for _, c := range integralCases(t) {
		for cut, pieces := range someCuts(c.args) {
			for _, stream := range []bool{true, false} {
				if !stream && cut != "whole" {
					continue
				}
				t.Run(c.name+" "+cut+map[bool]string{true: " stream", false: ""}[stream], func(t *testing.T) {
					fresh(t)
					up := httptest.NewServer(chatCall(c.tool, c.args, pieces))
					defer up.Close()
					if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"grok-4.6"}, Chat: up.URL + "/v1"}); err != nil {
						t.Fatal(err)
					}
					checkResponses(t, c, askGrok(t, "/v1/responses", codexShellTurn("relay/grok-4.6", stream)), stream)
				})
			}
		}
	}
}

// A Chat client relayed to a Chat upstream, and a Messages client relayed
// to a Messages upstream or translated from Chat, read the same integers.
func TestGrokCallsGiveIntegersOnEveryProtocol(t *testing.T) {
	for _, c := range integralCases(t) {
		for cut, pieces := range someCuts(c.args) {
			for _, stream := range []bool{true, false} {
				if !stream && cut != "whole" {
					continue
				}
				name := c.name + " " + cut + map[bool]string{true: " stream", false: ""}[stream]
				t.Run("chat "+name, func(t *testing.T) {
					fresh(t)
					up := httptest.NewServer(chatCall(c.tool, c.args, pieces))
					defer up.Close()
					if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"grok-4.6"}, Chat: up.URL + "/v1"}); err != nil {
						t.Fatal(err)
					}
					c.check(t, "chat", chatRead(t, askGrok(t, "/v1/chat/completions", chatShellTurn("relay/grok-4.6", stream)), stream))
				})
				t.Run("messages "+name, func(t *testing.T) {
					fresh(t)
					up := httptest.NewServer(messagesCall(c.tool, c.args, pieces))
					defer up.Close()
					if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"grok-4.6"}, Anthropic: up.URL + "/v1"}); err != nil {
						t.Fatal(err)
					}
					c.check(t, "messages", messagesRead(t, askGrok(t, "/v1/messages", messagesShellTurn("relay/grok-4.6", stream)), stream))
				})
				t.Run("messages from chat "+name, func(t *testing.T) {
					fresh(t)
					up := httptest.NewServer(chatCall(c.tool, c.args, pieces))
					defer up.Close()
					if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"grok-4.6"}, Chat: up.URL + "/v1"}); err != nil {
						t.Fatal(err)
					}
					c.check(t, "messages from chat", messagesRead(t, askGrok(t, "/v1/messages", messagesShellTurn("relay/grok-4.6", stream)), stream))
				})
			}
		}
	}
}

// A model magpie names otherwise but sends upstream as a Grok model is
// Grok's too.
func TestGrokCallsByUpstreamName(t *testing.T) {
	c := integralCases(t)[0]
	fresh(t)
	up := httptest.NewServer(responsesCall(c.tool, c.args, []string{c.args}))
	defer up.Close()
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"x-ai/grok-4.6"}, Responses: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	for _, stream := range []bool{true, false} {
		checkResponses(t, c, askGrok(t, "/v1/responses", codexShellTurn("relay/x-ai/grok-4.6", stream)), stream)
	}
}

// Another vendor's model is not Grok's: the same call reaches the client
// as it came, relayed byte for byte, and translated with its floats.
func TestOnlyGrokCallsGetIntegers(t *testing.T) {
	c := rollout(t, "grok_4_6_exec_command.jsonl")
	pieces := []string{c.Arguments[:60], c.Arguments[60:]}
	for _, stream := range []bool{true, false} {
		for _, rc := range []struct {
			path, body string
			up         http.HandlerFunc
			p          provider.Provider
		}{
			{"/v1/responses", codexShellTurn("relay/gpt-5.5", stream), responsesCall(c.Name, c.Arguments, pieces), provider.Provider{Responses: "/v1"}},
			{"/v1/chat/completions", chatShellTurn("relay/gpt-5.5", stream), chatCall(c.Name, c.Arguments, pieces), provider.Provider{Chat: "/v1"}},
			{"/v1/messages", messagesShellTurn("relay/gpt-5.5", stream), messagesCall(c.Name, c.Arguments, pieces), provider.Provider{Anthropic: "/v1"}},
		} {
			fresh(t)
			up := httptest.NewServer(rc.up)
			p := rc.p
			p.ID, p.Name, p.Key, p.Models = "relay", "relay", "k", []string{"gpt-5.5"}
			if p.Responses != "" {
				p.Responses = up.URL + p.Responses
			}
			if p.Chat != "" {
				p.Chat = up.URL + p.Chat
			}
			if p.Anthropic != "" {
				p.Anthropic = up.URL + p.Anthropic
			}
			if err := provider.Save(p); err != nil {
				t.Fatal(err)
			}
			// what the relay itself answers
			direct := httptest.NewRecorder()
			rc.up(direct, httptest.NewRequest("POST", rc.path, strings.NewReader(strings.Replace(rc.body, "relay/gpt-5.5", "gpt-5.5", 1))))
			if got := askGrok(t, rc.path, rc.body); !bytes.Equal(got, direct.Body.Bytes()) {
				t.Fatalf("%s stream %v:\n%s\nwant\n%s", rc.path, stream, got, direct.Body)
			}
			up.Close()
		}

		fresh(t)
		chat := httptest.NewServer(chatCall(c.Name, c.Arguments, pieces))
		if err := provider.Save(provider.Provider{ID: "relay", Name: "relay", Key: "k", Models: []string{"gpt-5.5"}, Chat: chat.URL + "/v1"}); err != nil {
			t.Fatal(err)
		}
		for where, got := range responsesRead(t, askGrok(t, "/v1/responses", codexShellTurn("relay/gpt-5.5", stream)), stream) {
			if got != c.Arguments {
				t.Fatalf("stream %v: %s translated %q, want %q", stream, where, got, c.Arguments)
			}
		}
		chat.Close()
	}
}

// grokModel tells Grok's models by the id, however a relay or a router
// names their vendor.
func TestGrokModel(t *testing.T) {
	for id, want := range map[string]bool{
		"grok-4.6": true, "grok-4.7": true, "xai/grok-4.5": true, "x-ai/grok-4": true, "relay/grok-4.6": true, "Grok-4": true, "grok-code-fast-1": true,
		"gpt-5.5": false, "deepseek-v4-pro": false, "openrouter/x-ai-grok-4": false, "": false,
	} {
		if grokModel(id) != want {
			t.Errorf("grokModel(%q) = %v", id, !want)
		}
	}
}
