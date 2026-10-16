package provider

import (
	"slices"
	"strings"
	"testing"
	"time"
)

func folderEnv(t *testing.T, fold bool, home string) {
	t.Helper()
	oldFold, oldHome := foldCase, ruleHome
	foldCase, ruleHome = fold, func() string { return home }
	t.Cleanup(func() { foldCase, ruleHome = oldFold, oldHome })
}

// A folder is under a rule's folder at a path's boundary only, ~ is the
// home folder, a Windows drive is one with its WSL mount and a WSL
// distro's share with the distro's own path, and case counts only where
// the system's folders have it.
func TestFolderUnder(t *testing.T) {
	folderEnv(t, false, "/home/me")
	for _, tc := range []struct {
		folder, root string
		want         bool
	}{
		{"/a/b", "/a/b", true},
		{"/a/b/c", "/a/b", true},
		{"/a/b/", "/a/b", true},
		{"/a/b", "/a/b/", true},
		{"/a/bc", "/a/b", false},
		{"/a", "/a/b", false},
		{"/a/b/../x", "/a/b", false},
		{"/A/B", "/a/b", false}, // Linux: case counts
		{"/home/me/work/acme", "~/work", true},
		{"/home/me/work", "~", true},
		{"/home/mex", "~", false},
		{"/home/me/x", "~/", true},
		{`C:\Users\me\work\acme`, `C:\Users\me\work`, true},
		{`c:\users\ME\work\acme`, `C:\Users\me\work`, true}, // a drive: no case, from either side
		{"/mnt/c/Users/me/work/acme", `C:\Users\me\work`, true},
		{`C:\Users\me\work`, "/mnt/c/users/me/work", true},
		{"/mnt/c/Users/me/workx", `C:\Users\me\work`, false},
		{"/home/me/proj", `\\wsl$\Ubuntu\home\me`, true},
		{"/home/me/proj", `\\wsl.localhost\Ubuntu-24.04\home\me\proj`, true},
		{"/x", "/", true},
		{"", "/", false}, // the agent didn't say: no rule matches it
		{"", "~", false},
		{"relative/x", "/", false},
	} {
		if got := FolderUnder(tc.folder, tc.root); got != tc.want {
			t.Errorf("FolderUnder(%q, %q) = %v, want %v", tc.folder, tc.root, got, tc.want)
		}
	}
	folderEnv(t, true, "/Users/Me")
	for _, tc := range []struct {
		folder, root string
		want         bool
	}{
		{"/users/me/Work/Acme", "~/work", true}, // macOS, Windows: no case
		{"/Users/Me/Workshop", "~/work", false},
	} {
		if got := FolderUnder(tc.folder, tc.root); got != tc.want {
			t.Errorf("fold: FolderUnder(%q, %q) = %v, want %v", tc.folder, tc.root, got, tc.want)
		}
	}
	folderEnv(t, true, "")
	if FolderUnder("/x/y", "~") {
		t.Error("~ with no home folder known matched")
	}
}

