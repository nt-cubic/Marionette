package gateway

import (
	"encoding/json"
	"fmt"
	"reflect"
	"testing"
)

// devinGeminiRefused are tool schemas Devin's gemini-4-argon refused on a
// real account, each on its own, with invalid_argument "There is an issue
// with this request, please try a different model" (7eleven on Discord,
// ZCode). Each went through once rewritten as devinGeminiSchema does;
// claude-sonnet-4-6 and swe-2 took every one as it was.
var devinGeminiRefused = map[string]string{
	"typeList":     `{"type":"object","properties":{"a":{"type":["string","null"]}}}`,
	"typeListOne":  `{"type":"object","properties":{"a":{"type":["string"]}}}`,
	"typeListTwo":  `{"type":"object","properties":{"a":{"type":["string","number"]}}}`,
	"noItems":      `{"type":"object","properties":{"a":{"type":"array"}}}`,
	"itemsTrue":    `{"type":"object","properties":{"a":{"type":"array","items":true}}}`,
	"itemsList":    `{"type":"object","properties":{"a":{"type":"array","items":[{"type":"string"}]}}}`,
	"deepNoItems":  `{"type":"object","properties":{"a":{"type":"object","properties":{"b":{"type":"array"}}}}}`,
	"unionNoItems": `{"type":"object","properties":{"a":{"anyOf":[{"type":"array"},{"type":"null"}]}}}`,
	"propsNoType":  `{"type":"object","properties":{"a":{"properties":{"b":{"type":"string"}}}}}`,
	"enumNoType":   `{"type":"object","properties":{"a":{"enum":["x","y"]}}}`,
	"rootNoType":   `{"properties":{"a":{"type":"string"}}}`,
	"emptyRoot":    `{}`,
}

// devinGeminiFaults is what in s Devin's Gemini models refuse.
func devinGeminiFaults(s any, at string) []string {
	m, ok := s.(map[string]any)
	if !ok {
		return nil
	}
	var out []string
	if _, list := m["type"].([]any); list {
		out = append(out, at+" list type")
	}
	_, typed := m["type"]
	if !typed && (m["properties"] != nil || m["enum"] != nil) {
		out = append(out, at+" no type")
	}
	if m["type"] == "array" {
		if _, ok := m["items"].(map[string]any); !ok {
			out = append(out, at+" items")
		}
	}
	props, _ := m["properties"].(map[string]any)
	for k, v := range props {
		out = append(out, devinGeminiFaults(v, at+"."+k)...)
	}
	for _, k := range []string{"anyOf", "oneOf", "allOf"} {
		l, _ := m[k].([]any)
		for i, v := range l {
			out = append(out, devinGeminiFaults(v, fmt.Sprintf("%s.%s[%d]", at, k, i))...)
		}
	}
	return append(out, devinGeminiFaults(m["items"], at+"[]")...)
}

func devinSentSchema(t *testing.T, schema, uid string) string {
	t.Helper()
	d := decodeDevinRequest(t, buildDevin(&Request{
		Tools:    []Tool{{Name: "t", Description: "t", Schema: json.RawMessage(schema)}},
		Messages: []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: "ok"}}}},
	}, uid, "k"))
	if len(d.schemas) != 1 {
		t.Fatalf("schemas %q", d.schemas)
	}
	return d.schemas[0]
}

func TestBuildDevinGeminiSchema(t *testing.T) {
	for name, schema := range devinGeminiRefused {
		got := devinSentSchema(t, schema, "gemini-4-argon-medium")
		var m map[string]any
		if err := json.Unmarshal([]byte(got), &m); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if m["type"] != "object" {
			t.Errorf("%s: root not an object: %s", name, got)
		}
		if f := devinGeminiFaults(m, "$"); len(f) > 0 {
			t.Errorf("%s: Gemini refuses %v in %s", name, f, got)
		}
		// Claude models and swe-2 take every one as it came
		if name == "rootNoType" || name == "emptyRoot" {
			continue
		}
		for _, uid := range []string{"claude-opus-5-5-high", "swe-2-high"} {
			if got := devinSentSchema(t, schema, uid); got != schema {
				t.Errorf("%s: %s got %s", name, uid, got)
			}
		}
	}
}

func TestDevinGeminiSchemaKeepsWhatItAllowed(t *testing.T) {
	for in, want := range map[string]string{
		`{"type":["string","null"],"description":"d"}`: `{"description":"d","nullable":true,"type":"string"}`,
		`{"type":["string","number","null"]}`:          `{"anyOf":[{"type":"string"},{"type":"number"}],"nullable":true}`,
		`{"type":["array","null"]}`:                    `{"items":{},"nullable":true,"type":"array"}`,
		`{"type":"array","items":[{"type":"string"}]}`: `{"items":{"type":"string"},"type":"array"}`,
		`{"enum":[1,2]}`:                               `{"enum":[1,2],"type":"integer"}`,
		`{"enum":["x",null]}`:                          `{"enum":["x"],"nullable":true,"type":"string"}`,
	} {
		// a schema at a property, as a tool's parameters hold it
		got := devinGeminiSchema(json.RawMessage(`{"type":"object","properties":{"a":` + in + `}}`))
		var g, w map[string]any
		json.Unmarshal(got, &g)
		json.Unmarshal([]byte(want), &w)
		if a := g["properties"].(map[string]any)["a"]; !reflect.DeepEqual(a, w) {
			t.Errorf("%s: got %v, want %v", in, a, w)
		}
	}
	// a schema Gemini takes is sent byte for byte
	ok := `{"type":"object","properties":{"a":{"anyOf":[{"type":"string"},{"type":"null"}]},"b":{"type":"array","items":{"type":"string","enum":["x"]}},"c":{"$ref":"#/$defs/x"},"d":{"const":"k"}},"$defs":{"x":{"type":"object","properties":{"y":{"type":"integer"}}}},"required":["a"]}`
	if got := devinSentSchema(t, ok, "gemini-4-argon-medium"); got != ok {
		t.Errorf("a schema Gemini takes was changed: %s", got)
	}
}
