package cli

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"runtime"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/claudecode"
	"github.com/yetone/magpie/internal/libc"
	"github.com/yetone/magpie/internal/settings"
)

// claudeCodeOut runs `magpie claude-code args…` and gives what it printed.
func claudeCodeOut(t *testing.T, args ...string) (string, error) {
	t.Helper()
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	old := os.Stdout
	os.Stdout = w
	var out strings.Builder
	done := make(chan struct{})
	go func() {
		b, _ := io.ReadAll(r)
		out.Write(b)
		close(done)
	}()
	err = claudeCodeCmd(args)
	w.Close()
	os.Stdout = old
	<-done
	r.Close()
	return out.String(), err
}

// `magpie claude-code install --yes` on a machine with no Claude Code (a
// container, Jorben on Discord) downloads the release's build, says from
// where and to where, and the subscription then runs it.
func TestClaudeCodeInstallDownloadsTheRelease(t *testing.T) {
	platform, err := claudecode.Platform(runtime.GOOS, runtime.GOARCH, false)
	if err != nil {
		t.Skip(err)
	}
	if libc.Musl() {
		t.Skip("a musl Linux asks for the -musl build")
	}
	const body = "#!/bin/sh\necho '2.1.296 (Claude Code)'\n"
	sum := sha256.Sum256([]byte(body))
	exe := map[bool]string{true: "claude.exe", false: "claude"}[runtime.GOOS == "windows"]
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/latest":
			fmt.Fprint(w, "2.1.296")
		case "/2.1.296/manifest.json":
			fmt.Fprintf(w, `{"version":"2.1.296","manifestSignatureEnforcement":"flag","platforms":{%q:{"binary":%q,"checksum":%q,"size":%d}}}`,
				platform, exe, hex.EncodeToString(sum[:]), len(body))
		case "/2.1.296/" + platform + "/" + exe:
			fmt.Fprint(w, body)
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	oldBase, oldOwn := claudecode.Base, claudeCode.own
	claudecode.Base, claudeCode.own = srv.URL, func() string { return "" }
	t.Cleanup(func() { claudecode.Base, claudeCode.own = oldBase, oldOwn; _ = claudecode.Remove() })

	out, err := claudeCodeOut(t)
	if err != nil || !strings.Contains(out, "magpie claude-code install") {
		t.Fatalf("status before: %q, %v", out, err)
	}
	out, err = claudeCodeOut(t, "install", "--yes")
	if err != nil {
		t.Fatalf("install: %v\n%s", err, out)
	}
	for _, want := range []string{"2.1.296", srv.URL + "/2.1.296/" + platform + "/" + exe, claudecode.Root(), "SHA-256", "downloaded and checked"} {
		if !strings.Contains(out, want) {
			t.Errorf("install didn't say %q:\n%s", want, out)
		}
	}
	if p, v := claudecode.Downloaded(); v != "2.1.296" || p == "" {
		t.Fatalf("Downloaded() = %q, %q", p, v)
	}
	if out, _ := claudeCodeOut(t, "install", "--yes"); !strings.Contains(out, "downloaded already") {
		t.Errorf("a second install: %q", out)
	}
	if out, _ := claudeCodeOut(t); !strings.Contains(out, "downloaded by magpie") {
		t.Errorf("status after: %q", out)
	}
	if _, err := claudeCodeOut(t, "remove"); err != nil {
		t.Fatal(err)
	}
	if p, _ := claudecode.Downloaded(); p != "" {
		t.Errorf("remove left %s", p)
	}
}

// `magpie claude-code runs N` is the user's own bound on Claude Code
// processes (#2956), which the gateway's 503 names; auto gives it back to
// the machine's memory, and a value that isn't a count is refused and
// writes nothing.
func TestClaudeCodeRunsSetsTheUsersOwn(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	out, err := claudeCodeOut(t, "runs", "3")
	if err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().ClaudeRunsMost; got != 3 {
		t.Fatalf("claudeRunsMost is %d after `runs 3`", got)
	}
	if !strings.Contains(out, "3 Claude Code processes may run at once") || !strings.Contains(out, "your setting") {
		t.Fatalf("runs 3 said:\n%s", out)
	}
	for _, bad := range []string{"0", "-2", "lots", "--yes"} {
		if _, err := claudeCodeOut(t, "runs", bad); err == nil {
			t.Fatalf("runs %s was taken", bad)
		}
		if got := settings.Load().ClaudeRunsMost; got != 3 {
			t.Fatalf("runs %s wrote %d", bad, got)
		}
	}
	if _, err := claudeCodeOut(t, "runs", "--help"); err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().ClaudeRunsMost; got != 3 {
		t.Fatalf("runs --help wrote %d", got)
	}
	if _, err := claudeCodeOut(t, "runs", "auto"); err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().ClaudeRunsMost; got != 0 {
		t.Fatalf("runs auto left %d", got)
	}
	out, err = claudeCodeOut(t, "runs")
	if err != nil || strings.Contains(out, "your setting") {
		t.Fatalf("runs after auto said (%v):\n%s", err, out)
	}
}
