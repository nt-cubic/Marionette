package gateway

// PLUGIN-SERVED (see AGENTS.md): Qoder ("qoder") and Qoder CN ("qoder-cn")
// are deprecated built-in subscriptions served by their plugin,
// @magpie-community/opencode-qoder-auth, each once moved onto it
// (provider.Moved; the default for a new sign-in). A moved one's
// sign-ins, models, requests and usage are all the plugin's, never this
// code's (only the move, in migrate*.go, still reads its accounts). A fix here alone doesn't reach those users; fix the plugin
// (github.com/magpie-community/plugins, packages/qoder) and raise the
// movers' min in internal/provider/migrate_qoder.go.

import (
	"bufio"
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/qoder"
)

// A Qoder subscription is served through the API the Qoder client talks to
// (api3.qoder.sh agent_chat_generation SSE), signed with the COSY envelope and
// encoded with the client's body codec, the way a Devin one is (devin.go).
// Qoder exposes tool calls as XML in its text, so the request is given the
// matching contract and the reply is adapted back into tool-call events.
// The protocol lives in internal/qoder.
//
// The XML markers below are written with hex escapes for their angle brackets
// so this source holds no literal tag the tooling would mistake for a call.
const (
	qoderCallOpen  = "\x3ctool_call\x3e"
	qoderCallClose = "\x3c/tool_call\x3e"
)

var (
	qoderFunc = regexp.MustCompile(`(?s)\x3cfunction=([^>]+)\x3e(.*?)\x3c/function\x3e`)
	qoderArg  = regexp.MustCompile(`(?s)\x3cparameter=([^>]+)\x3e(.*?)\x3c/parameter\x3e`)
)

const qoderSys = "You are a Qoder agent. Use the instructions below and the tools available to you to assist the user."

// qoderAuth hands a request the signed-in account's uid and a live job
// token, and qoderURL the chat endpoint on the credential's site (qoder.com's
// or Qoder CN's); vars so tests can stand in for them, the way a Devin round
// stands in for devinAuth.
var (
	qoderAuth  = provider.QoderCredentialOf
	qoderModel = provider.QoderModelOf
	qoderURL   = func(c *qoder.Credential) string { return c.OnSite().ChatURL() }
)

// serveQoder answers a request through Qoder's API.
func (s *Server) serveQoder(w http.ResponseWriter, r *http.Request, from provider.Protocol, p provider.Provider, model string, body []byte, usage *Usage) (int, string) {
	req, err := parse(from, body)
	if err != nil {
		return writeError(w, from, 400, err.Error()), err.Error()
	}
	// the vendor's own API is told the answer's format in words
	req = req.inSystem()
	req.Model = model
	ask := s.askQoder(p.Account.Agent, model, p.Account.User)
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	q := req
	var tool string
	if req.WebSearch && !searching(r.Context()) {
		if canSearch() {
			copy := *req
			copy.WebSearch = false
			search := searchTool(copy.Tools)
			copy.Tools = append(slices.Clone(copy.Tools), search)
			copy.Messages = slices.Clone(copy.Messages)
			q, tool = &copy, search.Name
		}
	}
	events, status, msg, wait := s.askQoderWait(p.Account.Agent, model, p.Account.User)(ctx, q)
	if events == nil {
		retryAfterOn(w, wait)
		return writeError(w, from, status, msg), msg
	}
	if tool != "" {
		first := events
		out := make(chan Event, 16)
		go func() {
			defer close(out)
			s.searchRounds(ctx, q, tool, first, ask, out)
		}()
		events = out
	}
	return relayQoder(w, from, req, events, usage, cancel)
}

// relayQoder preserves upstream failures before committing a response, including
// failures after partial non-streaming text. Streaming failures after output
// has begun are terminal error events. Keep this policy to the backends that
// report their own statuses: Qoder's, and Zed's (relayStatus).
func relayQoder(w http.ResponseWriter, from provider.Protocol, req *Request, events <-chan Event, usage *Usage, abort context.CancelFunc) (int, string) {
	return relayStatus(w, from, "Qoder", req, events, usage, abort)
}

