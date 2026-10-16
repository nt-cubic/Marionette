package davsync

import (
	"context"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// #1776 (tsuixl, Aliyun OSS, which answers a conditional PUT 400
// NotImplemented): two computers read the same file, A renames a provider
// and B sets the theme, and both write by HEAD then PUT; B's HEAD comes
// before A's PUT lands, so B's PUT goes over A's and both are told they
// wrote. Before, A's next sync took B's file as having seen A's and
// brought the old name back in over A's: the rename was lost on the
// server and here. Now A's write is kept as blind, and finding another
// file there, A merges from before it: the rename goes back up beside B's
// theme, and B brings it in.
func TestS3BlindWriteGoneOverIsMergedAgain(t *testing.T) {
	f, srv := newFakeS3(t)
	f.noCond = true
	cfg := f.config(srv)
	ctx := context.Background()
	const key = "team x+y/magpie/magpie.magpie-backup"
	sync := func(t *testing.T) {
		t.Helper()
		if err := Now(ctx); err != nil {
			t.Fatal(err)
		}
	}
	onServer := func(t *testing.T) backup.Bundle {
		t.Helper()
		f.mu.Lock()
		data := f.objects[key]
		f.mu.Unlock()
		b, err := backup.Open(data, cfg.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	name := func(ps []provider.Provider) string {
		for _, p := range ps {
			if p.ID == "deepseek" {
				return p.Name
			}
		}
		return ""
	}

	a, b := newComputer(t), newComputer(t)
	a.use(t)
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k1"})
	settings.Save(settings.Settings{Theme: "light"})
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	sync(t)
	b.use(t)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	sync(t)
	if got := name(must(provider.Stored())); got != "DeepSeek" {
		t.Fatalf("B's first sync: %q", got)
	}

	// the file both read
	f.mu.Lock()
	was, wasTag := f.objects[key], f.etags[key]
	f.mu.Unlock()

	a.use(t)
	provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek A", Chat: "https://api.deepseek.com/v1", Key: "k1"})
	sync(t)
	if got := name(onServer(t).Providers); got != "DeepSeek A" {
		t.Fatalf("A's write: %q", got)
	}
	// B's HEAD saw the file both read, before A's PUT landed: B's write
	// goes over A's
	f.mu.Lock()
	f.objects[key], f.etags[key] = was, wasTag
	f.mu.Unlock()
	b.use(t)
	s := settings.Load()
	s.Theme = "dark"
	settings.Save(s)
	sync(t)
	if got := onServer(t); name(got.Providers) != "DeepSeek" || got.Settings.Theme != "dark" {
		t.Fatalf("B's write over A's: %q %q", name(got.Providers), got.Settings.Theme)
	}

	// A's next sync finds B's file: A's rename stays and goes back up
	a.use(t)
	sync(t)
	if got := name(must(provider.Stored())); got != "DeepSeek A" {
		t.Fatalf("A's rename here after its next sync: %q", got)
	}
	if got := settings.Load().Theme; got != "dark" {
		t.Fatalf("B's theme on A: %q", got)
	}
	if got := onServer(t); name(got.Providers) != "DeepSeek A" || got.Settings.Theme != "dark" {
		t.Fatalf("on the server after A's next sync: %q %q", name(got.Providers), got.Settings.Theme)
	}
	if n := Status().Notice; n != nil {
		t.Fatalf("no conflict to tell: %+v", n)
	}

	// B brings the rename in, keeping its theme (its notice is still the
	// one from its first sync, which brought everything in)
	b.use(t)
	before := time.Now()
	sync(t)
	if got := name(must(provider.Stored())); got != "DeepSeek A" {
		t.Fatalf("A's rename on B: %q", got)
	}
	if got := settings.Load().Theme; got != "dark" {
		t.Fatalf("B's theme on B: %q", got)
	}
	if n := Status().Notice; n != nil && !n.At.Before(before) {
		t.Fatalf("no conflict to tell on B: %+v", n)
	}
	// and nothing more is written: each sync after reads, no PUT
	f.mu.Lock()
	puts := len(f.log)
	f.mu.Unlock()
	a.use(t)
	sync(t)
	b.use(t)
	sync(t)
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, l := range f.log[puts:] {
		if l == "PUT" {
			t.Fatalf("written again once both have it: %q", f.log[puts:])
		}
	}
}
