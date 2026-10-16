package autostart

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// what magpie up to now wrote for Open at login
const olderPlist = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>com.yetone.magpie</string>
	<key>ProgramArguments</key>
	<array><string>/Applications/Tom &amp; Jerry/magpie.app/Contents/MacOS/magpie</string><string>tray</string></array>
	<key>RunAtLoad</key><true/>
	<key>LimitLoadToSessionType</key><string>Aqua</string>
	<key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
`

// launchd kills what a job started once the job exits, so the shell a
// restart to update leaves to open the new version died with a magpie
// opened at login: the launch agent lets them go.
func TestLaunchAgentLetsTheRelaunchLive(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	if err := Set(true); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(record())
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), "<key>AbandonProcessGroup</key><true/>") {
		t.Fatalf("the launch agent doesn't abandon its process group:\n%s", b)
	}
	if out, err := exec.Command("plutil", "-lint", record()).CombinedOutput(); err != nil {
		t.Fatalf("not a valid plist: %s", out)
	}
}

// An older magpie's launch agent is written again as this one writes it,
// for the program it named; one that's off stays off.
func TestRefreshOlderLaunchAgent(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	if err := Refresh(); err != nil || Enabled() {
		t.Fatalf("Refresh turned Open at login on: %v", err)
	}
	if _, err := os.Stat(filepath.Join(home, "Library")); !os.IsNotExist(err) {
		t.Fatal("Refresh made the LaunchAgents folder for one that's off")
	}

	p := record()
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(olderPlist), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Refresh(); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(p)
	want := plist("/Applications/Tom & Jerry/magpie.app/Contents/MacOS/magpie")
	if string(b) != string(want) {
		t.Fatalf("not rewritten as this version writes it:\n%s\nwant:\n%s", b, want)
	}

	// one magpie didn't write is left as it is
	other := "<plist><dict><key>Label</key><string>x</string></dict></plist>"
	os.WriteFile(p, []byte(other), 0o644)
	if err := Refresh(); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(p); string(b) != other {
		t.Fatalf("a launch agent magpie didn't write was changed:\n%s", b)
	}
}

// fakeApp lays out an app bundle as a release's: Contents/MacOS/magpie
// beside Contents/Info.plist carrying the bundle id.
func fakeApp(t *testing.T, dir, id string) string {
	t.Helper()
	app := filepath.Join(dir, "Tom & Jerry", "magpie.app", "Contents")
	if err := os.MkdirAll(filepath.Join(app, "MacOS"), 0o755); err != nil {
		t.Fatal(err)
	}
	info := `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>magpie</string>
	<key>CFBundleIdentifier</key><string>` + id + `</string>
	<key>CFBundleExecutable</key><string>magpie</string>
</dict>
</plist>
`
	if err := os.WriteFile(filepath.Join(app, "Info.plist"), []byte(info), 0o644); err != nil {
		t.Fatal(err)
	}
	exe := filepath.Join(app, "MacOS", "magpie")
	if err := os.WriteFile(exe, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return exe
}

// Login Items lists a launch agent under the name its program is signed
// with, the developer's own, unless the agent names the app it belongs to
// (#1512, ysicing: "Xipeng Guan" instead of magpie). The launch agent for
// a program inside an app names that app's bundle id, an older magpie's is
// rewritten with it, and a bare program names none.
func TestLaunchAgentNamesItsApp(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	exe := fakeApp(t, home, "com.yetone.magpie")

	key := "<key>AssociatedBundleIdentifiers</key>\n\t<array><string>com.yetone.magpie</string></array>"
	if b := string(plist(exe)); !strings.Contains(b, key) {
		t.Fatalf("the launch agent doesn't name magpie's app:\n%s", b)
	}

	p := record()
	os.MkdirAll(filepath.Dir(p), 0o755)
	older := strings.Replace(olderPlist, "/Applications/Tom &amp; Jerry/magpie.app/Contents/MacOS/magpie", strings.ReplaceAll(exe, "&", "&amp;"), 1)
	if err := os.WriteFile(p, []byte(older), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Refresh(); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(p)
	if !strings.Contains(string(b), key) {
		t.Fatalf("an older launch agent isn't rewritten to name the app:\n%s", b)
	}
	if out, err := exec.Command("plutil", "-lint", p).CombinedOutput(); err != nil {
		t.Fatalf("not a valid plist: %s", out)
	}
	out, err := exec.Command("plutil", "-extract", "AssociatedBundleIdentifiers.0", "raw", p).CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "com.yetone.magpie" {
		t.Fatalf("plutil reads AssociatedBundleIdentifiers as %q (%v)", out, err)
	}

	// the app's Info.plist not reading is no reason to drop the name
	os.Rename(filepath.Join(filepath.Dir(filepath.Dir(exe)), "Info.plist"), filepath.Join(home, "Info.plist.aside"))
	if err := Refresh(); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(p); !strings.Contains(string(b), key) {
		t.Fatalf("the app's name was dropped over an unread Info.plist:\n%s", b)
	}

	// a program outside any app has nothing to name
	if b := string(plist(filepath.Join(home, "bin", "magpie"))); strings.Contains(b, "AssociatedBundleIdentifiers") {
		t.Fatalf("a bare program's launch agent names an app:\n%s", b)
	}
}

// A record a write cut short is not one that opens magpie at login: launchd
// refuses a job with no program to run, and one with nothing in it at all,
// so Open at login reads off rather than saying on for a record that never
// starts anything.
func TestSetThenTruncatedRecordReadsAsOff(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	if err := Set(true); err != nil {
		t.Fatal(err)
	}
	if !Enabled() {
		t.Fatal("not on after Set(true)")
	}
	p := record()
	b, err := os.ReadFile(p)
	if err != nil {
		t.Fatal(err)
	}
	// the head of the launch agent, cut off before the program it names,
	// which is what a write that goes no further leaves behind
	head := b[:strings.Index(string(b), "<key>ProgramArguments</key>")]
	if err := os.WriteFile(p, head, 0o644); err != nil {
		t.Fatal(err)
	}
	if Enabled() {
		t.Errorf("a launch agent with no ProgramArguments reads as on:\n%s", head)
	}
	if err := os.WriteFile(p, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	if Enabled() {
		t.Error("an empty launch agent reads as on")
	}
	// a program named as nothing is no program to run either, and launchd
	// refuses the job
	empty := strings.Replace(string(b), "<array><string>", "<array><string></string><string>", 1)
	if err := os.WriteFile(p, []byte(empty), 0o644); err != nil {
		t.Fatal(err)
	}
	if Enabled() {
		t.Errorf("a launch agent with no program reads as on:\n%s", empty)
	}
}
