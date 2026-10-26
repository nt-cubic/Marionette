package agent

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// restartSandbox is a home with a provider in it and WorkBuddy found by its
// folder; the system is said to be neither macOS nor Windows until a test
// says which.
func restartSandbox(t *testing.T) string {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("WORKBUDDY_CONFIG_DIR", "")
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro"}}); err != nil {
		t.Fatal(err)
	}
	os.MkdirAll(filepath.Join(home, ".workbuddy"), 0o755)
	os.WriteFile(filepath.Join(home, ".workbuddy", "models.json"), []byte(`[]`), 0o600)
	notOnMac(t)
	wasWin, wasMac, wasFolders, wasRunning := restartOnWindows, restartOnMac, appFolders, running
	wasQuit, wasOpen, wasQuitWait := appQuit, appOpen, appQuitWait
	wasProcs, wasClose, wasEnd, wasLaunch, wasCloseWait, wasBackWait, wasPoll := winProcs, winClose, winEnd, winLaunch, appCloseWait, appBackWait, appPoll
	t.Cleanup(func() {
		restartOnWindows, restartOnMac, appFolders, running = wasWin, wasMac, wasFolders, wasRunning
		appQuit, appOpen, appQuitWait = wasQuit, wasOpen, wasQuitWait
		winProcs, winClose, winEnd, winLaunch, appCloseWait, appBackWait, appPoll = wasProcs, wasClose, wasEnd, wasLaunch, wasCloseWait, wasBackWait, wasPoll
	})
	restartOnWindows, restartOnMac = false, false
	appPoll = 5 * time.Millisecond
	return home
}

