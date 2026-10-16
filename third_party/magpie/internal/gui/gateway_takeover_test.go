package gui

import (
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gateway"
)

// fakeOlderGateway is the test binary run as another process holding the
// gateway's port and answering as magpie 0.1.550 would, until it is ended.
func fakeOlderGateway(addr string) {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		os.Exit(3)
	}
	http.Serve(ln, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"name":"magpie","version":"0.1.550","models":3}`)
	}))
}

// The Gateway page's button (leslie_luo on Discord: the page said an older
// magpie served the gateway, and there was no telling which one to quit).
// It quits the process on the gateway's port only when its program is a
// magpie, by its pid and path, then serves the gateway here at once; a
// program of another name answering as magpie is left running and named.
// Restart then serves it here again. Both run on a port of the test's own,
// against copies of the test binary.
func TestGatewayFixQuitsOnlyAMagpie(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	port := freePort(t)
	addr := "127.0.0.1:" + strconv.Itoa(port)
	t.Setenv("MAGPIE_ADDR", addr)
	was := gateway.Version
	gateway.Version = "0.1.630"
	defer func() { gateway.Version = was }()
	served.Store(nil)
	waits := []*time.Duration{&quitWait, &killWait, &startWait}
	olds := []time.Duration{quitWait, killWait, startWait}
	quitWait, killWait, startWait = 3*time.Second, 2*time.Second, 5*time.Second
	defer func() {
		for i, w := range waits {
			*w = olds[i]
		}
	}()
	t.Cleanup(func() {
		gatewayMu.Lock()
		defer gatewayMu.Unlock()
		if served.Load() != nil && servedRun.stop != nil {
			servedRun.stop()
			<-servedRun.done
		}
		served.Store(nil)
	})

	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	// start runs the test binary under name, as the older magpie holding
	// the port; ended is closed once it has ended
	start := func(name string) (*exec.Cmd, chan struct{}) {
		if runtime.GOOS == "windows" {
			name += ".exe"
		}
		bin := filepath.Join(dir, name)
		// a copy on Windows: a link to the running test binary is the
		// running binary, which Windows won't let the temp folder's
		// cleanup delete
		if runtime.GOOS == "windows" || os.Link(self, bin) != nil {
			b, err := os.ReadFile(self)
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(bin, b, 0o755); err != nil {
				t.Fatal(err)
			}
		}
		cmd := exec.Command(bin, "-test.run=^$")
		cmd.Env = append(os.Environ(), "MAGPIE_TEST_FAKE_GATEWAY="+addr)
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		ended := make(chan struct{})
		go func() { cmd.Wait(); close(ended) }()
		t.Cleanup(func() { cmd.Process.Kill(); <-ended })
		for deadline := time.Now().Add(10 * time.Second); gateway.ServedBy().Version != "0.1.550"; time.Sleep(50 * time.Millisecond) {
			if time.Now().After(deadline) {
				t.Fatalf("%s didn't answer", name)
			}
		}
		return cmd, ended
	}

	// a program that isn't magpie is never ended, however it answers
	other, otherEnded := start("other-server")
	out := takeOverGateway()
	if out.OK || out.Reason != "not-magpie" || out.PID != other.Process.Pid || filepath.Base(out.Path) != filepath.Base(other.Path) {
		t.Fatalf("another program's port: %+v", out)
	}
	select {
	case <-otherEnded:
		t.Fatal("ended a program that isn't magpie")
	case <-time.After(300 * time.Millisecond):
	}
	if served.Load() != nil {
		t.Fatal("served beside it")
	}
	other.Process.Kill()
	<-otherEnded

	// an older magpie is quit, and this one serves at once
	old, oldEnded := start("magpie")
	_ = old
	if g := providersState().Gateway; !g.Older || g.Mine {
		t.Fatalf("not seen as older: %+v", g)
	}
	out = takeOverGateway()
	if !out.OK {
		t.Fatalf("take-over: %+v", out)
	}
	select {
	case <-oldEnded:
	case <-time.After(5 * time.Second):
		t.Fatal("the older magpie is still running")
	}
	gw := served.Load()
	if g := providersState().Gateway; gw == nil || !g.Mine || !g.Running || g.Older {
		t.Fatalf("not served here after the take-over: %+v", g)
	}

	// and Restart serves it here again, anew
	if out := restartGateway(); !out.OK {
		t.Fatalf("restart: %+v", out)
	}
	if now := served.Load(); now == nil || now == gw || !answers(port) {
		t.Fatal("the gateway wasn't restarted")
	}
}