// relayStatus is relayQoder for the backend called name.
func relayStatus(w http.ResponseWriter, from provider.Protocol, name string, req *Request, events <-chan Event, usage *Usage, abort context.CancelFunc) (int, string) {
	fail := func(ev Event) (int, string) {
		abort()
		code := ev.Status
		if code < 400 || code > 599 {
			code = 502
		}
		retryAfterOn(w, ev.Wait)
		return writeError(w, from, code, ev.Text), ev.Text
	}
	if req.Stream {
		var head []Event
		for ev := range events {
			if ev.Kind == KError {
				return fail(ev)
			}
			head = append(head, ev)
			if ev.Kind != KStart && ev.Kind != KUsage {
				break
			}
		}
		if len(head) == 0 || head[len(head)-1].Kind == KStart || head[len(head)-1].Kind == KUsage {
			return fail(Event{Text: name + " ended without an answer"})
		}
		sw := newSSEWriter(w)
		enc := encoder(from, sw, req, usage)
		var failed string
		see := func(ev Event) bool {
			if ev.Kind == KStart || ev.Kind == KUsage {
				usage.add(ev.Usage)
			}
			enc.event(ev)
			if ev.Kind == KError {
				failed = ev.Text
				return false
			}
			return true
		}
		for _, ev := range head {
			see(ev)
		}
		// kept from the client's idle timeout while none comes (#436)
		relayEvents(events, sw, enc, see)
		if failed != "" {
			abort()
			return 200, failed
		}
		enc.finish()
		return 200, ""
	}
	var col collector
	for ev := range events {
		if ev.Kind == KError {
			return fail(ev)
		}
		col.add(ev)
	}
	res := col.finish()
	usage.add(res.Usage)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(200)
	w.Write(renderUsage(from, res, req, usage))
	return 200, ""
}

// retryAfterOn sends, with an error about to be written to w, how long
// the vendor said to wait before asking again, when it said: the agent
// reads it, and so does the gateway's busy handling (noteBusy).
func retryAfterOn(w http.ResponseWriter, wait time.Duration) {
	if wait > 0 {
		w.Header().Set("Retry-After", strconv.Itoa(int(math.Ceil(wait.Seconds()))))
	}
}

// askQoder is a round for Qoder's API, on the site agent ("qoder" or
// "qoder-cn") names.
func (s *Server) askQoder(agent, model, user string) round {
	ask := s.askQoderWait(agent, model, user)
	return func(ctx context.Context, req *Request) (<-chan Event, int, string) {
		events, status, msg, _ := ask(ctx, req)
		return events, status, msg
	}
}

// askQoderWait is askQoder that also says, of a failure, how long Qoder
// said to wait before asking again (its queue's retryAfterSeconds), for
// serveQoder to send as Retry-After.
func (s *Server) askQoderWait(agent, model, user string) func(ctx context.Context, req *Request) (<-chan Event, int, string, time.Duration) {
	return func(ctx context.Context, req *Request) (<-chan Event, int, string, time.Duration) {
		cred, err := qoderAuth(ctx, agent, user)
		if err != nil {
			// only a sign-in that is gone asks the agent to log in again;
			// a refresh that timed out or never reached Qoder may pass
			status := 502
			if errors.Is(err, provider.ErrQoderSignIn) {
				status = 401
			}
			return nil, status, "Qoder: " + err.Error(), 0
		}
		config, err := qoderModel(ctx, agent, user, model)
		if err != nil {
			return nil, 400, err.Error(), 0
		}
		plain, err := qoderChatBody(req, config)
		if err != nil {
			return nil, 500, "Qoder: " + err.Error(), 0
		}
		wire := qoder.EncodeRequestBody(plain)
		ts := time.Now().Unix()
		chat := qoderURL(cred)
		headers, err := qoder.BuildCosyHeaders(chat, &qoder.User{UID: cred.UID, Token: cred.Token,
			Name: cred.Name, Email: cred.Email, MachineID: cred.MachineID}, wire, ts)
		if err != nil {
			return nil, 500, "Qoder: " + err.Error(), 0
		}
		hr, err := http.NewRequestWithContext(ctx, http.MethodPost, chat, strings.NewReader(wire))
		if err != nil {
			return nil, 500, "Qoder: " + err.Error(), 0
		}
		for k, v := range headers {
			hr.Header.Set(k, v)
		}
		hr.Header.Set("Accept", "text/event-stream")
		hr.Header.Set("Cache-Control", "no-cache")
		hr.Header.Set("Accept-Encoding", "identity")
		hr.Header.Set("X-Model-Key", config.Key)
		hr.Header.Set("X-Model-Source", config.Source)
		res, err := s.client.Do(hr)
		if err != nil {
			return nil, 502, "Qoder: " + err.Error(), 0
		}
		res = notAnAPIReply(res, "") // a web page served 200 (#1012)
		if res.StatusCode/100 != 2 {
			b, _ := io.ReadAll(io.LimitReader(res.Body, 1<<20))
			res.Body.Close()
			status, msg, wait := qoderFailure(res.StatusCode, b)
			return nil, status, msg, wait
		}
		events := make(chan Event, 32)
		go decodeQoder(ctx, res, events, model)
		return events, 0, "", 0
	}
}

