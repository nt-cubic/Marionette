package gateway

import (
	"encoding/json"
	"reflect"
	"testing"
)

// A $ref that points inside properties ("#/properties/…"), as dsh's tool
// parameters carry one, is a local JSON pointer like any other: resolved
// where it names, not written out as a bare object (#4561), and a
// self-reference stops at the target's type alone.
func TestPlainSchemaResolvesPointerIntoProperties(t *testing.T) {
	in := `{"type":"object",
	"properties":{
		"func":{"type":"object","properties":{
			"name":{"type":"string"},
			"triggers":{"type":"array","items":{
				"type":"object",
				"properties":{
					"kind":{"type":"string","enum":["command","prompt"]},
					"self":{"$ref":"#/properties/func/properties/triggers/items"}
				},
				"required":["kind"]
			}}
		}},
		"again":{"$ref":"#/properties/func/properties/triggers/items"}
	}}`
	want := `{"type":"object",
	"properties":{
		"func":{"type":"object","properties":{
			"name":{"type":"string"},
			"triggers":{"type":"array","items":{
				"type":"object",
				"properties":{
					"kind":{"type":"string","enum":["command","prompt"]},
					"self":{"type":"object"}
				},
				"required":["kind"]
			}}
		}},
		"again":{"type":"object","properties":{
			"kind":{"type":"string","enum":["command","prompt"]},
			"self":{"type":"object"}
		},"required":["kind"]}
	}}`
	var got, exp any
	if err := json.Unmarshal(plainSchema(json.RawMessage(in)), &got); err != nil {
		t.Fatal(err)
	}
	json.Unmarshal([]byte(want), &exp)
	if !reflect.DeepEqual(got, exp) {
		b, _ := json.Marshal(got)
		t.Errorf("got %s", b)
	}
}

// A self-reference is judged by whole pointer segments: a ref to
// "#/properties/a" inside /properties/ab is a ref to a sibling, resolved
// whole, not stopped at its type. And a pointer that names nothing where
// it points still finds its last segment among $defs, as it did before
// pointers were walked (#4561 review).
func TestPlainSchemaSelfRefBySegment(t *testing.T) {
	in := `{"type":"object",
	"$defs":{"Mode":{"type":"string","enum":["x","y"]}},
	"properties":{
		"a":{"type":"object","properties":{"n":{"type":"integer"}}},
		"ab":{"type":"object","properties":{
			"inner":{"$ref":"#/properties/a"},
			"mode":{"$ref":"#/components/schemas/Mode"}
		}}
	}}`
	want := `{"type":"object",
	"properties":{
		"a":{"type":"object","properties":{"n":{"type":"integer"}}},
		"ab":{"type":"object","properties":{
			"inner":{"type":"object","properties":{"n":{"type":"integer"}}},
			"mode":{"type":"string","enum":["x","y"]}
		}}
	}}`
	var got, exp any
	if err := json.Unmarshal(plainSchema(json.RawMessage(in)), &got); err != nil {
		t.Fatal(err)
	}
	json.Unmarshal([]byte(want), &exp)
	if !reflect.DeepEqual(got, exp) {
		b, _ := json.Marshal(got)
		t.Errorf("got %s", b)
	}
}

// Antigravity refuses a function declaration over any keyword outside its
// schema subset (#187: exclusiveMinimum from zcode's and dsh's tools).
func TestPlainSchemaKeepsOnlyWhatAntigravityTakes(t *testing.T) {
	in := `{"type":"object","$schema":"http://json-schema.org/draft-07/schema#","additionalProperties":false,
	"properties":{
		"limit":{"type":"integer","exclusiveMinimum":0,"maximum":100,"description":"how many"},
		"offset":{"type":"integer","exclusiveMinimum":0,"minimum":1},
		"ratio":{"type":"number","exclusiveMaximum":1},
		"url":{"type":"string","format":"uri","title":"URL","minLength":1,"pattern":"^https?://"},
		"tags":{"type":"array","items":{"type":"string","uniqueItems":true},"minItems":1,"uniqueItems":true,"contains":{"type":"string"}},
		"opts":{"type":"object","properties":{"deep":{"type":["boolean","null"],"default":false}},"minProperties":1,"dependentRequired":{"a":["b"]},"not":{"type":"null"}}
	},"required":["limit","gone"]}`
	want := `{"type":"object",
	"properties":{
		"limit":{"type":"integer","minimum":0,"maximum":100,"description":"how many"},
		"offset":{"type":"integer","minimum":1},
		"ratio":{"type":"number","maximum":1},
		"url":{"type":"string","minLength":1,"pattern":"^https?://"},
		"tags":{"type":"array","items":{"type":"string"},"minItems":1},
		"opts":{"type":"object","properties":{"deep":{"type":"boolean","nullable":true}},"minProperties":1}
	},"required":["limit"]}`
	var got, exp any
	if err := json.Unmarshal(plainSchema(json.RawMessage(in)), &got); err != nil {
		t.Fatal(err)
	}
	json.Unmarshal([]byte(want), &exp)
	if !reflect.DeepEqual(got, exp) {
		b, _ := json.Marshal(got)
		t.Errorf("got %s", b)
	}
}