// A rule's folders are whole paths, kept as typed but for a trailing
// slash; folders alone are a condition.
func TestRuleFoldersCleaned(t *testing.T) {
	members := []string{"a/m", "b/m"}
	out, err := cleanRules([]Rule{{Use: "b/m", Folders: []string{" ~/work/acme/ ", "", "~/work/acme", `C:\work\`, "/", `C:\`}}}, members)
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"~/work/acme", `C:\work`, "/", `C:\`}; !slices.Equal(out[0].Folders, want) {
		t.Errorf("folders %q, want %q", out[0].Folders, want)
	}
	if c := out[0].Conditions(); !slices.Equal(c, []string{`folder ~/work/acme|C:\work|/|C:\`}) {
		t.Errorf("conditions %q", c)
	}
	for _, bad := range []Rule{
		{Use: "b/m", Folders: []string{"work/acme"}},
		{Use: "b/m", Folders: []string{"~work"}},
		{Use: "b/m", Pause: true, Folders: []string{"/x"}, Tokens: 1000}, // a pause holds by folder, not by length
		{Use: "b/m", Folders: []string{" "}},                             // nothing left: no condition
	} {
		if _, err := cleanRules([]Rule{bad}, members); err == nil {
			t.Errorf("%+v was taken", bad)
		}
	}
}

// A rule by folder matches a request from under one of its folders, with
// its other conditions, and never one whose folder isn't known.
func TestRuleMatchesByFolder(t *testing.T) {
	folderEnv(t, false, "/home/me")
	r := Rule{Use: "b/m", Folders: []string{"~/work/acme", "/srv/oss"}}
	for _, tc := range []struct {
		q    RuleRequest
		want bool
	}{
		{RuleRequest{Folder: "/home/me/work/acme"}, true},
		{RuleRequest{Folder: "/home/me/work/acme/web"}, true},
		{RuleRequest{Folder: "/srv/oss/x"}, true},
		{RuleRequest{Folder: "/home/me/work/acme-old"}, false},
		{RuleRequest{Folder: "/home/me/work"}, false},
		{RuleRequest{}, false},
	} {
		if got := r.Matches(tc.q); got != tc.want {
			t.Errorf("%+v: %v", tc.q, got)
		}
	}
	both := Rule{Use: "b/m", Folders: []string{"/srv/oss"}, Tokens: 1000}
	if both.Matches(RuleRequest{Folder: "/srv/oss", Tokens: 10}) || !both.Matches(RuleRequest{Folder: "/srv/oss", Tokens: 2000}) {
		t.Error("a folder and a length: both must hold")
	}
	if MatchRule([]Rule{{Use: "a/m", Agents: []string{"codex"}}, r}, RuleRequest{Agent: "claude", Folder: "/srv/oss"}) != 1 {
		t.Error("the rule by folder after one by agent")
	}
}

// folders= is typed as other words are, comma separated, quoted when a
// folder has a space, and read back the same.
func TestParseRuleFolders(t *testing.T) {
	g := Group{ID: "g", Members: []string{"a/m", "b/x"}}
	r, _, _, err := ParseRule(g, RuleWords(`use=x folders="~/My Project,/srv/oss/" tokens=10k`))
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(r.Folders, []string{"~/My Project", "/srv/oss"}) {
		t.Fatalf("folders %q", r.Folders)
	}
	line := r.Line()
	if !strings.Contains(line, `folders="~/My Project,/srv/oss"`) {
		t.Errorf("line %s", line)
	}
	back, _, _, err := ParseRule(g, RuleWords(line))
	if err != nil || !slices.Equal(back.Folders, r.Folders) {
		t.Errorf("read back %q: %v", back.Folders, err)
	}
	if r, _, _, err := ParseRule(g, RuleWords(`use=x folder=~/a`)); err != nil || r.Line() != "use=b/x folders=~/a" {
		t.Errorf("%q %v", r.Line(), err)
	}
	for _, bad := range []string{`use=x folders=`, `use=x folders=work`, `pause=x folders=/a tokens=1k`, `use=x except-folders=`, `pause=x except-folders=work`} {
		if _, _, _, err := ParseRule(g, RuleWords(bad)); err == nil {
			t.Errorf("%s was taken", bad)
		}
	}
}

// #1606 (Ti800): a pause by folder. Outside some folders it holds for
// every request not known to be in one — another folder, or an agent that
// doesn't say — so the member is kept for the task there; in some folders
// it holds for theirs only. Its other conditions hold with it.
func TestPauseByFolderRule(t *testing.T) {
	folderEnv(t, false, "/home/me")
	keep := Rule{Use: "claude/opus", Pause: true, ExceptFolders: []string{"~/big-task"}}
	in := Rule{Use: "claude/opus", Pause: true, Folders: []string{"~/scratch"}}
	night := Rule{Use: "claude/opus", Pause: true, Folders: []string{"~/scratch"}, Time: &TimeWindow{From: "22:00", To: "08:00"}}
	at := time.Date(2026, 10, 12, 10, 0, 0, 0, time.Local)
	for _, tc := range []struct {
		name   string
		r      Rule
		folder string
		want   bool
	}{
		{"keep: the task", keep, "/home/me/big-task", false},
		{"keep: inside the task", keep, "/home/me/big-task/web", false},
		{"keep: another project", keep, "/home/me/small", true},
		{"keep: a name that starts the same", keep, "/home/me/big-task-2", true},
		{"keep: no folder said", keep, "", true},
		{"in: the folder", in, "/home/me/scratch/x", true},
		{"in: elsewhere", in, "/home/me/big-task", false},
		{"in: no folder said", in, "", false},
		{"in, at night: by day", night, "/home/me/scratch", false},
		{"a send-to rule pauses nothing", Rule{Use: "claude/opus", ExceptFolders: []string{"~/big-task"}}, "/x", false},
	} {
		if got := tc.r.Pauses("claude", tc.folder, at); got != tc.want {
			t.Errorf("%s: %v", tc.name, got)
		}
	}

	members := []string{"claude/opus", "openrouter/opus"}
	out, err := cleanRules([]Rule{{Use: "claude/opus", Pause: true, ExceptFolders: []string{"~/big-task/", "~/big-task"}}}, members)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(out[0].ExceptFolders, []string{"~/big-task"}) || !slices.Equal(out[0].Conditions(), []string{"not folder ~/big-task"}) {
		t.Errorf("cleaned %+v %q", out[0], out[0].Conditions())
	}
	for _, bad := range []Rule{
		{Use: "claude/opus", Pause: true, ExceptFolders: []string{"big-task"}},
		{Use: "claude/opus", Pause: true, ExceptFolders: []string{"~/x"}, Images: true},
	} {
		if _, err := cleanRules([]Rule{bad}, members); err == nil {
			t.Errorf("%+v was taken", bad)
		}
	}

	g := Group{ID: "g", Rules: []Rule{keep}}
	ms := []Member{{ID: "claude/opus", Provider: Provider{ID: "claude"}, Model: "opus", Path: []string{"claude/opus"}},
		{ID: "openrouter/opus", Provider: Provider{ID: "openrouter"}, Model: "opus", Path: []string{"openrouter/opus"}}}
	if !PausesByFolder(g, ms) || PausesByFolder(Group{Rules: []Rule{{Use: "claude/opus", Folders: []string{"/x"}}}}, ms) {
		t.Error("PausesByFolder: a pause by folder reads the folder, a send-to rule's doesn't")
	}
	kept, paused := PausedOut(g, ms, "claude", "/home/me/small", at)
	if len(kept) != 1 || kept[0].ID != "openrouter/opus" || len(paused) != 1 || paused[0].Folder != "/home/me/small" || !paused[0].ByFolder {
		t.Errorf("elsewhere: kept %+v paused %+v", kept, paused)
	}
	if kept, paused = PausedOut(g, ms, "claude", "/home/me/big-task", at); len(kept) != 2 || len(paused) != 0 {
		t.Errorf("in the task: kept %+v paused %+v", kept, paused)
	}
}

// A send-to rule outside some folders matches every request not known to
// be in one, as a pause's does.
func TestRuleMatchesExceptFolders(t *testing.T) {
	folderEnv(t, false, "/home/me")
	r := Rule{Use: "b/m", ExceptFolders: []string{"~/work/acme"}}
	for folder, want := range map[string]bool{"/home/me/work/acme": false, "/home/me/work/acme/web": false, "/home/me/oss": true, "": true} {
		if got := r.Matches(RuleRequest{Folder: folder}); got != want {
			t.Errorf("%q: %v", folder, got)
		}
	}
	both := Rule{Use: "b/m", Folders: []string{"~/work"}, ExceptFolders: []string{"~/work/acme"}}
	if !both.Matches(RuleRequest{Folder: "/home/me/work/x"}) || both.Matches(RuleRequest{Folder: "/home/me/work/acme/y"}) || both.Matches(RuleRequest{}) {
		t.Error("in ~/work but not ~/work/acme")
	}
}

// pause= takes folders= and except-folders=, typed and read back the same.
func TestParsePauseByFolder(t *testing.T) {
	g := Group{ID: "auto-claude-opus-5-5", Members: []string{"claude/claude-opus-5-5", "openrouter/anthropic/claude-opus-5-5"}}
	r, _, _, err := ParseRule(g, RuleWords(`pause=claude/claude-opus-5-5 except-folders="~/big-task,~/My Task"`))
	if err != nil {
		t.Fatal(err)
	}
	if !r.Pause || r.Use != "claude/claude-opus-5-5" || !slices.Equal(r.ExceptFolders, []string{"~/big-task", "~/My Task"}) {
		t.Fatalf("%+v", r)
	}
	line := r.Line()
	if line != `pause=claude/claude-opus-5-5 except-folders="~/big-task,~/My Task"` {
		t.Errorf("line %s", line)
	}
	if back, _, _, err := ParseRule(g, RuleWords(line)); err != nil || !slices.Equal(back.ExceptFolders, r.ExceptFolders) || !back.Pause {
		t.Errorf("read back %+v: %v", back, err)
	}
	if r, _, _, err := ParseRule(g, RuleWords(`pause=claude/claude-opus-5-5 folders=/tmp agents=claude`)); err != nil || r.Line() != "pause=claude/claude-opus-5-5 agents=claude folders=/tmp" {
		t.Errorf("%q %v", r.Line(), err)
	}
	if _, _, _, err := ParseRule(g, RuleWords(`pause=claude/claude-opus-5-5`)); err == nil {
		t.Error("a pause with no condition was taken")
	}
}
