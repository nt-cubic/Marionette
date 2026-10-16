package plugin

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/testenv"
)

// TestHostStartsPastAnOverlongStderrLine runs the real host with the real Bun
// testenv finds (the one on PATH, at least BunVersion's MinBun), and a plugin
// whose load writes one 5 MiB line to stderr. The line stops the drain at
// 4 MiB; with the child's stderr left unread, on Windows its init answer never
// comes and startOn runs out of time. On Linux Bun keeps the unread bytes in
// memory instead and starts either way, so there TestDrainStderrKeepsTheChildGoing
// is what fails without the fix.
func TestHostStartsPastAnOverlongStderrLine(t *testing.T) {
	bun := testenv.Bun(t)
	dir := storeSandbox(t)
	plug := filepath.Join(dir, "plug.mjs")
	src := "console.log('x'.repeat(5 * 1024 * 1024))\nexport const name = 'overlong-stderr'\n"
	if err := os.WriteFile(plug, []byte(src), 0o600); err != nil {
		t.Fatal(err)
	}
	list, err := json.Marshal(map[string]any{"plugins": []map[string]string{{"spec": plug}}})
	if err != nil {
		t.Fatal(err)
	}
	writeList(t, string(list))

	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	h, _, err := startOn(ctx, bun)
	if err != nil {
		t.Fatalf("the host did not start past the plugin's overlong stderr line: %v", err)
	}
	t.Cleanup(h.stop)
}
