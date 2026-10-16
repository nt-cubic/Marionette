package sessions

import (
	"bytes"
	"database/sql"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/klauspost/compress/zstd"
	_ "modernc.org/sqlite"
)

// codexState makes a Codex state database with a thread's row, as Codex
// 0.159's state_5.sqlite has it (the columns that matter).
func codexStateDB(t *testing.T, home, id, provider string) string {
	t.Helper()
	p := filepath.Join(home, "state_5.sqlite")
	db, err := sql.Open("sqlite", p)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err := db.Exec(`CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, model_provider TEXT NOT NULL, title TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0);
		CREATE INDEX idx_threads_provider ON threads(model_provider);
		INSERT INTO threads VALUES (?, 'x', ?, 'the title', 1), ('other', 'y', ?, 'another', 0)`, id, provider, provider); err != nil {
		t.Fatal(err)
	}
	return p
}

func threadProvider(t *testing.T, db, id string) (provider, title string, archived int) {
	t.Helper()
	d, err := sql.Open("sqlite", db)
	if err != nil {
		t.Fatal(err)
	}
	defer d.Close()
	if err := d.QueryRow(`SELECT model_provider, title, archived FROM threads WHERE id = ?`, id).Scan(&provider, &title, &archived); err != nil {
		t.Fatal(err)
	}
	return
}

// A Codex session made with another provider (CC Switch's "custom") is
// hidden from Codex's history once Codex uses magpie (#887). The Sessions
// page says which provider each was made with and which Codex uses now;
// MoveCodexProvider changes the provider in its files and in Codex's state
// database and nothing else, keeps a copy of the files first, and moving
// it back gives the files it had, byte for byte.
func TestMoveCodexProvider(t *testing.T) {
	_, codex := manageSetup(t)
	main := filepath.Join(codex, "sessions", "2026", "09", "20", "rollout-2026-09-20T15-48-28-"+mgCX+".jsonl")
	b, err := os.ReadFile(main)
	if err != nil {
		t.Fatal(err)
	}
	orig := bytes.Replace(b, []byte(`"model_provider":"openai"`), []byte(`"model_provider":"custom"`), 1)
	old := time.Now().Add(-24 * time.Hour).Truncate(time.Second)
	os.WriteFile(main, orig, 0o644)
	os.Chtimes(main, old, old)
	seg := filepath.Join(codex, "sessions", "2026", "09", "23", "rollout-2026-09-23T00-27-05-"+mgCX+"_01a0c9f1-3d6f-7f33-b3bd-25c1dd14251b.jsonl")
	segOrig, _ := os.ReadFile(seg)
	os.WriteFile(filepath.Join(codex, "config.toml"), []byte("model = \"gpt-6\"\nmodel_provider = \"magpie\"\n\n[model_providers.magpie]\nname = \"magpie\"\n"), 0o644)
	db := codexStateDB(t, codex, mgCX, "custom")

	m, ok := findManaged(ListAgent("codex"), mgCX)
	if !ok || m.Provider != "custom" || m.UsesProvider != "magpie" {
		t.Fatalf("listed %q, uses %q; want custom, magpie", m.Provider, m.UsesProvider)
	}

	mv, err := MoveCodexProvider(mgCX, "magpie")
	if err != nil {
		t.Fatal(err)
	}
	if mv.From != "custom" || mv.To != "magpie" || len(mv.Files) != 1 || mv.Files[0] != main || len(mv.DB) != 1 {
		t.Fatalf("moved %+v", mv)
	}
	now, _ := os.ReadFile(main)
	if want := bytes.Replace(orig, []byte(`"model_provider":"custom"`), []byte(`"model_provider":"magpie"`), 1); !bytes.Equal(now, want) {
		t.Fatalf("the file became\n%s", now)
	}
	if fi, _ := os.Stat(main); !fi.ModTime().Equal(old) {
		t.Errorf("its time became %v, want %v", fi.ModTime(), old)
	}
	if b, _ := os.ReadFile(seg); !bytes.Equal(b, segOrig) {
		t.Error("a segment naming no provider was changed")
	}
	if p, title, arch := threadProvider(t, db, mgCX); p != "magpie" || title != "the title" || arch != 1 {
		t.Errorf("thread row %q %q %d", p, title, arch)
	}
	if p, _, _ := threadProvider(t, db, "other"); p != "custom" {
		t.Errorf("another thread became %q", p)
	}
	if kept, _ := filepath.Glob(filepath.Join(mv.Backup, "files", "*")); len(kept) != 2 {
		t.Fatalf("backup %v", kept)
	} else if b, _ := os.ReadFile(filepath.Join(mv.Backup, "files", "0-"+filepath.Base(main))); !bytes.Equal(b, orig) {
		t.Error("the backup isn't the file as it was")
	}
	if !strings.HasPrefix(mv.Backup, os.Getenv("XDG_CONFIG_HOME")) || !strings.Contains(mv.Backup, filepath.Join("trash", "codex-provider")) {
		t.Errorf("backup in %s", mv.Backup)
	}
	if m, _ := findManaged(ListAgent("codex"), mgCX); m.Provider != "magpie" {
		t.Fatalf("listed as %q after the move", m.Provider)
	}

	// undo: back to custom, the file as it was
	if _, err := MoveCodexProvider(mgCX, "custom"); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(main); !bytes.Equal(b, orig) {
		t.Fatal("moving back didn't give the file it was")
	}
	if p, _, _ := threadProvider(t, db, mgCX); p != "custom" {
		t.Errorf("thread row %q after moving back", p)
	}

	// one being written is left alone
	hot := time.Now()
	os.Chtimes(main, hot, hot)
	if _, err := MoveCodexProvider(mgCX, "magpie"); !errors.Is(err, ErrActive) {
		t.Fatalf("err %v, want ErrActive", err)
	}
	if b, _ := os.ReadFile(main); !bytes.Equal(b, orig) {
		t.Fatal("a running session was changed")
	}
	if _, err := MoveCodexProvider(mgCX, "bad id"); err == nil {
		t.Fatal("a provider id with a space was taken")
	}
}

