package gui

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/proc"
)

var machO = []byte{0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0}

// The rex CLI counts only as a native program: Rex, the Perl deployment
// tool, puts a script named rex on PATH too (#4565). In the app it is
// found under Contents, the app's own Rex and a framework's passed over.
func TestFindRex(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("Rex runs on macOS, Linux and FreeBSD")
	}
	dir := t.TempDir()
	write := func(p string, b []byte, mode os.FileMode) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, b, mode); err != nil {
			t.Fatal(err)
		}
	}
	script := filepath.Join(dir, "perl", "rex")
	write(script, []byte("#!/usr/bin/env perl\n"), 0o755)
	native := filepath.Join(dir, "native", "rex")
	write(native, machO, 0o755)
	plain := filepath.Join(dir, "plain", "rex")
	write(plain, machO, 0o644)
	for p, want := range map[string]bool{script: false, native: true, plain: false, filepath.Join(dir, "none"): false} {
		if got := nativeProgram(p); got != want {
			t.Errorf("nativeProgram(%s) = %v, want %v", p, got, want)
		}
	}

	app := filepath.Join(dir, "Rex Beta.app")
	write(filepath.Join(app, "Contents", "MacOS", "Rex"), machO, 0o755)
	write(filepath.Join(app, "Contents", "Frameworks", "X.framework", "rex"), machO, 0o755)
	if got := rexInApp(app); got != "" {
		t.Fatalf("rexInApp took %q: the app's own Rex or a framework's", got)
	}
	cli := filepath.Join(app, "Contents", "Resources", "bin", "rex")
	write(cli, machO, 0o755)
	if got := rexInApp(app); got != cli {
		t.Fatalf("rexInApp = %q, want %q", got, cli)
	}

	// on PATH: the script is passed over for the one in the app
	home := filepath.Join(dir, "home")
	write(filepath.Join(home, "Applications", "Rex Beta.app", "Contents", "Resources", "bin", "rex"), machO, 0o755)
	t.Setenv("HOME", home)
	t.Setenv("PATH", filepath.Dir(script))
	proc.Forget()
	t.Cleanup(proc.Forget)
	if got := lookRex(); !strings.HasPrefix(got, filepath.Join(home, "Applications")) && !strings.HasPrefix(got, "/Applications/") {
		t.Fatalf("lookRex = %q with only Perl's rex on PATH, want the app's", got)
	}
	t.Setenv("PATH", filepath.Dir(native))
	proc.Forget()
	if got := lookRex(); got != native {
		t.Fatalf("lookRex = %q, want the one on PATH %q", got, native)
	}
}

// A session opens in a new Rex tab through `rex run`, its label and its
// script arguments of their own, never parsed by a shell on the way; with
// no session to add a tab to (exit 3) through `rex new`; with Rex's server
// not running (exit 4) it says to open Rex. The script runs the command,
// then the login shell, as the .command file does.
func TestOpenInRex(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("Rex runs on macOS, Linux and FreeBSD")
	}
	wasRun, wasShow := rexRun, rexShow
	t.Cleanup(func() { rexRun, rexShow = wasRun, wasShow })
	shown := 0
	rexShow = func() { shown++ }
	exit := func(code string) error { return proc.Command("/bin/sh", "-c", "exit "+code).Run() }
	var ran [][]string
	codes := []string{}
	rexRun = func(_ context.Context, bin string, args ...string) ([]byte, error) {
		ran = append(ran, args)
		if len(codes) > 0 {
			c := codes[0]
			codes = codes[1:]
			return []byte("Error: no session"), exit(c)
		}
		return nil, nil
	}
	label := `it's "$(touch pwned)"`
	if err := openInRex("/x/rex", "cd '/tmp' && claude", label); err != nil {
		t.Fatal(err)
	}
	if len(ran) != 1 || ran[0][0] != "run" || shown != 1 {
		t.Fatalf("ran %q shown %d, want one rex run and Rex shown", ran, shown)
	}
	i := slices.Index(ran[0], "--label")
	if i < 0 || ran[0][i+1] != label || !slices.Equal(ran[0][len(ran[0])-4:len(ran[0])-1], []string{"--", "/bin/sh", "-c"}) {
		t.Fatalf("rex run args %q", ran[0])
	}

	ran, codes, shown = nil, []string{"3"}, 0
	if err := openInRex("/x/rex", "claude", "Claude Code"); err != nil {
		t.Fatal(err)
	}
	if len(ran) != 2 || ran[1][0] != "new" || ran[1][1] != "Claude Code" || shown != 1 {
		t.Fatalf("with no session: ran %q, want rex run then rex new", ran)
	}

	ran, codes, shown = nil, []string{"4"}, 0
	if err := openInRex("/x/rex", "claude", ""); err == nil || !strings.Contains(err.Error(), "open Rex") || len(ran) != 1 || shown != 0 {
		t.Fatalf("server not running: %v, ran %q, shown %d", err, ran, shown)
	}
	ran, codes = nil, []string{"1"}
	if err := openInRex("/x/rex", "claude", ""); err == nil || !strings.Contains(err.Error(), "no session") {
		t.Fatalf("rex failing: %v, want its output", err)
	}

	// the script, run as Rex runs it
	dir := t.TempDir()
	args := rexArgs("run", "cd "+shellQuote(dir)+" && echo ran > out", "x")
	t.Setenv("SHELL", "/usr/bin/true")
	if out, err := proc.Command(args[len(args)-3], args[len(args)-2:]...).CombinedOutput(); err != nil {
		t.Fatalf("script: %v %s", err, out)
	}
	if b, _ := os.ReadFile(filepath.Join(dir, "out")); string(b) != "ran\n" {
		t.Fatalf("the command didn't run in its folder: %q", b)
	}
	if got := rexLabel("  a\nlong  " + strings.Repeat("x", 80)); !strings.HasPrefix(got, "a long x") || len([]rune(got)) != 60 || rexLabel("") != "magpie" {
		t.Fatalf("rexLabel: %q", got)
	}
}
