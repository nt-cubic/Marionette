//go:build !windows

package gui

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// A second magpie on the same config finds the first one's lock held, a
// copy elsewhere (magpie-dev) has its own, and a lock whose holder ended,
// as when the Mac shuts down with magpie running, is free again (Crispin
// on Discord: two magpies after each boot).
func TestOneMagpieAtATime(t *testing.T) {
	dir := t.TempDir()
	if os.Getenv("MAGPIE_HOLD_LOCK") != "" {
		if held, err := holdInstance(os.Getenv("MAGPIE_HOLD_LOCK")); err != nil || !held {
			os.Exit(3)
		}
		os.Stdout.WriteString("held\n")
		select {}
	}
	lock := instanceLock(dir, "/Applications/Magpie.app/Contents/MacOS/magpie")
	if dev := instanceLock(dir, "/src/magpie/magpie-dev"); dev == lock {
		t.Fatal("magpie-dev shares the app's lock")
	}
	// every installed copy shares the lock: another magpie.app (one the
	// Mac reopens, one run translocated from Downloads) and Homebrew's of
	// any version (inaction on Discord: two birds after a restart)
	for _, other := range []string{
		"/Users/u/Downloads/magpie.app/Contents/MacOS/magpie",
		"/private/var/folders/x/T/AppTranslocation/1F2E/d/magpie.app/Contents/MacOS/magpie",
		"/opt/homebrew/Cellar/magpie/0.1.900/bin/magpie",
	} {
		if instanceLock(dir, other) != lock {
			t.Errorf("%s has a lock of its own", other)
		}
	}
	if filepath.Dir(lock) != dir {
		t.Fatalf("lock %s not in the config dir", lock)
	}

	// the first magpie: another process, killed as a shutdown would
	cmd := exec.Command(os.Args[0], "-test.run=^TestOneMagpieAtATime$")
	cmd.Env = append(os.Environ(), "MAGPIE_HOLD_LOCK="+lock)
	out, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 5)
	if _, err := out.Read(buf); err != nil || string(buf[:4]) != "held" {
		cmd.Process.Kill()
		t.Fatalf("the first magpie didn't take the lock: %q %v", buf, err)
	}
	if held, err := holdInstance(lock); err != nil || held {
		cmd.Process.Kill()
		t.Fatalf("a second magpie took the held lock: %v %v", held, err)
	}
	cmd.Process.Kill()
	cmd.Wait()
	if held, err := holdInstance(lock); err != nil || !held {
		t.Fatalf("the lock of a magpie that was killed stayed held: %v %v", held, err)
	}
	heldInstance.Close()
	heldInstance = nil
}
