package davsync

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"sync/atomic"
	"testing"
	"time"
)

// TestLockHolder is the other magpie of TestLock: it takes the lock, says
// so, and holds it until it is killed.
func TestLockHolder(t *testing.T) {
	if os.Getenv("MAGPIE_LOCK_HOLDER") == "" {
		t.Skip("run by TestLock")
	}
	if _, err := lock(context.Background()); err != nil {
		t.Fatal(err)
	}
	fmt.Println("locked")
	time.Sleep(time.Minute)
}

func TestLock(t *testing.T) {
	newComputer(t).use(t)
	ctx := context.Background()

	// another magpie holds it: this one waits, and gives up when told to
	holder := exec.Command(os.Args[0], "-test.run=^TestLockHolder$")
	holder.Env = append(os.Environ(), "MAGPIE_LOCK_HOLDER=1")
	out, _ := holder.StdoutPipe()
	if err := holder.Start(); err != nil {
		t.Fatal(err)
	}
	defer holder.Process.Kill()
	if line, _ := bufio.NewReader(out).ReadString('\n'); line != "locked\n" {
		t.Fatalf("the other magpie: %q", line)
	}
	short, cancel := context.WithTimeout(ctx, 300*time.Millisecond)
	defer cancel()
	if _, err := lock(short); !errors.Is(err, ErrBusy) {
		t.Fatalf("taken while another magpie held it: %v", err)
	}
	// a wait called off, not run out, isn't another magpie's doing
	called, stop := context.WithCancel(ctx)
	time.AfterFunc(100*time.Millisecond, stop)
	if _, err := lock(called); !errors.Is(err, context.Canceled) {
		t.Fatalf("a wait called off: %v", err)
	}

	// it ends without letting go, as one stopped mid-sync does: the system
	// lets go for it
	holder.Process.Kill()
	holder.Wait()
	short, cancel = context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	unlock, err := lock(short)
	if err != nil {
		t.Fatalf("after the other magpie was killed: %v", err)
	}

	// Off waits for a sync in progress, and goes on once it is done
	done := make(chan error)
	go func() { done <- locked(func() error { return nil }) }()
	select {
	case err := <-done:
		t.Fatalf("ran while the lock was held: %v", err)
	case <-time.After(300 * time.Millisecond):
	}
	unlock()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

// What reads or writes the setup does it under the lock: a sync that
// waited for another magpie's reads the setup again, and does nothing when
// that magpie turned sync off meanwhile; Configure waits its turn.
func TestLockedSetup(t *testing.T) {
	newComputer(t).use(t)
	var reqs atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		reqs.Add(1)
		w.WriteHeader(http.StatusNotFound)
	}))
	defer srv.Close()
	cfg := Config{URL: srv.URL + "/dav/", Passphrase: "correct horse"}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()

	unlock, err := lock(ctx) // another magpie's sync
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan error)
	go func() { done <- Now(ctx) }()
	time.Sleep(300 * time.Millisecond) // waiting for it
	os.Remove(path("sync.json"))       // which ends in Off
	unlock()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path("sync-state.json")); !os.IsNotExist(err) {
		t.Fatalf("a sync turned off while it waited saved its state: %v", err)
	}
	if n := reqs.Load(); n != 0 {
		t.Fatalf("%d requests to the server after sync was turned off", n)
	}

	unlock, err = lock(ctx)
	if err != nil {
		t.Fatal(err)
	}
	go func() { done <- Configure(cfg) }()
	select {
	case err := <-done:
		t.Fatalf("configured while another magpie held the lock: %v", err)
	case <-time.After(300 * time.Millisecond):
	}
	unlock()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}
