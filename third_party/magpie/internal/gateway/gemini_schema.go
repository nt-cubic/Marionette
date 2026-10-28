package gateway

import (
	"strings"

	"github.com/tidwall/gjson"
)

// geminiToolTypes is body with a type written into each schema of its
// tools' that has none, for a Gemini model whoever serves it. Vertex AI
// takes a function declaration's parameters as its own Schema object, whose
// type is required, and turns the whole request away over one schema
// without it, though the tool is never called: 400 "Unable to submit
// request because `mcp__cua-driver__drag` functionDeclaration
// `parameters.target` schema didn't specify the schema type field."
// (kthink on Discord: dsh with cua-driver's MCP tools, Command Code's
// google/gemini-3.8-flash, which its gateway serves on Vertex). Whoever
// relays a Chat, Responses or Messages body to Vertex — a plugin's
// gateway, a router, a relay — turns the schema into Vertex's, and an
// untyped node stays untyped, so it is keyed on the model, not the
// provider.
//
// Each schema with no type is given the one its values (enum, const,
// default), keywords (properties: object, items: array, pattern:
// string …; an object's or array's before its values) or branches (anyOf, oneOf, allOf, a local $ref) say, null
// left out: Vertex has one type a node, nullable apart. A node whose
// branches have more than one type, or a $ref or union whose types can't
// be told, is left as it came: no one type is right for it. One that says
// nothing ({} or only a description) is a string, the value that carries
// anything; a tool's root is an object. "not" and "if" are left alone: a
// type there would turn the rule round. The type is written in place,
// after the schema's brace, so the rest of the body and a cache's prefix
// stay as they were; every other model's body goes as it came.
//
// Google's own Gemini API and Code Assist are sent parametersJsonSchema,
// which takes JSON Schema whole, and Antigravity its own cut-down schema
// (plainSchema), so a Gemini-shaped body isn't walked.
func geminiToolTypes(body []byte) []byte {
	if !geminiNamed(gjson.GetBytes(body, "model").String()) {
		return body
	}
	tools := gjson.GetBytes(body, "tools")
	if !tools.IsArray() || tools.Index <= 0 {
		return body
	}
	var ins []typeInsert
	tools.ForEach(func(_, tool gjson.Result) bool {
		for _, at := range []string{"function.parameters", "parameters", "input_schema"} {
			if root := tool.Get(at); root.IsObject() {
				collectGeminiUntyped(body, root, root, true, &ins)
			}
		}
		return true
	})
	return withTypes(body, ins)
}

// geminiNamed is whether a model id names a Gemini model: gemini-…, or
// a relay's or router's vendor/gemini-… (google/gemini-3.8-flash,
// vertex_ai/gemini-…).
func geminiNamed(id string) bool {
	if i := strings.LastIndexByte(id, '/'); i >= 0 {
		id = id[i+1:]
	}
	return strings.HasPrefix(strings.ToLower(id), "gemini")
}

// collectGeminiUntyped adds, for node and each schema under it that has no
// type, where it opens and the type Vertex is told.
func collectGeminiUntyped(body []byte, root, node gjson.Result, isRoot bool, ins *[]typeInsert) {
	switch {
	case node.IsArray():
		node.ForEach(func(_, v gjson.Result) bool {
			collectGeminiUntyped(body, root, v, false, ins)
			return true
		})
	case node.IsObject():
		if !node.Get("type").Exists() && node.Index > 0 && node.Index < len(body) && body[node.Index] == '{' {
			if t := geminiSchemaType(root, node, isRoot); t != "" {
				*ins = append(*ins, typeInsert{node.Index, `"` + t + `"`})
			}
		}
		node.ForEach(func(key, v gjson.Result) bool {
			switch key.String() {
			case "properties", "$defs", "definitions", "dependentSchemas", "patternProperties":
				v.ForEach(func(_, s gjson.Result) bool {
					collectGeminiUntyped(body, root, s, false, ins)
					return true
				})
			case "items", "additionalProperties", "contains", "propertyNames", "then", "else", "allOf", "anyOf", "oneOf", "prefixItems", "unevaluatedProperties", "unevaluatedItems", "contentSchema":
				collectGeminiUntyped(body, root, v, false, ins)
			}
			return true
		})
	}
}

// geminiSchemaType is the one type an untyped schema s is given for
// Vertex, "" when it is left as it came.
func geminiSchemaType(root, s gjson.Result, isRoot bool) string {
	if t := keywordType(s); t == "object" || t == "array" {
		// properties or items say it before its values do: a schema
		// with items and a string enum given "string" went to Google
		// with items on a string, and Antigravity turned the whole turn
		// away ("properties[display].items: field predicate failed:
		// $type == Type.ARRAY", ferock on Discord)
		return t
	}
	vals := s.Get("enum").Array()
	if c := s.Get("const"); c.Exists() {
		vals = []gjson.Result{c}
	}
	if s.Get("enum").IsArray() || s.Get("const").Exists() {
		var rest []gjson.Result
		for _, v := range vals {
			if v.Type != gjson.Null {
				rest = append(rest, v)
			}
		}
		return valuesType(rest)
	}
	ts := schemaTypes(root, s, 0)
	delete(ts, "null")
	if ts["integer"] && ts["number"] {
		delete(ts, "integer")
	}
	if len(ts) == 1 {
		for t := range ts {
			return t
		}
	}
	if len(ts) > 1 {
		return ""
	}
	if isRoot {
		return "object"
	}
	for _, k := range []string{`\$ref`, "anyOf", "oneOf", "allOf"} {
		if s.Get(k).Exists() {
			return ""
		}
	}
	if d := s.Get("default"); d.Exists() && d.Type != gjson.Null {
		switch {
		case d.IsObject():
			return "object"
		case d.IsArray():
			return "array"
		}
		if t := valuesType([]gjson.Result{d}); t != "" {
			return t
		}
	}
	return "string"
}