// Qoder's own error codes, read as Qoder's client reads them (the
// QoderErrorCode table in Qoder.app's qoder-agent-sdk worker), and as the
// plugin does (@magpie-community/opencode-qoder-auth 0.2.14): only 105
// (loginExpired) is a lapsed sign-in there, so a 401/403 that carries
// another code isn't one here either (plugins #98: the model's queue,
// 10605, came as a 403 and read as "sign in again").
const qoderQueued = "10605"

var (
	// the codes Qoder's client counts as a plan's usage or credits used up
	qoderQuotaCodes = map[string]bool{"110": true, "112": true, "113": true, "114": true, "115": true,
		"116": true, "117": true, "118": true, "119": true, "122": true}
	// refusals of this request, not of the account: sensitive content,
	// not supported, input too long, too many media, a custom model's
	// service (Qoder's table has no 100402)
	qoderRequestCodes = map[string]bool{"406": true, "416": true, "430": true, "80411": true, "80412": true,
		"100400": true, "100401": true, "100403": true}
)

// qoderBusyWait is how long a queued request waits when Qoder names no time.
const qoderBusyWait = 30 * time.Second

// qoderFailure maps a bad upstream status to magpie's status and message,
// and how long Qoder said to wait before asking again (0 when it didn't).
// Qoder's code in the body says what the failure is, whatever the status.
func qoderFailure(status int, body []byte) (int, string, time.Duration) {
	if status < 400 || status > 599 {
		status = 502
	}
	msg := strings.TrimSpace(string(body))
	var code string
	var queue gjson.Result
	if gjson.ValidBytes(body) {
		code = gjson.GetBytes(body, "code").String()
		m := gjson.GetBytes(body, "message").String()
		if m != "" {
			msg = m
		}
		// the model vendor's own reason, e.g. an effort the model refuses
		if d := gjson.Get(gjson.GetBytes(body, "details").String(), "error.message").String(); d != "" {
			msg += ": " + d
		}
		// the queue's state: a field, or the message itself as JSON
		if q := gjson.GetBytes(body, "queue"); q.IsObject() {
			queue = q
		} else if q := gjson.Parse(m); strings.HasPrefix(strings.TrimSpace(m), "{") && q.IsObject() &&
			(q.Get("isQueued").Exists() || q.Get("serviceAvailable").Exists() || q.Get("retryAfterSeconds").Exists()) {
			queue = q
		}
	}
	if msg == "" {
		msg = http.StatusText(status)
	}
	// Qoder's queue for the model: busy for now, with when to ask again.
	// The sign-in is fine (the same token reads the models and the usage),
	// so it is a 503 with Retry-After, which the gateway counts as busy and
	// asks again after, and never "sign in again"
	if code == qoderQueued || queue.Get("isQueued").Type == gjson.True || queue.Get("serviceAvailable").Type == gjson.False {
		wait := qoderBusyWait
		if n := queue.Get("retryAfterSeconds").Float(); n > 0 {
			wait = time.Duration(math.Ceil(n)) * time.Second
		}
		what := "is busy"
		if queue.Get("isQueued").Type == gjson.True {
			what = "queued the request"
		}
		if k := queue.Get("modelKey").String(); k != "" {
			what += " for " + k
		}
		if t := queue.Get("queueType").String(); t != "" {
			what += " (this plan's queue: " + t + ")"
		}
		return 503, fmt.Sprintf("Qoder %s; try again in %ds", what, int(wait/time.Second)), wait
	}
	switch {
	case qoderQuotaCodes[code]:
		return 429, "usage limit reached: " + msg, 0
	case qoderRequestCodes[code]:
		return 400, fmt.Sprintf("Qoder refused the request (%s): %s", code, msg), 0
	case status == http.StatusUnauthorized || status == http.StatusForbidden:
		return 401, "Qoder: the sign-in lapsed — sign in again", 0
	case status == http.StatusTooManyRequests || strings.Contains(strings.ToLower(msg), "quota"):
		return 429, "usage limit reached: " + msg, 0
	}
	return status, "Qoder: " + msg, 0
}

