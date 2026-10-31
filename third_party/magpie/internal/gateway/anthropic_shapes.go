package gateway

import (
	"bytes"
	"encoding/json"
	"regexp"
	"strings"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
)

// Claude Code (2.1.288) asks Anthropic's API for what its newest models
// take and a relay may not know yet (Keenc on Discord: CHCP and a Vertex
// relay answered 400 for each):
//
//   - thinking.display "updates" (and "highlights"), where a relay that
//     checks the field takes only "summarized" and "omitted": "thinking.
//     adaptive.display: Input should be 'summarized', 'omitted'";
//   - the effort of each turn, as output_config on a {role: "system"}
//     message inside messages: "messages.1.output_config: Extra inputs are
//     not permitted". OpenRouter's Anthropic endpoint takes it as a
//     configuration_update and refuses that on DeepSeek and GLM:
//     "Mid-conversation reasoning effort (configuration_update) is not
//     supported on z-ai/glm-5.3-flash-20260826" (#3696).
//
// Claude Code mends either after the 400, once per conversation, sending
// the request again without it; every new conversation asks again, and a
// relay's refusal is an error in magpie's log each time. magpie mends it
// itself instead: the provider that refused is asked again at once without
// it, and is not sent it again while magpie runs. A provider that takes
// them (Anthropic's own API, a relay passing them on) is sent them as the
// agent wrote them.
//
// Command Code's Provider API turns away a {role: "system"} message inside
// messages at all, the empty one that only carries a turn's effort too:
// "Invalid input at messages.1.role" (#1407). A provider that refuses one is
// sent its system messages' text at the end of the top-level system, and an
// empty one left out.
//
// Copilot's Messages API turns away auto mode's `safeguards` (automode.go)
// on its Claude models: 400 "safeguards: Extra inputs are not permitted"
// (#1756, claude-opus-5.5). A provider that refuses it is sent none, and
// Claude Code, finding no safeguard_results, reviews its actions locally.
//
// Claude Code's deferred tools (ToolSearch, MCP tools that come and go)
// put blocks of their own in a message: {"type":"tool_addition","tool":
// {"type":"tool_reference","name":…}} (or a tool_definition) when a tool
// is loaded, a tool_removal when one goes, and a tool_reference. DeepSeek's
// Anthropic endpoint takes a tool_addition naming a tool, but not one
// defining it, nor a tool_removal or a message's tool_reference: 422
// "messages[1].content: unknown variant `tool_removal`". A provider that
// refuses one is sent none of them (withoutDeferredTools); Kimi, whose 400
// names nothing, is never sent them (forwardOnce, #2042). OpenCode's
// DeepSeek route also names nothing: 422 "Endpoint is unavailable.".
//
// ToolSearch's answer is a tool_result whose content is tool_reference
// blocks alone. OpenDesign's Anthropic endpoint can't read one: 422 "Failed
// to deserialize the JSON body into the target type: messages[4]: missing
// field `content`" (#2124), while it takes the same result with a text block
// beside the references. A provider that refuses one is sent each such
// result with a text block first, saying which tools are loaded, in the
// words a translated route tells a model (stringOrText); the references and
// the tool_use/tool_result pairing stay.

// The refusals, remembered in unfit under the provider's id as betas are.
const (
	shapeDisplayNew = "shape:thinking.display=new" // a display other than summarized or omitted
	shapeDisplay    = "shape:thinking.display"     // display at all
	shapeTurnEffort = "shape:messages[].output_config"
	shapeTurnSystem = "shape:messages[].role=system"
	shapeSafeguards = "shape:safeguards"
	shapeDeferred   = "shape:messages[].content[].tool_addition"
	shapeRefsOnly   = "shape:tool_result.content=tool_reference"
)

