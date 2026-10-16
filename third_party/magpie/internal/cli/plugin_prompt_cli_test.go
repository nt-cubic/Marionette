package cli

import (
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// TestAskPromptPasswordIsUnseen: a plugin question of type "password" (a
// Qoder PAT, #1982) is read from the terminal without echo, while a "text"
// question on the same terminal is still typed in the open; piped in, a
// password question is read as a line, as a script answers it.
func TestAskPromptPasswordIsUnseen(t *testing.T) {
	hidden := 0
	oldTerm, oldRead := promptTerminal, promptHidden
	t.Cleanup(func() { promptTerminal, promptHidden = oldTerm, oldRead })
	terminal := true
	promptTerminal = func(int) bool { return terminal }
	promptHidden = func(int) ([]byte, error) { hidden++; return []byte(" pt-secret \n"), nil }

	piped(t, "typed-in-the-open")
	v, err := askPrompt(&plugin.Prompt{Type: "password", Key: "pat", Message: "Your Qoder Personal Access Token", Placeholder: "pt-..."})
	if err != nil || v != "pt-secret" || hidden != 1 {
		t.Fatalf("password on a terminal = %q, %v, hidden reads %d; want pt-secret read unseen", v, err, hidden)
	}

	piped(t, "blue")
	if v, err := askPrompt(&plugin.Prompt{Type: "text", Key: "team", Message: "Your team"}); err != nil || v != "blue" || hidden != 1 {
		t.Fatalf("text on a terminal = %q, %v, hidden reads %d; want blue typed in the open", v, err, hidden)
	}

	terminal = false
	piped(t, "pt-piped")
	if v, err := askPrompt(&plugin.Prompt{Type: "password", Key: "pat", Message: "PAT"}); err != nil || v != "pt-piped" || hidden != 1 {
		t.Fatalf("password piped = %q, %v, hidden reads %d; want the line", v, err, hidden)
	}
}