// qoderChatBody is the plaintext JSON Qoder's agent_chat_generation takes,
// built from magpie's IR request the way the reference does: messages as
// content blocks, tool calls and results as OpenAI tool turns, and the
// caller's tools beside them.
func qoderChatBody(req *Request, model qoder.ModelInfo) ([]byte, error) {
	if model.Key == "" || len(model.Config) == 0 {
		return nil, fmt.Errorf("Qoder: missing model configuration")
	}
	sys := map[string]any{"type": "text", "text": qoderSysText(req)}
	all := []any{map[string]any{"role": "system", "content": []any{sys}}}

	asked := req.Effort
	if req.ThinkOff {
		asked = "none" // Effort reads none as low
	}
	thinking, effort := qoderEffort(asked, model)
	maxTok := int64(32000)
	if req.MaxTokens > 0 {
		maxTok = int64(req.MaxTokens)
	}
	params := map[string]any{"enable_thinking": thinking, "max_tokens": maxTok}
	if effort != "" {
		params["reasoning_effort"] = effort
	}
	if model.MaxInputTokens > 0 {
		params["context_length"] = model.MaxInputTokens
	}
	body := map[string]any{
		"parameters": params,
		"business": map[string]any{
			"product": "app", "version": "1.1.49", "type": "agent",
			"id": qoder.NewID(), "name": "magpie session",
			"begin_at": time.Now().UnixMilli(), "stage": "start",
		},
		"agent_id":     "agent_common",
		"task_id":      "common",
		"session_type": "app",
		"model_config": model.Config,
		"system":       []any{sys},
		"messages":     append(all, qoderMessages(req.Messages)...),
	}
	if req.ToolChoice != "none" && len(req.Tools) > 0 {
		body["tools"] = qoderTools(req.Tools)
	}
	return json.Marshal(body)
}

// qoderMessages renders magpie's turns as OpenAI turns, the format Qoder's
// chat endpoint takes: an assistant's tool calls as tool_calls and their
// results as tool turns named by tool_call_id, so a result is paired with
// its call and not by order. A tool turn holds text only, so the images a
// tool returned go after the tool turns in a user turn — as the start of
// the user's own turn when one comes next — the way buildChat sends them.
func qoderMessages(ms []Message) []any {
	var out []any
	names := map[string]string{}
	var seen []any
	seeLater := func(p Part) int {
		var ims []any
		for _, im := range p.Images {
			if im.Data != "" {
				ims = append(ims, map[string]any{"type": "image_url",
					"image_url": map[string]any{"url": "data:" + im.MediaType + ";base64," + im.Data}})
			}
		}
		if len(ims) == 0 {
			return 0
		}
		of := "tool call " + p.CallID
		if name := cmp.Or(names[p.CallID], p.Name); name != "" {
			of = name + " (" + of + ")"
		}
		seen = append(seen, map[string]any{"type": "text", "text": "[From the result of " + of + ":]"})
		seen = append(seen, ims...)
		return len(ims)
	}
	showSeen := func() {
		if len(seen) > 0 {
			out = append(out, map[string]any{"role": "user", "content": seen})
			seen = nil
		}
	}
	for _, m := range ms {
		var blocks []any
		var calls []map[string]any
		var text string
		flush := func() {
			if len(blocks) == 0 {
				return
			}
			if m.Role == "user" && len(seen) > 0 {
				blocks, seen = append(seen, blocks...), nil
			}
			out = append(out, map[string]any{"role": m.Role, "content": blocks})
			blocks = nil
		}
		if m.Role != "user" {
			showSeen()
		}
		for _, p := range m.Parts {
			switch p.Kind {
			case Text:
				blocks = append(blocks, map[string]any{"type": "text", "text": p.Text})
				if text == "" {
					text = p.Text
				}
			case Image:
				if p.Data != "" {
					blocks = append(blocks, map[string]any{"type": "image_url",
						"image_url": map[string]any{"url": "data:" + p.MediaType + ";base64," + p.Data}})
				}
			case ToolCall:
				if m.Role != "assistant" {
					continue
				}
				id := p.ID
				if id == "" {
					id = "call_" + qoder.NewID()
				}
				names[id] = p.Name
				calls = append(calls, map[string]any{"id": id, "type": "function",
					"function": map[string]any{"name": p.Name, "arguments": string(argsOf(p))}})
			case ToolResult:
				flush()
				txt := p.Text
				if p.IsError {
					txt = "Error: " + txt
				}
				if n := seeLater(p); n > 0 {
					note := fmt.Sprintf("[The tool returned %d images; they follow in the next message.]", n)
					if n == 1 {
						note = "[The tool returned an image; it follows in the next message.]"
					}
					if strings.TrimSpace(txt) != "" {
						txt += "\n\n"
					}
					txt += note
				}
				out = append(out, map[string]any{"role": "tool", "tool_call_id": p.CallID, "content": txt})
			}
		}
		if m.Role == "assistant" && len(calls) > 0 {
			out = append(out, map[string]any{"role": "assistant", "content": text, "tool_calls": calls})
			continue
		}
		flush()
	}
	showSeen()
	return out
}