var (
	displayValueRefused = regexp.MustCompile(`thinking\.(?:adaptive|enabled)\.display:? Input should be`)
	displayRefused      = regexp.MustCompile(`thinking\.(?:adaptive|enabled)\.display:? Extra inputs are not permitted`)
	turnEffortRefused   = regexp.MustCompile(`messages\.\d+\.output_config:? Extra inputs are not permitted|Mid-conversation reasoning effort \(configuration_update\) is not supported`)
	turnRoleRefused     = regexp.MustCompile(`messages\.(\d+)\.role\b`)
	safeguardsRefused   = regexp.MustCompile(`\bsafeguards:? Extra inputs are not permitted`)
	deferredRefused     = regexp.MustCompile("messages\\[\\d+\\]\\.content: unknown variant `(?:tool_addition|tool_removal|tool_definition|tool_reference)`")
	refsOnlyRefused     = regexp.MustCompile("missing field `content`")
)

// knownDisplay is a thinking.display every Anthropic endpoint has taken.
func knownDisplay(d string) bool { return d == "summarized" || d == "omitted" }

// refuseShapes notes what of body the provider turned away in errBody, a
// 400's; true when body has something to leave out on the next try.
func (s *Server) refuseShapes(p provider.Provider, errBody, body []byte) bool {
	fresh := false
	mark := func(key string) {
		if s.fits(p.ID, key, provider.Anthropic) {
			s.markUnfit(p.ID, key, provider.Anthropic)
			fresh = true
		}
	}
	// the display as forwardOnce sends it: a budget turned adaptive asks one
	if d := gjson.GetBytes(adaptiveThinking(body), "thinking.display"); d.Exists() {
		if displayRefused.Match(errBody) {
			mark(shapeDisplay)
		} else if displayValueRefused.Match(errBody) && !knownDisplay(d.String()) {
			mark(shapeDisplayNew)
		}
	}
	if turnEffortRefused.Match(errBody) && hasTurnEffort(body) {
		mark(shapeTurnEffort)
	}
	if safeguardsRefused.Match(errBody) && gjson.GetBytes(body, "safeguards").Exists() {
		mark(shapeSafeguards)
	}
	if deferredRefused.Match(errBody) && hasDeferredTools(body) {
		mark(shapeDeferred)
	}
	if refsOnlyRefused.Match(errBody) && hasRefsOnlyResult(body) {
		mark(shapeRefsOnly)
	}
	// a role refused at a message that is a system one, counted in the
	// messages as they were sent; a refusal naming a user or assistant
	// message is over something else
	if m := turnRoleRefused.FindSubmatch(errBody); m != nil {
		sent := s.withoutRefusedShapes(p, body)
		if gjson.GetBytes(sent, "messages."+string(m[1])+".role").String() == "system" {
			mark(shapeTurnSystem)
		}
	}
	return fresh
}

// withoutRefusedShapes is body as the provider takes it, leaving out what
// it has refused before.
func (s *Server) withoutRefusedShapes(p provider.Provider, body []byte) []byte {
	if !s.fits(p.ID, shapeDeferred, provider.Anthropic) {
		body = withoutDeferredTools(body)
	}
	if !s.fits(p.ID, shapeRefsOnly, provider.Anthropic) {
		body = withRefsSaid(body)
	}
	if !s.fits(p.ID, shapeTurnSystem, provider.Anthropic) {
		body = withSystemOnTop(body)
	}
	if !s.fits(p.ID, shapeSafeguards, provider.Anthropic) && gjson.GetBytes(body, "safeguards").Exists() {
		body = withoutFields(body, "safeguards")
	}
	if !bytes.Contains(body, []byte(`"display"`)) && !bytes.Contains(body, []byte(`"output_config"`)) {
		return body
	}
	if d := gjson.GetBytes(body, "thinking.display"); d.Exists() {
		all := !s.fits(p.ID, shapeDisplay, provider.Anthropic)
		if all || !knownDisplay(d.String()) && !s.fits(p.ID, shapeDisplayNew, provider.Anthropic) {
			body = withDisplay(body, all)
		}
	}
	if !s.fits(p.ID, shapeTurnEffort, provider.Anthropic) {
		body = withoutTurnEffort(body)
	}
	return body
}

