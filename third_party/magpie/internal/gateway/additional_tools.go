package gateway

import (
	"bytes"
	"encoding/json"
	"strings"

	"github.com/yetone/magpie/internal/provider"
)

// Codex's Responses Lite, for the models its catalog marks so (the ChatGPT
// backend's gpt-6 and gpt-5.6 among them), sends no tools and no
// instructions: its tools go as the first input item, an additional_tools
// one, and its prompt as a developer message after it. Every plain
// function and freeform tool is grouped in a namespace named "functions",
// which Codex reads as no namespace at all when a call comes back. Only
// OpenAI knows the item — xAI's backend turns the whole request away over
// it (#350: 422 "unknown item type additional_tools") — so a request going
// anywhere else has its tools in the tools, the "functions" namespace
// opened into them, as Codex sends a model that isn't Lite.

// Lite also asks for reasoning context "all_turns" (the ChatGPT backend
// turns a Lite request away without it, #534) and says it is Lite in the
// X-OpenAI-Internal-Codex-Responses-Lite header. Neither is in the
// Responses API anyone else serves. A subagent a GPT lead spawns on another
// model is sent in the lead's Lite shape, its model name alone changed
// (codex-rs builds the child's request from the lead's catalog entry), and
// OpenCode Go's DeepSeek answered it with 503 "Endpoint is unavailable"
// while the same subagent under a DeepSeek lead, not Lite, was answered
// (TangForX on X). So a request leaving Lite has both taken off too
// (fromLite, liteHeaderKept).

// liteHeader says a Codex request is Responses Lite.
const liteHeader = "X-OpenAI-Internal-Codex-Responses-Lite"

// fromLite is a Responses request Codex sent as Responses Lite in the shape
// it sends a model that isn't Lite: its additional_tools in its tools
// (liftAdditionalTools) and no reasoning context. A request that isn't
// Lite is returned as it was.
func fromLite(body []byte) []byte {
	return withoutReasoningContext(liftAdditionalTools(body))
}

// withoutReasoningContext is body without reasoning.context, the rest of
// reasoning and every other member byte for byte as they were.
func withoutReasoningContext(body []byte) []byte {
	if !bytes.Contains(body, []byte(`"context"`)) {
		return body
	}
	r, found, ok := topMember(body, "reasoning")
	if !found || !ok {
		return body
	}
	var m map[string]json.RawMessage
	if json.Unmarshal(r, &m) != nil || m == nil {
		return body
	}
	if _, has := m["context"]; !has {
		return body
	}
	delete(m, "context")
	nb, err := json.Marshal(m)
	if err != nil {
		return body
	}
	return withMembers(body, map[string][]byte{"reasoning": nb})
}

// liteKept says a Responses request to p goes in Codex's Lite shape as it
// was sent: to OpenAI, to a ChatGPT account, and to a relay that takes
// Codex's requests as Codex sent them (Provider.CodexAsSent, #1758).
func liteKept(p provider.Provider, to provider.Protocol, codex bool) bool {
	if to != provider.Responses {
		return false
	}
	return p.Account != nil && p.Account.Agent == "codex" ||
		strings.HasSuffix(p.Host(), "openai.com") ||
		p.CodexAsSent && p.Account == nil && codex
}

// liteNamespace is the namespace Responses Lite groups Codex's own
// functions in (codex_protocol::DEFAULT_FUNCTION_NAMESPACE).
const liteNamespace = "functions"

// liftAdditionalTools is a Responses request with its additional_tools items
// moved into its tools, or the request as it was when it has none.
func liftAdditionalTools(body []byte) []byte {
	if !bytes.Contains(body, []byte(`"additional_tools"`)) {
		return body
	}
	dec := json.NewDecoder(bytes.NewReader(body))
	dec.UseNumber()
	var m map[string]any
	if dec.Decode(&m) != nil || m == nil {
		return body
	}
	items, ok := m["input"].([]any)
	if !ok {
		return body
	}
	tools, _ := m["tools"].([]any)
	have := toolsByName(tools)
	kept := items[:0:0]
	lifted := false
	for _, it := range items {
		im, _ := it.(map[string]any)
		if im["type"] != "additional_tools" {
			kept = append(kept, it)
			continue
		}
		lifted = true
		more, _ := im["tools"].([]any)
		for _, t := range more {
			tm, _ := t.(map[string]any)
			if tm == nil {
				continue
			}
			if tm["type"] == "namespace" && tm["name"] == liteNamespace {
				nested, _ := tm["tools"].([]any)
				for _, n := range nested {
					if nm, _ := n.(map[string]any); nm != nil {
						tools = addTool(tools, have, nm)
					}
				}
				continue
			}
			tools = addTool(tools, have, tm)
		}
	}
	if !lifted {
		return body
	}
	// a call the model made to one of Codex's own tools, under the
	// namespace they were grouped in, is the plain call it now is
	for _, it := range kept {
		if im, _ := it.(map[string]any); im != nil && im["namespace"] == liteNamespace {
			delete(im, "namespace")
		}
	}
	m["input"] = kept
	if len(tools) > 0 {
		m["tools"] = tools
	}
	out, err := marshalPlain(m)
	if err != nil {
		return body
	}
	return out
}

// toolsByName is a request's tools by their names, a hosted tool without
// one (web_search) by its type.
func toolsByName(tools []any) map[string]map[string]any {
	have := map[string]map[string]any{}
	for _, t := range tools {
		if tm, _ := t.(map[string]any); tm != nil {
			have[toolKey(tm)] = tm
		}
	}
	return have
}

func toolKey(tm map[string]any) string {
	if n, _ := tm["name"].(string); n != "" {
		return n
	}
	ty, _ := tm["type"].(string)
	return "#" + ty
}

// addTool is tools with tm among them: a tool added once, and a namespace
// given again adding to the one there the tools it didn't have.
func addTool(tools []any, have map[string]map[string]any, tm map[string]any) []any {
	k := toolKey(tm)
	old := have[k]
	if old == nil {
		have[k] = tm
		return append(tools, tm)
	}
	if old["type"] != "namespace" || tm["type"] != "namespace" {
		return tools
	}
	nested, _ := old["tools"].([]any)
	more, _ := tm["tools"].([]any)
	for _, nt := range more {
		nm, _ := nt.(map[string]any)
		if nm != nil && !containsNamed(nested, nm["name"]) {
			nested = append(nested, nm)
		}
	}
	old["tools"] = nested
	return tools
}

func containsNamed(tools []any, name any) bool {
	for _, o := range tools {
		if om, _ := o.(map[string]any); om["name"] == name {
			return true
		}
	}
	return false
}
