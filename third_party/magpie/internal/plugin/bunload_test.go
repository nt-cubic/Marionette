package plugin

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// exitedWith is a process's exit as Windows reports one: its NTSTATUS as
// the exit code, which a fake bun on another system can't exit with.
type exitedWith uint32

func (e exitedWith) ExitCode() int { return int(uint32(e)) }
func (e exitedWith) Error() string { return fmt.Sprintf("exit status %#x", uint32(e)) }

// A bun that Windows won't load (STATUS_ENTRYPOINT_NOT_FOUND: Bun 1.3.14
// and later import CreatePseudoConsole, which Windows has from 10 1809 on,
// zgcwkj on Discord) or the processor won't run (STATUS_ILLEGAL_
// INSTRUCTION, SIGILL) says which and what to do; any other exit is
// given back as it was.
func TestWontLoadSaysWindowsOrCPU(t *testing.T) {
	bun := `C:\magpie\cache\bun\1.4.2\magpie-bun.exe`
	for _, c := range []struct {
		err  error
		why  string
		says []string
	}{
		{exitedWith(0xc0000139), "windows", []string{"Windows 10 version 1809", "Windows Server 2019", "0xc0000139", "MAGPIE_BUN", bun}},
		{exitedWith(0xc000001d), "cpu", []string{"processor", "SSE4.2", "MAGPIE_BUN", bun}},
	} {
		wrapped := errors.Join(errors.New("starting"), c.err)
		ue := wontLoad(bun, wrapped)
		if ue == nil || ue.Why != c.why {
			t.Fatalf("wontLoad(%v) = %#v, want %s", c.err, ue, c.why)
		}
		for _, s := range c.says {
			if !strings.Contains(ue.Error(), s) {
				t.Errorf("%s: %q doesn't say %q", c.why, ue.Error(), s)
			}
		}
		if !errors.Is(ue, c.err) {
			t.Errorf("%s: the exit isn't kept under it", c.why)
		}
		var got *BunUnstartableError
		if err := cantStart(bun, c.err); !errors.As(err, &got) || got.Why != c.why {
			t.Errorf("cantStart(%v) = %v", c.err, err)
		}
	}
	for _, other := range []error{exitedWith(1), exitedWith(0xc0000005), errors.New("exit status 3"), nil} {
		if ue := wontLoad(bun, other); ue != nil {
			t.Errorf("wontLoad(%v) = %v, want nil", other, ue)
		}
	}
	if runtime.GOOS != "windows" {
		err := exec.Command("/bin/sh", "-c", "kill -ILL $$").Run()
		if ue := wontLoad("bun", err); ue == nil || ue.Why != "cpu" {
			t.Fatalf("wontLoad(%v) = %v, want cpu", err, ue)
		}
		err = exec.Command("/bin/sh", "-c", "kill -SEGV $$").Run()
		if ue := wontLoad("bun", err); ue != nil {
			t.Fatalf("wontLoad(%v) = %v, want nil", err, ue)
		}
	}
}

// stopsOnSIGILL is a bun as one the processor can't run is: the kernel
// stops it on its first instruction the processor lacks.
func stopsOnSIGILL(t *testing.T, path string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the bun is a shell script")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("#!/bin/sh\nkill -ILL $$\n"), 0o755); err != nil {
		t.Fatal(err)
	}
}

// An install on a bun this machine can't run says so, as a
// *BunUnstartableError the GUI translates, and isn't tried again from the
// community repo's dist branch: that bun add can't start either, and its
// error read as the dist branch's (bun add ./dist/…: exit status …).
func TestInstallOnUnstartableBunSaysWhyAndSkipsDist(t *testing.T) {
	bunHome(t, BunVersion, time.Now())
	stopsOnSIGILL(t, bunExeOf(BunVersion))
	var hits atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		http.NotFound(w, r)
	}))
	t.Cleanup(srv.Close)
	old := distBase
	distBase = srv.URL
	t.Cleanup(func() { distBase = old })

	err := install(context.Background(), "@magpie-community/opencode-raccoon-auth@latest")
	var ue *BunUnstartableError
	if !errors.As(err, &ue) || ue.Why != "cpu" || ue.Bun != bunExeOf(BunVersion) {
		t.Fatalf("install on a bun stopped by SIGILL: %v", err)
	}
	if strings.Contains(err.Error(), "dist branch") || hits.Load() != 0 {
		t.Fatalf("the dist branch was tried (%d requests): %v", hits.Load(), err)
	}
}

// The host on a bun this machine can't run says why.
func TestHostOnUnstartableBunSaysWhy(t *testing.T) {
	bunHome(t, BunVersion, time.Now())
	t.Cleanup(Settle)
	stopsOnSIGILL(t, bunExeOf(BunVersion))
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	_, _ = Add(ctx, abs)
	_, err := Providers(ctx)
	var ue *BunUnstartableError
	if !errors.As(err, &ue) || ue.Why != "cpu" || !strings.Contains(err.Error(), "SSE4.2") {
		t.Fatalf("Providers on a bun stopped by SIGILL: %v", err)
	}
}
