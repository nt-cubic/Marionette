package davsync

import (
	"context"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/library"
	"github.com/yetone/magpie/internal/provider"
)

// The remote never held C's token. A's keyed library upload therefore cannot
// recover it at merge time: C must preserve its own secret when applying it.
func TestKeyedLibraryUploadPreservesKeylessLocalSecrets(t *testing.T) {
	f, srv := newFakeS3(t)
	cfg := f.config(srv)
	cfg.Keys, cfg.Agents = false, false
	a, c := newComputer(t), newComputer(t)
	use := func(computer computer) {
		t.Helper()
		computer.use(t)
		appdir.UseExecutable("")
		provider.ForgetAccounts()
	}
	t.Cleanup(provider.ForgetAccounts)
	now := func() {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	collect := func() *library.Bundle {
		t.Helper()
		b, err := library.Collect()
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	remote := func() backup.Bundle {
		t.Helper()
		f.mu.Lock()
		data := slices.Clone(f.objects["team x+y/magpie/magpie.magpie-backup"])
		f.mu.Unlock()
		b, err := backup.Open(data, cfg.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return b
	}

	use(c)
	if _, err := library.SaveServer("", library.Server{Name: "github", Transport: "stdio", Command: "fixture-mcp",
		Env: map[string]string{"GITHUB_TOKEN": "fixture-c-mcp"}, Agents: []string{}}); err != nil {
		t.Fatal(err)
	}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if got := remote(); got.Keys || got.Library.MCP[0].Env["GITHUB_TOKEN"] != "" {
		t.Fatal("C must start a remote without its private token")
	}

	use(a)
	keyed := cfg
	keyed.Keys = true
	if err := Configure(keyed); err != nil {
		t.Fatal(err)
	}
	now()
	if collect().MCP[0].Env["GITHUB_TOKEN"] != "" {
		t.Fatal("A must receive a redaction, not C's private token")
	}
	text := "Instructions edited by A."
	if _, err := library.SaveInstructions(library.InstructionsChange{Shared: &text}); err != nil {
		t.Fatal(err)
	}
	now()
	if got := remote(); got.LibraryKeys == nil || !*got.LibraryKeys || got.Library.MCP[0].Env["GITHUB_TOKEN"] != "" {
		t.Fatal("A's keyed library upload must still contain the redacted token")
	}

	use(c)
	now()
	got := collect()
	if got.Texts["default"] != text || got.MCP[0].Env["GITHUB_TOKEN"] != "fixture-c-mcp" {
		t.Fatalf("C must receive A's instructions and keep its own token: text=%q, token=%q", got.Texts["default"], got.MCP[0].Env["GITHUB_TOKEN"])
	}
	now()
	if remote().Library.MCP[0].Env["GITHUB_TOKEN"] != "" {
		t.Fatal("C's keyless sync uploaded its private token")
	}
}