// macApp puts a bundle with id and executable exe at dir/name, as an
// installer leaves it.
func macApp(t *testing.T, dir, name, id, exe string) string {
	t.Helper()
	b := filepath.Join(dir, name)
	os.MkdirAll(filepath.Join(b, "Contents", "MacOS"), 0o755)
	os.WriteFile(filepath.Join(b, "Contents", "Info.plist"), []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
	<key>CFBundleExecutable</key>
	<string>`+exe+`</string>
	<key>CFBundleIdentifier</key>
	<string>`+id+`</string>
</dict></plist>`), 0o644)
	return b
}

// WorkBuddy on a Mac: its row offers a Restart only while WorkBuddy.app
// runs on a models.json older than magpie's last write of it, and the
// Restart quits it as ⌘Q does, waits for it to end, and opens it again.
// One that doesn't end is said and not opened a second time.
func TestWorkBuddyRestartOnMac(t *testing.T) {
	home := restartSandbox(t)
	apps := filepath.Join(home, "Applications")
	bundle := macApp(t, apps, "WorkBuddy.app", "com.tencent.workbuddy.mac", "Electron")
	// WorkBuddy AI's bundle is another app: never WorkBuddy's
	macApp(t, apps, "WorkBuddy AI.app", "com.workbuddy.workbuddy-ai", "Electron")
	restartOnMac = true
	appFolders = func() []string { return []string{apps} }
	alive, up := true, time.Hour
	var pats []string
	running = func(pat string) []runningProc {
		pats = append(pats, pat)
		ps := []runningProc{
			{up, bundle + "/Contents/MacOS/Electron"},
			{up, bundle + "/Contents/Frameworks/WorkBuddy Helper.app/Contents/MacOS/WorkBuddy Helper --type=gpu-process"},
		}
		var out []runningProc
		for _, p := range ps {
			if alive && regexp.MustCompile(pat).MatchString(p.cmd) {
				out = append(out, p)
			}
		}
		return out
	}
	a := workbuddy(home)

	if a.AppBehind() != nil {
		t.Fatal("offered a Restart for WorkBuddy not connected to magpie")
	}
	if err := a.Field("provider").Set("magpie"); err != nil {
		t.Fatal(err)
	}
	got := a.AppBehind()
	if got == nil {
		t.Fatalf("no Restart for WorkBuddy started an hour before magpie wrote its models (asked %q)", pats)
	}
	if d := time.Since(got.Since); d < 59*time.Minute || d > 61*time.Minute {
		t.Fatalf("since %v ago", d)
	}
	if !slices.ContainsFunc(pats, func(p string) bool { return strings.Contains(p, `WorkBuddy\.app/Contents/MacOS/Electron`) }) {
		t.Fatalf("not asked by the bundle's executable: %q", pats)
	}

	// reopened since: nothing to offer
	up = 0
	if a.AppBehind() != nil {
		t.Fatal("offered a Restart for WorkBuddy started after the change")
	}
	// not running: nothing to offer
	up, alive = time.Hour, false
	if a.AppBehind() != nil {
		t.Fatal("offered a Restart for WorkBuddy not running")
	}
	alive = true

	var did []string
	appQuit = func(_ context.Context, b string) error { did = append(did, "quit "+b); alive = false; return nil }
	appOpen = func(_ context.Context, b string) error { did = append(did, "open "+b); alive, up = true, 0; return nil }
	if err := a.RestartApp(context.Background()); err != nil {
		t.Fatal(err)
	}
	if want := []string{"quit " + bundle, "open " + bundle}; !slices.Equal(did, want) {
		t.Fatalf("did %q, want %q", did, want)
	}
	if a.AppBehind() != nil {
		t.Fatal("still offered after the restart")
	}
	// a second Restart, from a page that hadn't caught up: nothing is quit
	did = nil
	if err := a.RestartApp(context.Background()); err == nil || len(did) > 0 {
		t.Fatalf("restarted an app on the current settings: %v, %q", err, did)
	}

	// an app that doesn't end (it asks something): said, never opened again
	up = time.Hour
	appQuit = func(_ context.Context, b string) error { did = append(did, "quit "+b); return nil }
	appQuitWait = 50 * time.Millisecond
	err := a.RestartApp(context.Background())
	if err == nil || err.Error() != noticeRestartStillOpen.say("agent", "WorkBuddy") {
		t.Fatalf("err %v", err)
	}
	if want := []string{"quit " + bundle}; !slices.Equal(did, want) {
		t.Fatalf("did %q, want %q", did, want)
	}
}

// WorkBuddy on Windows: its main process is the one whose parent isn't
// WorkBuddy's, its windows are asked to close, and when it stays in the
// tray it is ended with its tree before it is opened again.
func TestWorkBuddyRestartOnWindows(t *testing.T) {
	home := restartSandbox(t)
	restartOnWindows = true
	appCloseWait, appBackWait = 50*time.Millisecond, time.Second
	const exe = `C:\Program Files\WorkBuddy\WorkBuddy.exe`
	start := time.Now().Add(-time.Hour)
	alive := true
	winProcs = func(name string) []winProc {
		if !alive || !strings.EqualFold(name, "WorkBuddy.exe") {
			return nil
		}
		return []winProc{
			{pid: 11, parent: 10, image: exe, start: start.Add(time.Second)},
			{pid: 10, parent: 4, image: exe, start: start},
			{pid: 12, parent: 10, image: exe, start: start.Add(2 * time.Second)},
			// the international build, never WorkBuddy's
			{pid: 20, parent: 4, image: `C:\Program Files\WorkBuddy AI\WorkBuddy.exe`, start: start.Add(-time.Hour)},
		}
	}
	var did []string
	winClose = func(pids []int) error {
		slices.Sort(pids)
		did = append(did, "close "+strings.Trim(fmt.Sprint(pids), "[]"))
		return nil
	}
	winEnd = func(pid int) error {
		did = append(did, fmt.Sprint("end ", pid))
		alive = false
		return nil
	}
	winLaunch = func(p winProc) error {
		did = append(did, "open "+p.image)
		alive, start = true, time.Now()
		return nil
	}
	a := workbuddy(home)
	if err := a.Field("provider").Set("magpie"); err != nil {
		t.Fatal(err)
	}
	got := a.AppBehind()
	if got == nil || !got.Since.Equal(start) {
		t.Fatalf("behind: %+v, want since %v", got, start)
	}
	if err := a.RestartApp(context.Background()); err != nil {
		t.Fatal(err)
	}
	if want := []string{"close 10 11 12", "end 10", "open " + exe}; !slices.Equal(did, want) {
		t.Fatalf("did %q, want %q", did, want)
	}
	if a.AppBehind() != nil {
		t.Fatal("still offered after the restart")
	}
}

// Which Windows processes are each app's: Claude Desktop's claude.exe, not
// Claude Code's; WorkBuddy's WorkBuddy.exe, not WorkBuddy AI's.
func TestRestartAppsOnWindows(t *testing.T) {
	cases := []struct {
		id, image string
		want      bool
	}{
		{"claude-desktop", `C:\Users\u\AppData\Local\AnthropicClaude\app-2.9939.2\claude.exe`, true},
		{"claude-desktop", `C:\Users\u\AppData\Local\AnthropicClaude\claude.exe`, true},
		{"claude-desktop", `C:\Program Files\WindowsApps\Claude_1.0.0.0_x64__pzs8sxrjxfjjc\app\claude.exe`, true},
		{"claude-desktop", `C:\Program Files\WindowsApps\AnthropicPBC.Claude_1.0.0.0_x64__fnn82j28hfe8t\app\Claude.exe`, true},
		{"claude-desktop", `C:\Users\u\.local\bin\claude.exe`, false},
		{"claude-desktop", `C:\Users\u\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe`, false},
		{"claude-desktop", `C:\Users\u\AppData\Local\AnthropicClaude\Update.exe`, false},
		{"workbuddy", `C:\Program Files\WorkBuddy\WorkBuddy.exe`, true},
		{"workbuddy", `C:\Program Files\WorkBuddy AI\WorkBuddy.exe`, false},
		{"workbuddy", `C:\Program Files\WorkBuddy\resources\cli\codebuddy.exe`, false},
	}
	for _, c := range cases {
		if got := restartApps[c.id].win(c.image); got != c.want {
			t.Errorf("%s %s: %v, want %v", c.id, c.image, got, c.want)
		}
	}
	if restartApps["workbuddy-ai"].win != nil {
		t.Error("WorkBuddy AI's Windows build isn't known, yet it has a matcher")
	}
	if _, ok := restartApps["codex"]; ok {
		t.Error("the Codex app has its own Restart (codex_app.go)")
	}
}

// A change no file's time shows (Claude Desktop's models, from the
// gateway's catalog) is put at when it is first seen, not at the files'
// last write: a Desktop started between the two still read the old list.
func TestChangedAtWithoutAWrite(t *testing.T) {
	restartSandbox(t)
	f := filepath.Join(t.TempDir(), "config.json")
	os.WriteFile(f, []byte("{}"), 0o600)
	old := time.Now().Add(-2 * time.Hour)
	os.Chtimes(f, old, old)
	a := &Agent{ID: "changed-at-test"}
	list := "a"
	reads := func() string { return list }
	if got := a.changedAt([]string{f}, reads); !got.Equal(old) {
		t.Fatalf("first seen: %v, want the file's %v", got, old)
	}
	if got := a.changedAt([]string{f}, reads); !got.Equal(old) {
		t.Fatalf("again: %v, want %v", got, old)
	}
	list = "a b"
	before := time.Now()
	got := a.changedAt([]string{f}, reads)
	if got.Before(before) {
		t.Fatalf("changed with no write: %v, want now (after %v)", got, before)
	}
	if again := a.changedAt([]string{f}, reads); !again.Equal(got) {
		t.Fatalf("moved on a second read: %v, then %v", got, again)
	}
	// a change with a write is at the write, as before
	list = "a b c"
	later := time.Now().Add(time.Hour)
	os.Chtimes(f, later, later)
	if got := a.changedAt([]string{f}, reads); !got.Equal(later) {
		t.Fatalf("with a write: %v, want %v", got, later)
	}
}

// WorkBuddy saves models.json itself seconds after every start (it purges
// local models the computer can't run, and writes the file even when none
// were): JSON.stringify's two-space indent with no trailing newline, and
// on Windows 5.6 the apiKey encrypted. That write is WorkBuddy's, not a
// change of magpie's, so after a restart the row says nothing more; a
// change of magpie's models still asks for one (Sac2333o on X: the row
// kept saying "已连接 · 重启 WorkBuddy 后生效" after every restart).
func TestWorkBuddyOwnSaveAfterStartIsNoChange(t *testing.T) {
	for _, osName := range []string{"darwin", "windows"} {
		t.Run(osName, func(t *testing.T) {
			home := restartSandbox(t)
			start := time.Now().Add(-time.Hour)
			if osName == "windows" {
				restartOnWindows = true
				winProcs = func(name string) []winProc {
					if !strings.EqualFold(name, "WorkBuddy.exe") {
						return nil
					}
					return []winProc{{pid: 10, parent: 4, image: `C:\Program Files\WorkBuddy\WorkBuddy.exe`, start: start}}
				}
			} else {
				apps := filepath.Join(home, "Applications")
				bundle := macApp(t, apps, "WorkBuddy.app", "com.tencent.workbuddy.mac", "Electron")
				restartOnMac = true
				appFolders = func() []string { return []string{apps} }
				running = func(pat string) []runningProc {
					p := runningProc{time.Since(start), bundle + "/Contents/MacOS/Electron"}
					if regexp.MustCompile(pat).MatchString(p.cmd) {
						return []runningProc{p}
					}
					return nil
				}
			}
			a := workbuddy(home)
			if err := a.Field("provider").Set("magpie"); err != nil {
				t.Fatal(err)
			}
			wrote := time.Now().Add(-30 * time.Minute)
			os.Chtimes(a.Path, wrote, wrote)
			if a.AppBehind() == nil {
				t.Fatal("no Restart for WorkBuddy started half an hour before magpie wrote its models")
			}

			// restarted: WorkBuddy starts, then saves models.json 4s later
			start = time.Now().Add(-10 * time.Second)
			workbuddySaves(t, a.Path, osName == "windows", start.Add(4*time.Second))
			if got := a.AppBehind(); got != nil {
				t.Fatalf("WorkBuddy's own save after its start read as magpie's change: behind since %v", got.Since)
			}
			// magpie's next sync writes the same models again
			if err := workbuddyWrite(a.Path, true); err != nil {
				t.Fatal(err)
			}
			if got := a.AppBehind(); got != nil {
				t.Fatalf("magpie's rewrite of the same models read as a change: behind since %v", got.Since)
			}
			// a model magpie adds is a change WorkBuddy hasn't read
			if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro", "flash"}}); err != nil {
				t.Fatal(err)
			}
			if err := workbuddyWrite(a.Path, true); err != nil {
				t.Fatal(err)
			}
			if a.AppBehind() == nil {
				t.Fatal("no Restart after magpie added a model WorkBuddy started without")
			}
		})
	}
}

// workbuddySaves writes models.json as WorkBuddy saves it,
// JSON.stringify(models, null, 2) with no trailing newline, the apiKey
// encrypted when encrypt, and dates the write at.
func workbuddySaves(t *testing.T, path string, encrypt bool, at time.Time) {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	if err := json.Indent(&out, bytes.TrimSpace(b), "", "  "); err != nil {
		t.Fatal(err)
	}
	s := out.String()
	if encrypt {
		s = regexp.MustCompile(`"apiKey": "[^"]*"`).ReplaceAllString(s, `"apiKey": "WBEF1:bm90LXRoZS1rZXk="`)
	}
	if s == string(b) {
		t.Fatal("WorkBuddy's save came out as magpie's bytes; the test proves nothing")
	}
	if err := os.WriteFile(path, []byte(s), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(path, at, at); err != nil {
		t.Fatal(err)
	}
}