// withDisplay is body with thinking.display left out, or (not all) only a
// value beyond summarized and omitted: "highlights" as "summarized", the
// thinking still shown; "updates", or any other, left out, as Claude Code
// itself sends it again.
func withDisplay(body []byte, all bool) []byte {
	var v struct {
		Thinking map[string]any `json:"thinking"`
	}
	if json.Unmarshal(body, &v) != nil || v.Thinking == nil {
		return body
	}
	d, _ := v.Thinking["display"].(string)
	switch {
	case all:
		delete(v.Thinking, "display")
	case knownDisplay(d):
		return body
	case d == "highlights":
		v.Thinking["display"] = "summarized"
	default:
		delete(v.Thinking, "display")
	}
	return withFields(body, map[string]any{"thinking": v.Thinking})
}

// hasTurnEffort is a request with an output_config on one of its messages.
func hasTurnEffort(body []byte) bool {
	if !bytes.Contains(body, []byte(`"output_config"`)) {
		return false
	}
	found := false
	getIn(body, "messages").ForEach(func(_, m gjson.Result) bool {
		found = m.Get("output_config").Exists()
		return !found
	})
	return found
}

// withoutTurnEffort is body without the per-turn effort on its messages, as
// Claude Code sends it once refused: a system message that carried nothing
// else goes with it. The request's own output_config, the effort of the
// turn now asked, stays.
func withoutTurnEffort(body []byte) []byte {
	if !hasTurnEffort(body) {
		return body
	}
	var v struct {
		Messages []json.RawMessage `json:"messages"`
	}
	if json.Unmarshal(body, &v) != nil {
		return body
	}
	out := make([]json.RawMessage, 0, len(v.Messages))
	for _, raw := range v.Messages {
		var m map[string]json.RawMessage
		if json.Unmarshal(raw, &m) != nil {
			out = append(out, raw)
			continue
		}
		if _, ok := m["output_config"]; !ok {
			out = append(out, raw)
			continue
		}
		delete(m, "output_config")
		if r := gjson.GetBytes(raw, "role").String(); r == "system" && emptyContent(gjson.GetBytes(raw, "content")) {
			continue
		}
		b, err := marshalPlain(m)
		if err != nil {
			return body
		}
		out = append(out, b)
	}
	return withFields(body, map[string]any{"messages": out})
}

// withSystemOnTop is body with no system message inside messages: the text
// of each goes, in order, at the end of the top-level system, and one with
// none (a turn's effort alone) is left out. The other messages stay as they
// were sent.
func withSystemOnTop(body []byte) []byte {
	if !bytes.Contains(body, []byte(`"system"`)) {
		return body
	}
	var v struct {
		System   json.RawMessage   `json:"system"`
		Messages []json.RawMessage `json:"messages"`
	}
	if json.Unmarshal(body, &v) != nil {
		return body
	}
	var moved []any // text blocks for the top-level system
	out := make([]json.RawMessage, 0, len(v.Messages))
	for _, raw := range v.Messages {
		if gjson.GetBytes(raw, "role").String() != "system" {
			out = append(out, raw)
			continue
		}
		c := gjson.GetBytes(raw, "content")
		switch {
		case c.Type == gjson.String && c.String() != "":
			moved = append(moved, map[string]any{"type": "text", "text": c.String()})
		case c.IsArray():
			for _, b := range c.Array() {
				if b.Get("type").String() == "text" && b.Get("text").String() != "" {
					moved = append(moved, json.RawMessage(b.Raw))
				}
			}
		}
	}
	if len(out) == len(v.Messages) {
		return body
	}
	fields := map[string]any{"messages": out}
	if len(moved) > 0 {
		var top []any
		var s string
		var blocks []json.RawMessage
		switch {
		case json.Unmarshal(v.System, &s) == nil:
			if s != "" {
				top = append(top, map[string]any{"type": "text", "text": s})
			}
		case json.Unmarshal(v.System, &blocks) == nil:
			for _, b := range blocks {
				top = append(top, b)
			}
		}
		fields["system"] = append(top, moved...)
	}
	return withFields(body, fields)
}

