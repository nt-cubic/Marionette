package gui

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/yetone/magpie/internal/proc"
)

// Rex (superlogical.com/rex, a terminal multiplexer) claims no .command
// file, only public.item and rex://, so Launch Services never lists it as
// a terminal and `open -a` can't hand it a script (StringKe, #4565). Its
// own CLI opens a command in it: `rex run` adds a tab to the session the
// app shows, running the command; with no session to add to (exit 3, a
// target not found) `rex new` makes one. Rex is offered when its CLI is
// found, on PATH or in the app.
const rexBundleID = "com.superlogical.rex"

// rexFound is the rex CLI, or "" with none: a var so a test puts a fake.
var rexFound = findRex

var rexMemo struct {
	sync.Mutex
	at   time.Time
	path string
}

// findRex is the rex program: one on PATH, else one in a Rex app in
// /Applications or ~/Applications (the app puts its own on PATH only in
// the terminals it opens). Only a native program counts: Rex, the Perl
// deployment tool, installs a script named rex too. Kept a minute, as
// the settings ask on every state.
func findRex() string {
	rexMemo.Lock()
	defer rexMemo.Unlock()
	if !rexMemo.at.IsZero() && time.Since(rexMemo.at) < time.Minute {
		return rexMemo.path
	}
	rexMemo.at, rexMemo.path = time.Now(), lookRex()
	return rexMemo.path
}

func lookRex() string {
	if p, err := proc.Look("rex"); err == nil && nativeProgram(p) {
		return p
	}
	var apps []string
	for _, dir := range []string{"/Applications", filepath.Join(os.Getenv("HOME"), "Applications")} {
		m, _ := filepath.Glob(filepath.Join(dir, "Rex*.app"))
		apps = append(apps, m...)
	}
	for _, app := range apps {
		if p := rexInApp(app); p != "" {
			return p
		}
	}
	return ""
}

// rexInApp is an executable named rex under app's Contents, not in a
// framework, at most four folders down.
func rexInApp(app string) string {
	root := filepath.Join(app, "Contents")
	found := ""
	filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil || found != "" {
			return fs.SkipDir
		}
		if d.IsDir() {
			rel, _ := filepath.Rel(root, p)
			if strings.Count(rel, string(filepath.Separator)) >= 4 || strings.HasSuffix(d.Name(), ".framework") || d.Name() == "Frameworks" {
				return fs.SkipDir
			}
			return nil
		}
		if d.Name() == "rex" && nativeProgram(p) {
			found = p
			return fs.SkipAll
		}
		return nil
	})
	return found
}

// nativeProgram says p is an executable Mach-O or ELF file, not a script.
func nativeProgram(p string) bool {
	st, err := os.Stat(p)
	if err != nil || !st.Mode().IsRegular() || st.Mode().Perm()&0o111 == 0 {
		return false
	}
	f, err := os.Open(p)
	if err != nil {
		return false
	}
	defer f.Close()
	head := make([]byte, 4)
	if _, err := f.Read(head); err != nil {
		return false
	}
	for _, magic := range [][]byte{
		{0xcf, 0xfa, 0xed, 0xfe}, {0xce, 0xfa, 0xed, 0xfe}, // Mach-O 64, 32
		{0xca, 0xfe, 0xba, 0xbe}, // universal
		{0x7f, 'E', 'L', 'F'},
	} {
		if bytes.Equal(head, magic) {
			return true
		}
	}
	return false
}

// withRex adds Rex to the terminals found when its CLI is, as the program
// that opens it.
func withRex(found terminalDiscovery) terminalDiscovery {
	for _, app := range found.Apps {
		if app.ID == rexBundleID {
			return found
		}
	}
	if p := rexFound(); p != "" {
		found.Apps = append(found.Apps, terminalApp{ID: rexBundleID, Name: "Rex", Path: p})
	}
	return found
}

// rexArgs is how rex starts command in a new tab labelled label: the
// command in sh, as the .command file runs it, then the login shell, so
// the tab stays when the agent quits. Nothing is parsed by a shell on the
// way: the label and the script are arguments of their own.
func rexArgs(verb, command, label string) []string {
	script := command + "\nexec \"${SHELL:-/bin/zsh}\" -l"
	args := []string{verb}
	if verb == "new" {
		args = append(args, label) // the new session's label
	}
	args = append(args, "--label", label, "--", "/bin/sh", "-c", script)
	return args
}

// rexLabel is a tab's label: the session's or agent's name on one line,
// cut to 60 characters; "magpie" when it has none.
func rexLabel(s string) string {
	s = strings.Join(strings.Fields(s), " ")
	if s == "" {
		return "magpie"
	}
	if utf8.RuneCountInString(s) > 60 {
		s = string([]rune(s)[:59]) + "…"
	}
	return s
}

// rexRun runs the rex CLI, a var so a test runs none.
var rexRun = func(ctx context.Context, bin string, args ...string) ([]byte, error) {
	return proc.CommandContext(ctx, bin, args...).CombinedOutput()
}

// rexShow brings Rex forward, a var so a test opens nothing.
var rexShow = func() { proc.Command("open", "-b", rexBundleID).Run() }

// openInRex opens command in a new tab of Rex, through its CLI at bin.
func openInRex(bin, command, label string) error {
	label = rexLabel(label)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	out, err := rexRun(ctx, bin, rexArgs("run", command, label)...)
	if ee := (*exec.ExitError)(nil); errors.As(err, &ee) && ee.ExitCode() == 3 {
		// no session to add a tab to: a session of its own
		out, err = rexRun(ctx, bin, rexArgs("new", command, label)...)
	}
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if ee := (*exec.ExitError)(nil); errors.As(err, &ee) && ee.ExitCode() == 4 {
			return errors.New("rex couldn't reach the Rex server: open Rex and try again")
		}
		if msg != "" {
			return fmt.Errorf("rex: %s", msg)
		}
		return fmt.Errorf("rex: %w", err)
	}
	rexShow()
	return nil
}
