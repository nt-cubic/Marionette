package plugin

import (
	"bytes"
	"log"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"
)

// The test binary stands in for a plugin's child when MAGPIE_STDERR_CHILD is
// set: 8 MiB to stderr with no newline, then a marker line, then it exits.
func init() {
	if os.Getenv("MAGPIE_STDERR_CHILD") == "1" {
		chunk := strings.Repeat("x", 64<<10)
		for range 128 {
			os.Stderr.WriteString(chunk)
		}
		os.Stderr.WriteString("\nMARKER\n")
		os.Exit(0)
	}
}

// A plugin's console.log of one line over 4 MiB stopped the drain, and nothing
// read the child's stderr after that, so the child blocked on its next write.
func TestDrainStderrKeepsTheChildGoing(t *testing.T) {
	var logs bytes.Buffer
	old := log.Writer()
	log.SetOutput(&logs)
	t.Cleanup(func() { log.SetOutput(old) })

	cmd := exec.Command(os.Args[0], "-test.run=^TestDrainStderrKeepsTheChildGoing$")
	cmd.Env = append(os.Environ(), "MAGPIE_STDERR_CHILD=1")
	stderr, err := cmd.StderrPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	drained := make(chan struct{})
	go func() {
		drainStderr(stderr)
		close(drained)
	}()
	exited := make(chan error, 1)
	go func() { exited <- cmd.Wait() }()
	select {
	case <-exited:
	case <-time.After(15 * time.Second):
		cmd.Process.Kill()
		<-exited
		<-drained
		t.Fatal("the child is still blocked writing stderr after a line over 4 MiB")
	}
	<-drained
	if !strings.Contains(logs.String(), "stderr is dropped from here on") {
		t.Fatalf("the log doesn't say the plugin's stderr stopped being logged:\n%.300s", logs.String())
	}
}
