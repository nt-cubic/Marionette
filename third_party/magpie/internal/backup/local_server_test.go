package backup

import (
	"testing"

	"github.com/yetone/magpie/internal/library"
)

func libServer(t *testing.T, name string) *library.Server {
	t.Helper()
	v, err := library.Read(nil)
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range v.Servers {
		if s.Name == name {
			return s.Server
		}
	}
	return nil
}

// A server kept to this computer (huhuy72219405 on X) is in no backup
// file, and restoring one — made before it was added, or on another
// computer with a server by its name — keeps it as it is here.
func TestRestoreKeepsLocalServer(t *testing.T) {
	home(t)
	gh := library.Server{Name: "gh", Transport: "stdio", Command: "gh-mcp", Agents: []string{}}
	if _, err := library.SaveServer("", gh); err != nil {
		t.Fatal(err)
	}
	// the other computer's db, by the name of this one's own
	if _, err := library.SaveServer("", library.Server{Name: "db", Transport: "stdio", Command: "their-db", Agents: []string{}}); err != nil {
		t.Fatal(err)
	}
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	data, _ := Seal(b, "pw")

	home(t) // this computer, with a db of its own
	if _, err := library.SaveServer("", library.Server{Name: "db", Transport: "stdio", Command: "/opt/my-db", Agents: []string{}, Local: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := library.SaveServer("", library.Server{Name: "mine", Transport: "stdio", Command: "/opt/mine", Agents: []string{}, Local: true}); err != nil {
		t.Fatal(err)
	}
	own, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	if len(own.Library.MCP) != 0 {
		t.Fatalf("a backup made here carries the local servers: %+v", own.Library.MCP)
	}
	got, err := Open(data, "pw")
	if err != nil {
		t.Fatal(err)
	}
	if r, err := Restore(got, Parts{Library: true}); err != nil || !r.Library {
		t.Fatalf("restore: %+v %v", r, err)
	}
	if s := libServer(t, "db"); s == nil || !s.Local || s.Command != "/opt/my-db" {
		t.Fatalf("db after the restore: %+v", s)
	}
	if s := libServer(t, "mine"); s == nil || !s.Local {
		t.Fatalf("mine after the restore: %+v", s)
	}
	if libServer(t, "gh") == nil {
		t.Fatal("the backup's gh didn't come in")
	}
	// this computer's own backup, restored: still there
	sealed, _ := Seal(own, "pw")
	got, _ = Open(sealed, "pw")
	if _, err := Restore(got, Parts{Library: true}); err != nil {
		t.Fatal(err)
	}
	if s := libServer(t, "mine"); s == nil || !s.Local || libServer(t, "gh") != nil {
		t.Fatalf("after this computer's own backup: mine %+v", s)
	}
}