// emptyContent is a message's content with nothing in it: missing, "", or
// [] (or text blocks with no text).
func emptyContent(c gjson.Result) bool {
	switch {
	case !c.Exists() || c.Type == gjson.Null:
		return true
	case c.Type == gjson.String:
		return c.String() == ""
	case c.IsArray():
		empty := true
		c.ForEach(func(_, b gjson.Result) bool {
			empty = b.Get("type").String() == "text" && b.Get("text").String() == ""
			return empty
		})
		return empty
	}
	return false
}

// deferredBlock is a message block of Claude Code's deferred tools.
func deferredBlock(typ string) bool {
	return typ == "tool_addition" || typ == "tool_removal" || typ == "tool_reference"
}

// hasDeferredTools is a request with a deferred-tool block in a message's
// content. A tool_reference inside a tool_result (ToolSearch's answer) is
// not one.
func hasDeferredTools(body []byte) bool {
	if !bytes.Contains(body, []byte(`"tool_addition"`)) && !bytes.Contains(body, []byte(`"tool_removal"`)) && !bytes.Contains(body, []byte(`"tool_reference"`)) {
		return false
	}
	found := false
	getIn(body, "messages").ForEach(func(_, m gjson.Result) bool {
		m.Get("content").ForEach(func(_, b gjson.Result) bool {
			found = deferredBlock(b.Get("type").String())
			return !found
		})
		return !found
	})
	return found
}

// withoutDeferredTools is body without the deferred-tool blocks in its
// messages, and without a message they leave empty. Definitions carried
// only inline join the request's own tools list; a definition already on
// that list wins. A tool_result's tool_reference stays: it is
// the tool's answer, and Kimi and DeepSeek take it. A body with none is
// sent as it was.
func withoutDeferredTools(body []byte) []byte {
	if !hasDeferredTools(body) {
		return body
	}
	var v struct {
		Messages []json.RawMessage `json:"messages"`
		Tools    []json.RawMessage `json:"tools"`
	}
	if json.Unmarshal(body, &v) != nil {
		return body
	}
	have := make(map[string]bool, len(v.Tools))
	for _, tool := range v.Tools {
		have[gjson.GetBytes(tool, "name").String()] = true
	}
	tools := v.Tools
	out := make([]json.RawMessage, 0, len(v.Messages))
	for _, raw := range v.Messages {
		c := gjson.GetBytes(raw, "content")
		if !c.IsArray() {
			out = append(out, raw)
			continue
		}
		var kept []json.RawMessage
		for _, b := range c.Array() {
			if b.Get("type").String() == "tool_addition" && b.Get("tool.type").String() == "tool_definition" {
				def := b.Get("tool.definition")
				if name := def.Get("name"); def.IsObject() && name.Type == gjson.String && name.String() != "" && !have[name.String()] {
					tools = append(tools, json.RawMessage(def.Raw))
					have[name.String()] = true
				}
			}
			if !deferredBlock(b.Get("type").String()) {
				kept = append(kept, json.RawMessage(b.Raw))
			}
		}
		if len(kept) == len(c.Array()) {
			out = append(out, raw)
			continue
		}
		if len(kept) == 0 {
			continue // it said only that tools came or went
		}
		var m map[string]json.RawMessage
		if json.Unmarshal(raw, &m) != nil {
			return body
		}
		content, err := marshalPlain(kept)
		if err != nil {
			return body
		}
		m["content"] = content
		b, err := marshalPlain(m)
		if err != nil {
			return body
		}
		out = append(out, b)
	}
	fields := map[string]any{"messages": out}
	if len(tools) != len(v.Tools) {
		fields["tools"] = tools
	}
	return withFields(body, fields)
}

