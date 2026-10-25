package redact

import (
	"strings"
	"testing"
)

// What the local model took for personal data or a secret in real agent
// requests, and isn't: each text is as a Claude Code or Codex request had
// it on 10-25, with the piece the model reported. Measured over 3 MB of
// them, the model alone masked ~1,800 such pieces; the rules 17.
func TestModelMisreadsNotMasked(t *testing.T) {
	for _, c := range []struct{ text, value, label string }{
		// the count each turn carries
		{"<total_tokens>14881209 tokens left</total_tokens>", "14881209", "account_number"},
		// run ids, hashes and versions
		{"| 6123456789 | ci.yml | success |", "6123456789", "account_number"},
		{"ba319a758 gateway: the lone tool_choice none", "ba319a758", "account_number"},
		{"Update to v0.1.1442 now", "0.1.1442", "account_number"},
		{"a screenshot of 1624x1487 pixels", "1624x1487", "private_phone"},
		// the model's and the assistant's own names
		{"You are powered by the model named Opus 5.5.", "Opus", "private_person"},
		{"Claude publishes it as an Artifact.", "Claude", "private_person"},
		{"Another Claude session holds the port.", "Another Claude", "private_person"},
		// a user name inside a path, a skill's name
		{"read /Users/yetone/workspace/projects/magpie/AGENTS.md", "yetone", "private_person"},
		{"- artifact-design: Design guidance", "artifact-design", "private_person"},
		// ids cut out of longer words
		{`"tool_use_id":"toolu_01XyZabcDEF123456"`, "XyZabcDEF123456", "secret"},
		{`"id":"srvtoolu_01AbCdEfGhIjKlMnOp"`, "srvtoolu_01AbCdEfGhIjKlMnOp", "secret"},
		{"<task-id>a97c9bc67cc1f1e27</task-id>", ">a97c9bc67cc1f1e27", "secret"},
		{"hex40 ok? 1 2f1d0c9b8a7e6f5d4c3b2a1908f7e6d5c4b3a291", "2f1d0c9b8a7e6f5d4c3b2a1908f7e6d5c4b3a291", "secret"},
		{"919585d1700e515ac38da49e31bc60a527fc15c2 ok? 1", "919585d1700e515ac38da49e31bc60a527fc15c2 ok? 1", "secret"},
		// names in Go's module cache, a path's piece
		{"/Users/x/Library/Caches/go-build/63700b7ab65fd1d3@v63", "63700b7ab65fd1d3@v63", "secret"},
		{"d6426ecaaf0a28de@v31", "d6426ecaaf0a28de@v31", "private_email"},
		{"P=/Users/yetone/.npm/_npx/1e7f6d9597241db0", "/yetone/.", "private_email"},
		// a piece of a base64 image
		{"iVBORw0KGgoAAAANSUhEUgAA+k5zM7Md7tM9ehxd51xF/isvM9", "k5zM7Md7tM9ehxd51xF", "secret"},
		// a grep alternation, a session's file, a link
		{"rg 'modelTakes|modelFits|personFits' internal", "modelTakes|modelFits|personFits", "secret"},
		{"wrote 31a4f944-e7cf-4f6f-acc7-eef50d9911dd.jsonl", "31a4f944-e7cf-4f6f-acc7-eef50d9911dd.jsonl", "secret"},
		{"join at discord.gg/Ab3dEf9h today", "discord.gg/Ab3dEf9h", "secret"},
	} {
		f := &fakeModel{values: map[string]string{c.value: c.label}}
		if out, n := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); n != 0 {
			t.Errorf("%q as %s: masked %q", c.value, c.label, out)
		}
	}
}

// What the checks above leave out is still masked.
func TestModelFindsStillMasked(t *testing.T) {
	for _, c := range []struct{ text, value, label string }{
		{"my account is 13393848347348", "13393848347348", "account_number"},
		{"Ask Rina Kowalczyk about it", "Rina Kowalczyk", "private_person"},
		{"Ludwig van Beethoven wrote it", "Ludwig van Beethoven", "private_person"},
		{"call me on 13800138000 tonight", "13800138000", "private_phone"},
		{"ring 020 7946 0958 after six", "020 7946 0958", "private_phone"},
		{"send it to 42 Wallaby Way, Sydney", "42 Wallaby Way, Sydney", "private_address"},
		{"the vault phrase is Zq8vR2mT6wLp", "Zq8vR2mT6wLp", "secret"},
		{"api key: 9f2c4e1a7b3d5c8e9f2c4e1a7b3d5c8e", "9f2c4e1a7b3d5c8e9f2c4e1a7b3d5c8e", "secret"},
		{"write to rina.k@kowalczyk.pl", "rina.k@kowalczyk.pl", "private_email"},
	} {
		f := &fakeModel{values: map[string]string{c.value: c.label}}
		if out, _ := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); strings.Contains(out, c.value) {
			t.Errorf("%q as %s: left in %q", c.value, c.label, out)
		}
	}
}
