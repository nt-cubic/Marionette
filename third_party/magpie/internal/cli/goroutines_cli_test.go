package cli

import (
	"bytes"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/gateway"
)

// --help and its other forms print magpie goroutines' usage, ask nothing
// and write nothing; a flag it doesn't take, "-", or a second file are
// refused the same way, before the gateway is asked.
func TestGoroutinesHelpAndBadArgsWriteNothing(t *testing.T) {
	root, _ := heapDir(t)
	asked := false
	gw := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { asked = true }))
	defer gw.Close()
	t.Setenv("MAGPIE_ADDR", strings.TrimPrefix(gw.URL, "http://"))
	before := tree(t, root)
	for _, args := range [][]string{{"goroutines", "--help"}, {"goroutines", "-h"}, {"goroutines", "help"}, {"goroutines", "out.txt", "--help"}} {
		out, err := runOut(t, args...)
		if err != nil || !strings.Contains(out, "usage: magpie goroutines [<file>]") {
			t.Errorf("magpie %s: %v %q", strings.Join(args, " "), err, out)
		}
	}
	for _, args := range [][]string{{"goroutines", "--out", "x"}, {"goroutines", "-o"}, {"goroutines", "-"}, {"goroutines", "a.txt", "b.txt"}, {"goroutines", " "}} {
		if _, err := runOut(t, args...); err == nil || !strings.Contains(err.Error(), "nothing was written") {
			t.Errorf("magpie %s: %v", strings.Join(args, " "), err)
		}
	}
	if asked {
		t.Error("the gateway was asked")
	}
	if got := tree(t, root); !reflect.DeepEqual(got, before) {
		t.Fatalf("written:\nbefore %v\nafter  %v", before, got)
	}
}

// Against a running gateway (its Handler, on loopback), magpie goroutines
// writes the dump into the working folder, prints its path and which
// process it is of, and never writes over a file there.
func TestGoroutinesWritesTheRunningGatewaysDump(t *testing.T) {
	_, dir := heapDir(t)
	gw := httptest.NewServer(gateway.New().Handler())
	defer gw.Close()
	t.Setenv("MAGPIE_ADDR", strings.TrimPrefix(gw.URL, "http://"))

	out, err := runOut(t, "goroutines", "stuck.txt")
	if err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(dir, "stuck.txt")
	b, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	// this test's own goroutine is in it, waiting on the command
	if !bytes.HasPrefix(b, []byte("goroutine ")) || !bytes.Contains(b, []byte("TestGoroutinesWritesTheRunningGatewaysDump")) {
		t.Fatalf("not this process's goroutine dump: %.300q", b)
	}
	if info, _ := os.Stat(file); info.Mode().Perm() != 0o600 && os.PathSeparator == '/' {
		t.Errorf("mode %v", info.Mode())
	}
	for _, want := range []string{"stuck.txt", "process", "goroutines"} {
		if !strings.Contains(out, want) {
			t.Errorf("printed no %s: %q", want, out)
		}
	}

	// the same name again: refused, the first file kept
	if _, err := runOut(t, "goroutines", "stuck.txt"); err == nil || !strings.Contains(err.Error(), "there already") {
		t.Fatalf("written over: %v", err)
	}
	if again, _ := os.ReadFile(file); !bytes.Equal(again, b) {
		t.Fatal("the first dump changed")
	}

	// no name: magpie-goroutines-<date>-<time>.txt here
	if _, err := runOut(t, "goroutines"); err != nil {
		t.Fatal(err)
	}
	if m, _ := filepath.Glob(filepath.Join(dir, "magpie-goroutines-*.txt")); len(m) != 1 {
		t.Fatalf("no default file: %v", m)
	}
}

// A magpie older than this one answers 404: magpie goroutines says to
// update it and writes nothing. With no magpie answering, it says so.
func TestGoroutinesWithAnOlderOrNoGateway(t *testing.T) {
	_, dir := heapDir(t)
	old := httptest.NewServer(http.NotFoundHandler())
	defer old.Close()
	t.Setenv("MAGPIE_ADDR", strings.TrimPrefix(old.URL, "http://"))
	if _, err := runOut(t, "goroutines", "x.txt"); err == nil || !strings.Contains(err.Error(), "older than this one") {
		t.Fatalf("an older magpie: %v", err)
	}

	// a port kept by the test that hangs up on every call: one freed
	// could be handed to anything next
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			c.Close()
		}
	}()
	t.Setenv("MAGPIE_ADDR", ln.Addr().String())
	if _, err := runOut(t, "goroutines", "x.txt"); err == nil || !strings.Contains(err.Error(), "is magpie running") {
		t.Fatalf("no magpie: %v", err)
	}
	if ents, _ := os.ReadDir(dir); len(ents) != 0 {
		t.Fatalf("written: %v", ents)
	}
}
