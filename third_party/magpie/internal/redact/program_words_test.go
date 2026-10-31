package redact

import (
	"encoding/json"
	"strings"
	"testing"
)

// kekek711 on X, 10-29: "node_modules给识别成姓名了". With the local model
// on, the folder went to the vendor as a placeholder of personal data, and
// the agent couldn't read or list it. A program's words (a folder, an
// identifier) are no one's name or address, wherever the model's span
// ends: alone, with the command before it, or run on into the sentence
// after it. Here in a Claude Code request as it is sent: a Bash call, its
// tool_result, and the user's sentence.
func TestModelLeavesProgramWords(t *testing.T) {
	const (
		cmd      = "ls node_modules"
		listing  = "@types\ntypescript\nvite\n"
		bin      = "node_modules/.bin/tsc --noEmit"
		sentence = "node_modules 太大了"
	)
	body, _ := json.Marshal(map[string]any{"model": "claude-opus-5-5", "messages": []any{
		map[string]any{"role": "assistant", "content": []any{map[string]any{"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": map[string]any{"command": cmd}}}},
		map[string]any{"role": "user", "content": []any{map[string]any{"type": "tool_result", "tool_use_id": "toolu_01", "content": listing + bin + "\n"}}},
		map[string]any{"role": "user", "content": sentence},
	}})
	for _, label := range []string{"private_person", "private_address", "secret"} {
		for _, v := range []string{"node_modules", "node_modules/", "node_modules 太", "ls node_modules"} {
			if label == "secret" && strings.Contains(v, " ") {
				continue // a secret's span with words around it: not changed here
			}
			Forget()
			resetSent()
			f := &fakeModel{values: map[string]string{v: label}}
			out, _ := MaskJSON(body, Options{Secrets: true, Personal: true, Model: f.find})
			for _, want := range []string{`"command":"` + cmd + `"`, bin, sentence} {
				if !strings.Contains(string(out), want) {
					t.Errorf("%s %q: %q masked: %s", label, v, want, out)
				}
			}
		}
	}
	// the class: a folder or a word of a program's, as the model finds it
	for _, c := range []struct{ text, value string }{
		{"du -sh site-packages __pycache__ dist vendor", "site-packages"},
		{"du -sh site-packages __pycache__ dist vendor", "__pycache__"},
		{"du -sh site-packages __pycache__ dist vendor", "dist"},
		{"du -sh site-packages __pycache__ dist vendor", "vendor"},
		{"find . -name __pycache__ -prune", "__pycache__"},
		{"rm -rf node_modules && pnpm install", "node_modules"},
		{"确实有问题。node_modules给识别成姓名了", "node_modules给"},
	} {
		for _, label := range []string{"private_person", "private_address", "secret"} {
			if label == "secret" && strings.Contains(c.value, "给") {
				continue
			}
			Forget()
			f := &fakeModel{values: map[string]string{c.value: label}}
			if out, n := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); n != 0 {
				t.Errorf("%s %q: masked %q", label, c.value, out)
			}
		}
	}
}

// A real name and a real address beside the folder are masked still.
func TestModelStillMasksNamesBesideProgramWords(t *testing.T) {
	Forget()
	const text = "Rina Kowalczyk said node_modules is 2 GB; she lives at 42 Wallaby Way, Sydney, 张伟住在建国路88号"
	f := &fakeModel{values: map[string]string{
		"Rina Kowalczyk": "private_person", "42 Wallaby Way, Sydney": "private_address", "建国路88号": "private_address",
		"node_modules": "private_person",
	}}
	out, n := Mask(text, Options{Secrets: true, Personal: true, Model: f.find})
	for _, gone := range []string{"Rina Kowalczyk", "42 Wallaby Way", "建国路88号"} {
		if strings.Contains(out, gone) {
			t.Errorf("%q not masked: %q", gone, out)
		}
	}
	if !strings.Contains(out, " node_modules is 2 GB") || n != 3 {
		t.Errorf("%d masked: %q", n, out)
	}
}