// qoderTools is the caller's tools as Qoder's own native function tools.
func qoderTools(ts []Tool) []any {
	var out []any
	for _, t := range ts {
		fn := map[string]any{"name": t.Name, "description": t.Description}
		sch := t.Schema
		if strings.TrimSpace(string(sch)) == "" {
			sch = json.RawMessage(`{"type":"object","properties":{}}`)
		}
		fn["parameters"] = json.RawMessage(sch)
		out = append(out, map[string]any{"type": "function", "function": fn})
	}
	return out
}

// qoderEffort is whether to think and at which of the model's own efforts
// ("" to send none). Asked for none, a model that always thinks thinks at
// its lowest; asked for nothing, or for a level Qoder doesn't name, it
// thinks at its own default.
func qoderEffort(asked string, model qoder.ModelInfo) (bool, string) {
	want := strings.ToLower(asked)
	if !slices.Contains(effortRank, want) {
		want = ""
	}
	if !model.Thinks || want == "none" && !model.AlwaysThinks {
		return false, ""
	}
	if len(model.Efforts) == 0 {
		return true, ""
	}
	switch want {
	case "":
		return true, model.DefaultEffort
	case "none":
		return true, model.Efforts[0]
	}
	return true, fitEffort(want, model.Efforts)
}

// qoderSysText is the system prompt: Qoder's own line plus the caller's.
// The caller's tools go to Qoder as native function tools.
func qoderSysText(req *Request) string {
	sys := qoderSys
	if req.System != "" {
		sys = sys + "\n\n" + req.System
	}
	if req.ToolChoice == "required" && len(req.Tools) > 0 {
		sys += "\nYou must call an available function in this response."
	}
	return sys
}

