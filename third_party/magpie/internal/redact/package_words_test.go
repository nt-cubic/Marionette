package redact

import (
	"encoding/json"
	"strings"
	"testing"
)

// dniray on Discord, v0.1.1976 after 300b7eee0: drizzle-orm still went
// out as {{SECRET_o2tu7vej}}, now in a tsup config in a Markdown fence,
// where nothing says a package is loaded or pinned:
// external: [/@nestjs/, 'reflect-metadata', 'rxjs', 'mysql2', 'drizzle-orm'].
// The places a package's name is written can't all be listed. A value of
// words in lower case alone has no randomness for a key to be made of:
// the model's secret of that shape is a secret only where it is assigned
// to a password's, a token's or a key's name.
const dnirayBlock = "```ts\nexport default defineConfig({\n  entry: ['src/main.ts'],\n  format: [\n  'cjs',\n  clean: true,\n  noExternal: [/@nexus\\/shared/],\n  external: [/@nestjs/, 'reflect-metadata', 'rxjs', 'mysql2', 'drizzle-orm'],\n  sourcemap: true,\n})\n```\n"

// scoredModel finds each of values as a secret at a score, as the real
// models give one (a fake's 0 counts as a rule's find).
func scoredModel(values ...string) Model {
	f := &fakeModel{values: map[string]string{}}
	for _, v := range values {
		f.values[v] = "secret"
	}
	return func(texts []string) [][]Found {
		out := f.find(texts)
		for i := range out {
			for j := range out[i] {
				out[i][j].Score = 0.93
			}
		}
		return out
	}
}

func TestModelWordsArentSecrets(t *testing.T) {
	const line = "  external: [/@nestjs/, 'reflect-metadata', 'rxjs', 'mysql2', 'drizzle-orm'],"
	o := Options{Secrets: true, Personal: true, Model: scoredModel("drizzle-orm", "reflect-metadata")}
	// in a user's message, as Markdown
	body, _ := json.Marshal(map[string]any{"model": "m", "messages": []any{
		map[string]any{"role": "user", "content": "tsup.config.ts:\n\n" + dnirayBlock + "\nwhy is drizzle-orm bundled?"},
	}})
	out, _ := MaskJSON(body, o)
	if s := string(out); strings.Contains(s, "{{SECRET_") || !strings.Contains(s, line) {
		t.Errorf("in a message: %s", out)
	}
	// in a Read's tool_result, with Claude Code's line numbers
	var read strings.Builder
	for i, l := range strings.Split(strings.TrimSuffix(dnirayBlock, "\n"), "\n") {
		read.WriteString(strings.Repeat(" ", 5) + string(rune('1'+i%9)) + "→" + l + "\n")
	}
	body, _ = json.Marshal(map[string]any{"model": "m", "messages": []any{
		map[string]any{"role": "assistant", "content": []any{map[string]any{"type": "tool_use", "id": "toolu_01", "name": "Read", "input": map[string]any{"file_path": "/w/docs/build.md"}}}},
		map[string]any{"role": "user", "content": []any{map[string]any{"type": "tool_result", "tool_use_id": "toolu_01", "content": read.String()}}},
	}})
	if out, _ := MaskJSON(body, o); strings.Contains(string(out), "{{SECRET_") {
		t.Errorf("in a tool_result: %s", out)
	}
	// prose and a name alone
	for _, s := range []string{"we keep drizzle-orm external", "drizzle-orm", "see reflect-metadata."} {
		if out, n := Mask(s, o); n != 0 {
			t.Errorf("%q: %q", s, out)
		}
	}
}

// A value the model took for a secret before this, remembered as known, is
// masked again only where it is assigned to a secret's name: what a user's
// running magpie had learned goes as written in the next request, the
// model finding nothing in it.
func TestKnownModelWordsGoAsWritten(t *testing.T) {
	// as an older magpie remembered it: masked as the model's, at its score
	p := placeholder("SECRET", "drizzle-orm")
	placeholder("SECRET", "reflect-metadata")
	for _, v := range []string{"drizzle-orm", "reflect-metadata"} {
		noteScores(v, []span{{0, len(v), "SECRET", ByModel, 0.93, false}})
	}
	o := Options{Secrets: true, Personal: true}
	if out, n := Mask(dnirayBlock, o); n != 0 || strings.Contains(out, p) {
		t.Errorf("as known: %q", out)
	}
	body, _ := json.Marshal(map[string]any{"messages": []any{map[string]any{"role": "user", "content": "and now\n" + dnirayBlock}}})
	o.Model = scoredModel()
	if out, _ := MaskJSON(body, o); strings.Contains(string(out), "{{SECRET_") {
		t.Errorf("as known, in a body: %s", out)
	}
	// where it is assigned to a password, it is the password still
	if out, _ := Mask("DB_PASSWORD: drizzle-orm", o); strings.Contains(out, "drizzle-orm") {
		t.Errorf("assigned: %q", out)
	}
}

// What stays a secret: words the model finds where they are assigned to a
// secret's name, any value with a digit, a capital or a symbol in it, and
// whatever a rule found, words alone too, wherever it comes again.
func TestWordSecretsStillMasked(t *testing.T) {
	for _, c := range []struct{ text, value string }{
		{"PASSWORD=opensesame", "opensesame"},
		{"{\"password\": \"letmeinplease\"}", "letmeinplease"},
		{"the wifi password is purplemonkey", "purplemonkey"},
		{"api_key: correct-horse-battery", "correct-horse-battery"},
		{"login with hunter2pass", "hunter2pass"},
		{"use Zq8vR2mT6wLp to sign in", "Zq8vR2mT6wLp"},
		{"the code is Tr0ub4dor&3xq", "Tr0ub4dor&3xq"},
	} {
		o := Options{Secrets: true, Personal: true, Model: scoredModel(c.value)}
		if out, _ := Mask(c.text, o); strings.Contains(out, c.value) {
			t.Errorf("%q left in %q", c.value, out)
		}
	}
	// a rule's find of words is masked as known wherever it comes again
	if out, _ := Mask("token 是 bluewhalesinging", Options{Secrets: true}); strings.Contains(out, "bluewhalesinging") {
		t.Fatalf("not masked by the rule: %q", out)
	}
	if out, _ := Mask("I typed bluewhalesinging there", Options{Secrets: true}); strings.Contains(out, "bluewhalesinging") {
		t.Errorf("rule's known value left: %q", out)
	}
}
