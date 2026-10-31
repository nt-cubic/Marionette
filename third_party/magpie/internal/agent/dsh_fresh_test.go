package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A dsh CLI on the machine running the tests never decides which file a
// test's dsh is wired in; a test that needs a version says it.
func init() { dshVersion = func() string { return "" } }

// dshVersionIs holds the dsh CLI's version on this machine at v.
func dshVersionIs(t *testing.T, v string) {
	t.Helper()
	was := dshVersion
	dshVersion = func() string { return v }
	t.Cleanup(func() { dshVersion = was })
}

// dshNoApp holds DeepSeek Harness Desktop uninstalled.
func dshNoApp(t *testing.T) {
	t.Helper()
	was := appFolders
	appFolders = func() []string { return nil }
	t.Cleanup(func() { appFolders = was })
}

// liby233 on Discord: in WSL, a dsh 0.2 that had booted no profile yet was
// wired in ~/.dsh/config.yaml, which no dsh on npm reads (0.0.1-rc.1 on;
// 0.2.1-rc.5 makes $DSH_HOME/profiles/web on `dsh web`, as dsh's
// initProfile, and reads no config.yaml). magpie now makes the web
// profile's patch list as dsh's template with its route in it, which dsh
// keeps on its first boot; a dsh before profiles still gets config.yaml.
func TestDshCLIWithNoProfileYet(t *testing.T) {
	home, dir, web := dshRouteHome(t)
	if err := os.RemoveAll(dir); err != nil {
		t.Fatal(err)
	}
	dshNoApp(t)
	dshVersionIs(t, "0.2.1-rc.5")
	legacy := filepath.Join(dir, "config.yaml")
	read := func(p string) string { b, _ := os.ReadFile(p); return string(b) }

	a := dsh(home)
	if a.Path != web {
		t.Fatalf("names %s, not the web profile's patch list", a.Path)
	}
	if err := a.Pick("model", "magpie/deepseek/flash"); err != nil {
		t.Fatal(err)
	}
	s := read(web)
	if !strings.HasPrefix(s, strings.TrimSuffix(dshDesktopPatch, "[]\n")) || !strings.Contains(s, "apiKeyEnv: "+dshKeyRef) || !strings.Contains(s, "provider: "+dshRoute) || !strings.Contains(s, `model: "deepseek/flash"`) {
		t.Fatalf("the web profile magpie made:\n%s", s)
	}
	if _, err := os.Stat(legacy); err == nil {
		t.Fatalf("wrote config.yaml, which this dsh doesn't read:\n%s", read(legacy))
	}
	a = dsh(home)
	if !a.Wired() || a.Check() != "" || a.Drift() != nil {
		t.Fatalf("not wired: wired %v, check %q, drift %+v", a.Wired(), a.Check(), a.Drift())
	}
	// Disconnect before dsh booted it: what magpie made goes
	if err := a.Disconnect(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "profiles")); !os.IsNotExist(err) {
		t.Fatalf("the profile magpie made outlived Disconnect: %v\n%s", err, read(web))
	}

	// a dsh before profiles keeps config.yaml
	dshVersionIs(t, "0.0.0")
	a = dsh(home)
	if a.Path != legacy {
		t.Fatalf("an old dsh names %s", a.Path)
	}
	if err := a.Pick("model", "magpie/deepseek/flash"); err != nil {
		t.Fatal(err)
	}
	if s := read(legacy); !strings.Contains(s, "- id: llm-deepseek # magpie") {
		t.Fatalf("an old dsh's config.yaml:\n%s", s)
	}
	if _, err := os.Stat(filepath.Join(dir, "profiles")); !os.IsNotExist(err) {
		t.Fatal("made a profile for a dsh before profiles")
	}
	// its version not known: config.yaml with an entry of the user's says
	// it is one before profiles; one with magpie's alone says nothing
	dshVersionIs(t, "")
	if dshBeforeProfiles(dir, "", false) {
		t.Fatal("magpie's entries alone taken for an old dsh's config")
	}
	if err := os.WriteFile(legacy, []byte("- id: tools\n  config:\n    disabled: []\n"+read(legacy)), 0o644); err != nil {
		t.Fatal(err)
	}
	if !dshBeforeProfiles(dir, "", false) || dsh(home).Path != legacy {
		t.Fatal("an old dsh's own config.yaml not taken for one")
	}
}

// liby233's state: magpie wired a dsh 0.2 in config.yaml before it booted a
// profile, and its first boot made the web profile as its template. The
// sync carries magpie's model over to the profile, and config.yaml loses
// magpie's entries; with no profile yet, it makes the web one.
func TestDshConfigYamlCarriedToProfiles(t *testing.T) {
	for _, booted := range []bool{true, false} {
		home, dir, web := dshRouteHome(t)
		if err := os.RemoveAll(dir); err != nil {
			t.Fatal(err)
		}
		dshNoApp(t)
		legacy := filepath.Join(dir, "config.yaml")
		read := func(p string) string { b, _ := os.ReadFile(p); return string(b) }

		// what magpie wrote before, for a dsh it took for an old one
		dshVersionIs(t, "0.0.0")
		if err := dsh(home).Pick("model", "magpie/deepseek/pro"); err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(read(legacy), "- id: llm-deepseek # magpie") {
			t.Fatalf("config.yaml:\n%s", read(legacy))
		}
		dshVersionIs(t, "0.2.1-rc.5")
		if booted {
			os.MkdirAll(filepath.Dir(web), 0o755)
			os.WriteFile(web, []byte(dshDesktopPatch), 0o644)
			os.WriteFile(filepath.Join(filepath.Dir(web), "package.json"), []byte("{\n  \"name\": \"dsh-profile-web\"\n}\n"), 0o644)
		}
		a := dsh(home)
		if err := a.Sync(); err != nil {
			t.Fatal(err)
		}
		s := read(web)
		if !strings.Contains(s, "- id: llm-pi-ai # magpie") || !strings.Contains(s, "provider: "+dshRoute) || !strings.Contains(s, `model: "deepseek/pro"`) {
			t.Fatalf("booted %v: the web profile after the sync:\n%s", booted, s)
		}
		if s := read(legacy); strings.Contains(s, "# magpie") {
			t.Fatalf("booted %v: config.yaml kept magpie's entries:\n%s", booted, s)
		}
		if a = dsh(home); a.Field("model").Get() != "magpie/deepseek/pro" || a.Check() != "" {
			t.Fatalf("booted %v: model %q, check %q", booted, a.Field("model").Get(), a.Check())
		}
	}
}