// decodeQoder turns Qoder's SSE reply into events: each "data:" line carries an
// envelope whose "body" is a standard OpenAI chunk, whose content may hold
// embedded tool calls to lift out.
func decodeQoder(ctx context.Context, res *http.Response, out chan<- Event, model string) {
	defer func() { res.Body.Close(); close(out) }()
	send := func(ev Event) bool {
		select {
		case out <- ev:
			return true
		case <-ctx.Done():
			return false
		}
	}
	if !send(Event{Kind: KStart, MsgID: "msg_" + randomToken(), Model: model}) {
		return
	}
	a := qoderText{tool: -1}
	// emit lifts one piece of the split text into events: a call, the text,
	// or a call that can't be read, which ends the turn as an error so the
	// agent can try it again (plugins#132, as the moved qoder plugin's).
	// False when the turn ends here: the consumer went, or an error was
	// sent — an error frame carries no finish.
	emit := func(frag qoderFrag) bool {
		switch {
		case frag.err != "":
			send(Event{Kind: KError, Status: 502, Text: frag.err})
			return false
		case frag.call != nil:
			if !send(Event{Kind: KToolStart, ID: "call_" + qoder.NewID(), Name: frag.call.name}) {
				return false
			}
			return send(Event{Kind: KToolArgs, Text: frag.call.args})
		case frag.text != "":
			return send(Event{Kind: KText, Text: frag.text})
		}
		return true
	}
	finish := func() {
		for _, frag := range a.flush() {
			if !emit(frag) {
				return
			}
		}
		stop := "stop"
		if a.sawTool {
			stop = "tool"
		}
		send(Event{Kind: KStop, Stop: stop})
	}
	sc := bufio.NewScanner(res.Body)
	sc.Buffer(make([]byte, 0, 64<<10), 32<<20)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		payload := strings.TrimSpace(strings.TrimPrefix(line, "data:"))
		if payload == "" || !gjson.Valid(payload) {
			continue
		}
		inner := gjson.Get(payload, "body").String()
		if status := gjson.Get(payload, "statusCodeValue"); status.Exists() && status.Int() != 200 {
			data := []byte(inner)
			if len(data) == 0 {
				data = []byte(payload)
			}
			code, msg, wait := qoderFailure(int(status.Int()), data)
			send(Event{Kind: KError, Status: code, Text: msg, Wait: wait})
			return
		}
		if inner == "" || !json.Valid([]byte(inner)) {
			continue
		}
		delta := gjson.Get(inner, "choices.0.delta")
		if rc := delta.Get("reasoning_content").String(); rc != "" {
			if !send(Event{Kind: KThink, Text: rc}) {
				return
			}
		}
		if content := delta.Get("content"); content.Exists() {
			for _, frag := range a.feed(content.String()) {
				if !emit(frag) {
					return
				}
			}
		}
		// Qoder also serves tool calls in the OpenAI way: the id it sends
		// is the one a result is answered with, so it is kept.
		for _, tc := range delta.Get("tool_calls").Array() {
			idx := int(tc.Get("index").Int())
			if idx != a.tool {
				a.tool = idx
				for _, frag := range a.flush() {
					if !emit(frag) {
						return
					}
				}
				id := tc.Get("id").String()
				if id == "" {
					id = "call_" + qoder.NewID()
				}
				if !send(Event{Kind: KToolStart, ID: id, Name: tc.Get("function.name").String()}) {
					return
				}
				a.sawTool = true
			}
			if args := tc.Get("function.arguments").String(); args != "" {
				if !send(Event{Kind: KToolArgs, Text: args}) {
					return
				}
			}
		}
		if fr := gjson.Get(inner, "choices.0.finish_reason").String(); fr != "" {
			if u := gjson.Get(inner, "usage"); u.Exists() {
				send(Event{Kind: KUsage, Usage: Usage{
					Input: int(u.Get("prompt_tokens").Int()), Output: int(u.Get("completion_tokens").Int())}})
			}
			finish()
			return
		}
	}
	if err := sc.Err(); err != nil {
		send(Event{Kind: KError, Status: 502, Text: "Qoder: reading stream: " + err.Error()})
		return
	}
	finish()
}

// qoderText is the streaming tool-call splitter: it holds text until a full
// tool-call block arrives, then emits it as a call. A block on its way to
// being a call that can't be read, or that the stream ends inside, is an
// error, never the answer (plugins#132, as the moved qoder plugin's 0.2.16):
// the turn fails, so it can be tried again. A block whose JSON is whole when
// the stream ends without its close is still the call; one that is no call
// at all, or one inside code, stays text.
type qoderText struct {
	textBuf string
	callBuf string
	inCall  bool
	sawTool bool
	sent    string // the text given out so far, to tell a block in code from a call
	tool    int    // index of the native tool call being assembled, -1 for none
}

type qoderFrag struct {
	text string
	call *qoderCall
	err  string
}
type qoderCall struct{ name, args string }

// qoderQuoted says text ends inside a code fence or an inline code span: a
// tool-call block there is prose about one, not a call.
func qoderQuoted(text string) bool {
	if strings.Count(text, "```")%2 == 1 {
		return true
	}
	line := text[strings.LastIndex(text, "\n")+1:]
	return strings.Count(strings.ReplaceAll(line, "```", ""), "`")%2 == 1
}

// qoderCallish says a block was on its way to being a call: its JSON began,
// or the XML function form.
func qoderCallish(v string) bool {
	return strings.HasPrefix(strings.TrimLeft(v, " \t\r\n"), "{") || strings.Contains(v, "\x3cfunction=")
}

const (
	qoderCallUnreadable = "Qoder: the model's tool call, written as text, can't be read; it is not sent on as the answer"
	qoderCallCutShort   = "Qoder: the model's tool call, written as text, was cut short; it is not sent on as the answer"
)