// toolLoadedText is what a model is told of a tool ToolSearch loaded, where
// the tool_reference that names it can't go as it is.
func toolLoadedText(name string) string {
	return "Tool " + name + " is loaded and can be called now."
}

// toolRefs are the tools a tool_result's content names in tool_reference
// blocks, as Claude Code's ToolSearch answers, in their order.
func toolRefs(content json.RawMessage) []string {
	if !bytes.Contains(content, []byte(`"tool_reference"`)) {
		return nil
	}
	var names []string
	gjson.ParseBytes(content).ForEach(func(_, b gjson.Result) bool {
		if n := b.Get("tool_name").String(); b.Get("type").String() == "tool_reference" && n != "" {
			names = append(names, n)
		}
		return true
	})
	return names
}

// refsOnly is a tool_result's content that is tool_reference blocks and
// nothing else, as ToolSearch answers.
func refsOnly(c gjson.Result) bool {
	if !c.IsArray() || len(c.Array()) == 0 {
		return false
	}
	only := true
	c.ForEach(func(_, b gjson.Result) bool {
		only = b.Get("type").String() == "tool_reference"
		return only
	})
	return only
}

// hasRefsOnlyResult is a request with a tool_result holding only
// tool_reference blocks.
func hasRefsOnlyResult(body []byte) bool {
	if !bytes.Contains(body, []byte(`"tool_reference"`)) {
		return false
	}
	found := false
	getIn(body, "messages").ForEach(func(_, m gjson.Result) bool {
		m.Get("content").ForEach(func(_, b gjson.Result) bool {
			found = b.Get("type").String() == "tool_result" && refsOnly(b.Get("content"))
			return !found
		})
		return !found
	})
	return found
}

// withRefsSaid is body with a text block first in each tool_result that
// holds only tool_reference blocks, naming the tools they load; the
// references stay after it. A body with none is sent as it was.
func withRefsSaid(body []byte) []byte {
	if !hasRefsOnlyResult(body) {
		return body
	}
	var v struct {
		Messages []json.RawMessage `json:"messages"`
	}
	if json.Unmarshal(body, &v) != nil {
		return body
	}
	out := make([]json.RawMessage, 0, len(v.Messages))
	for _, raw := range v.Messages {
		c := gjson.GetBytes(raw, "content")
		if !c.IsArray() {
			out = append(out, raw)
			continue
		}
		changed := false
		blocks := make([]json.RawMessage, 0, len(c.Array()))
		for _, b := range c.Array() {
			rc := b.Get("content")
			if b.Get("type").String() != "tool_result" || !refsOnly(rc) {
				blocks = append(blocks, json.RawMessage(b.Raw))
				continue
			}
			var said []string
			for _, r := range rc.Array() {
				said = append(said, toolLoadedText(r.Get("tool_name").String()))
			}
			content := []any{map[string]any{"type": "text", "text": strings.Join(said, "\n")}}
			for _, r := range rc.Array() {
				content = append(content, json.RawMessage(r.Raw))
			}
			var m map[string]json.RawMessage
			if json.Unmarshal([]byte(b.Raw), &m) != nil {
				return body
			}
			cb, err := marshalPlain(content)
			if err != nil {
				return body
			}
			m["content"] = cb
			nb, err := marshalPlain(m)
			if err != nil {
				return body
			}
			blocks = append(blocks, nb)
			changed = true
		}
		if !changed {
			out = append(out, raw)
			continue
		}
		var m map[string]json.RawMessage
		if json.Unmarshal(raw, &m) != nil {
			return body
		}
		cb, err := marshalPlain(blocks)
		if err != nil {
			return body
		}
		m["content"] = cb
		nb, err := marshalPlain(m)
		if err != nil {
			return body
		}
		out = append(out, nb)
	}
	return withFields(body, map[string]any{"messages": out})
}
