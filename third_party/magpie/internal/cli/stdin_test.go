package cli

import (
	"os"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// swapStdin sets os.Stdin to a pipe holding text, after package init, and
// touches nothing else: it is what the Windows desktop build's init does
// when it reopens the terminal's CONIN$ (console_windows.go).
func swapStdin(t *testing.T, text string) {
	t.Helper()
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	w.WriteString(text)
	w.Close()
	old := os.Stdin
	os.Stdin = r
	t.Cleanup(func() {
		os.Stdin = old
		r.Close()
	})
}

// TestPromptsReadStdinSetAfterInit: every CLI prompt reads os.Stdin as it
// is when asked, not as it was when the package's variables were set. On
// Windows a reader made at package init held the GUI binary's invalid
// handle, and `magpie plugin login` failed with "read /dev/stdin: The
// handle is invalid" (tom047784).
func TestPromptsReadStdinSetAfterInit(t *testing.T) {
	// plugin login's question, as a line
	swapStdin(t, "blue\n")
	if v, err := askPrompt(&plugin.Prompt{Type: "text", Key: "team", Message: "Your team"}); err != nil || v != "blue" {
		t.Fatalf("askPrompt = %q, %v; want blue from the stdin set after init", v, err)
	}

	// plugin login's choice of method or option
	swapStdin(t, "2\n")
	if i, err := askNumber(3); err != nil || i != 1 {
		t.Fatalf("askNumber = %d, %v; want 1 from the stdin set after init", i, err)
	}

	// backup's and WebDAV's passphrase and password, piped: a line each,
	// both from the one reader
	swapStdin(t, "first\nsecond\n")
	for _, want := range []string{"first", "second"} {
		if v, err := secret("passphrase", "Passphrase: ", false); err != nil || v != want {
			t.Fatalf("secret = %q, %v; want %s from the stdin set after init", v, err, want)
		}
	}
}