func (a *qoderText) feed(input string) []qoderFrag {
	var out []qoderFrag
	for input != "" {
		if !a.inCall {
			comb := a.textBuf + input
			a.textBuf = ""
			i := strings.Index(comb, qoderCallOpen)
			if i < 0 {
				if keep := qoderSuffix(comb); keep < len(comb) {
					a.text(comb[:len(comb)-keep])
					out = append(out, qoderFrag{text: comb[:len(comb)-keep]})
					a.textBuf = comb[len(comb)-keep:]
				} else {
					a.textBuf = comb
				}
				break
			}
			if i > 0 {
				a.text(comb[:i])
				out = append(out, qoderFrag{text: comb[:i]})
			}
			input = comb[i+len(qoderCallOpen):]
			a.inCall = true
			continue
		}
		comb := a.callBuf + input
		a.callBuf = ""
		j := strings.Index(comb, qoderCallClose)
		if j < 0 {
			a.callBuf = comb
			break
		}
		body := comb[:j]
		if c, ok := qoderParseCall(body); ok {
			a.sawTool = true
			out = append(out, qoderFrag{call: &c})
		} else if qoderCallish(body) && !qoderQuoted(a.sent) {
			a.inCall = false
			return append(out, qoderFrag{err: qoderCallUnreadable})
		} else {
			a.text(qoderCallOpen + body + qoderCallClose)
			out = append(out, qoderFrag{text: qoderCallOpen + body + qoderCallClose})
		}
		input = comb[j+len(qoderCallClose):]
		a.inCall = false
	}
	return out
}

// text hands a piece of text out, remembering it: whether the next block is
// inside code is told from what came before it.
func (a *qoderText) text(t string) { a.sent += t }

func (a *qoderText) flush() []qoderFrag {
	if a.inCall {
		body := a.callBuf
		a.inCall, a.callBuf = false, ""
		if qoderCallish(body) && !qoderQuoted(a.sent) {
			if c, ok := qoderParseCall(body); ok {
				a.sawTool = true
				return []qoderFrag{{call: &c}}
			}
			return []qoderFrag{{err: qoderCallCutShort}}
		}
		t := qoderCallOpen + body
		a.text(t)
		if t != "" {
			return []qoderFrag{{text: t}}
		}
	}
	if a.textBuf != "" {
		t := a.textBuf
		a.textBuf = ""
		a.text(t)
		return []qoderFrag{{text: t}}
	}
	return nil
}

// qoderSuffix is the length of the tail that could still become a call marker.
func qoderSuffix(v string) int {
	for size := len(qoderCallOpen) - 1; size > 0; size-- {
		if strings.HasSuffix(v, qoderCallOpen[:size]) {
			return size
		}
	}
	return 0
}

// qoderParseCall reads one tool-call block: a JSON {name,arguments}, or the
// XML function/parameter form.
func qoderParseCall(v string) (qoderCall, bool) {
	var framed struct {
		Name      string          `json:"name"`
		Arguments json.RawMessage `json:"arguments"`
	}
	if json.Unmarshal([]byte(strings.TrimSpace(v)), &framed) == nil && strings.TrimSpace(framed.Name) != "" {
		var m map[string]json.RawMessage
		if json.Unmarshal(framed.Arguments, &m) == nil && m != nil {
			return qoderCall{name: strings.TrimSpace(framed.Name), args: string(framed.Arguments)}, true
		}
	}
	m := qoderFunc.FindStringSubmatch(v)
	if len(m) != 3 {
		return qoderCall{}, false
	}
	name := strings.TrimSpace(m[1])
	if name == "" {
		return qoderCall{}, false
	}
	args := map[string]any{}
	for _, pm := range qoderArg.FindAllStringSubmatch(m[2], -1) {
		key := strings.TrimSpace(pm[1])
		if key == "" {
			continue
		}
		// A value is taken as it stands: nothing is told to escape them.
		val := strings.TrimSpace(pm[2])
		var dec any
		if json.Unmarshal([]byte(val), &dec) != nil {
			dec = val
		}
		args[key] = dec
	}
	b, err := json.Marshal(args)
	if err != nil {
		return qoderCall{}, false
	}
	return qoderCall{name: name, args: string(b)}, true
}