// A rollout the Codex app compressed (.jsonl.zst) is moved too, and stays
// compressed.
func TestMoveCodexProviderPacked(t *testing.T) {
	_, codex := manageSetup(t)
	main := filepath.Join(codex, "sessions", "2026", "09", "20", "rollout-2026-09-20T15-48-28-"+mgCX+".jsonl")
	b, _ := os.ReadFile(main)
	b = bytes.Replace(b, []byte(`"model_provider":"openai"`), []byte(`"model_provider":"custom"`), 1)
	enc, _ := zstd.NewWriter(nil)
	os.WriteFile(main+zstSuffix, enc.EncodeAll(b, nil), 0o644)
	os.Remove(main)
	old := time.Now().Add(-24 * time.Hour)
	os.Chtimes(main+zstSuffix, old, old)

	if m, _ := findManaged(ListAgent("codex"), mgCX); m.Provider != "custom" || m.UsesProvider != "openai" {
		t.Fatalf("listed %q, uses %q", m.Provider, m.UsesProvider)
	}
	mv, err := MoveCodexProvider(mgCX, "openai")
	if err != nil || len(mv.Files) != 1 {
		t.Fatalf("%+v %v", mv, err)
	}
	f, err := os.Open(main + zstSuffix)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	dec, _ := zstd.NewReader(f)
	defer dec.Close()
	var got bytes.Buffer
	if _, err := got.ReadFrom(dec); err != nil {
		t.Fatal(err)
	}
	if want := bytes.Replace(b, []byte(`"model_provider":"custom"`), []byte(`"model_provider":"openai"`), 1); !bytes.Equal(got.Bytes(), want) {
		t.Fatal("the compressed file wasn't moved")
	}
}

// Codex's provider is its profile's when config.toml chooses one.
func TestCodexProviderIn(t *testing.T) {
	d := t.TempDir()
	if p := CodexProviderIn(d); p != "openai" {
		t.Fatalf("no config: %q", p)
	}
	os.WriteFile(filepath.Join(d, "config.toml"), []byte("model_provider = \"custom\"\nprofile = \"m\"\n[profiles.m]\nmodel_provider = \"magpie\"\n"), 0o644)
	if p := CodexProviderIn(d); p != "magpie" {
		t.Fatalf("profile: %q", p)
	}
}
