package proc

import (
	"context"
	"net"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// The process listening on a port is found, with the program it runs: here
// the test's own, on a port of its own.
func TestListeningOn(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	pids, err := ListeningOn(context.Background(), ln.Addr().(*net.TCPAddr).Port)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Contains(pids, os.Getpid()) {
		t.Fatalf("listening on the port: %v, not this process (%d)", pids, os.Getpid())
	}
	got, err := Executable(os.Getpid())
	if err != nil {
		t.Fatal(err)
	}
	want, _ := os.Executable()
	if a, b := evalPath(got), evalPath(want); a != b {
		t.Fatalf("program %q, want %q", a, b)
	}
}

// A port nothing listens on is no listener and no error, on every system:
// the take-over reads exactly that to know an older magpie let the port go
// (#2231, where Windows answered "exit status 1").
func TestNothingListeningIsNoError(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := ln.Addr().(*net.TCPAddr).Port
	ln.Close()
	pids, err := ListeningOn(context.Background(), port)
	if err != nil {
		t.Fatalf("nothing listens on %d: %v", port, err)
	}
	if len(pids) != 0 {
		t.Fatalf("nothing listens on %d, read %v", port, pids)
	}
}

func evalPath(p string) string {
	if q, err := filepath.EvalSymlinks(p); err == nil {
		return q
	}
	return p
}

// Only magpie's own programs are taken for magpie. An update moves a running
// exe aside to exe.old, and to exe.old-2, -3 when that one is held too, so
// every name it can take while it still runs is magpie's.
func TestIsMagpie(t *testing.T) {
	for path, want := range map[string]bool{
		"/Applications/Magpie.app/Contents/MacOS/magpie": true,
		"/usr/local/bin/magpie":                          true,
		`C:\Users\u\AppData\Local\magpie\magpie.exe`:     true,
		`C:\Users\u\AppData\Local\magpie\magpie.exe.old`: true,
		// an update that finds .old still held leaves the running exe at
		// .old-2, the name it holds the gateway port under
		`C:\Users\u\AppData\Local\magpie\magpie.exe.old-2`: true,
		`C:\Users\u\AppData\Local\magpie\magpie.exe.old-3`: true,
		"/home/u/magpie.old-2":                             true,
		"/home/u/dial.old-4":                               true,
		// the numbered suffix is an update's, not another program's
		`C:\Users\u\other-server.old-2`:   false,
		`C:\Users\u\magpie.old-backup`:    false,
		"/home/u/.local/bin/dial":         true,
		"/opt/magpie-dev":                 true,
		"/usr/bin/python3":                false,
		"/usr/bin/magpies":                false,
		"/tmp/other-server":               false,
		`C:\Windows\System32\svchost.exe`: false,
		"":                                false,
	} {
		if IsMagpie(path) != want {
			t.Errorf("IsMagpie(%q) = %v", path, !want)
		}
	}
}

// The programs holding a port for a container are told apart from others:
// a magpie answering there runs inside it, out of this one's reach.
func TestIsForwarder(t *testing.T) {
	for path, want := range map[string]bool{
		"/Applications/OrbStack.app/Contents/Frameworks/OrbStack Helper.app/Contents/MacOS/OrbStack Helper": true,
		"/Applications/Docker.app/Contents/MacOS/com.docker.backend":                                        true,
		"/usr/bin/docker-proxy": true,
		`C:\Program Files\Docker\Docker\resources\com.docker.backend.exe`: true,
		`C:\Windows\System32\wslrelay.exe`:                                true,
		"/opt/homebrew/bin/limactl":                                       true,
		"/usr/libexec/podman/gvproxy":                                     true,
		"/Applications/Magpie.app/Contents/MacOS/magpie":                  false,
		"/usr/bin/python3":                                                false,
		"/tmp/other-server":                                               false,
		"":                                                                false,
	} {
		if IsForwarder(path) != want {
			t.Errorf("IsForwarder(%q) = %v", path, !want)
		}
	}
}
