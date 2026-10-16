package agent

import (
	"database/sql"
	"encoding/base64"
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"
)

func TestCindyLink(t *testing.T) {
	link := CindyLink("http://127.0.0.1:3425")
	data, ok := strings.CutPrefix(link, CindyScheme+"v=1&data=")
	if !ok {
		t.Fatalf("link = %s", link)
	}
	b, err := base64.RawURLEncoding.DecodeString(data)
	if err != nil {
		t.Fatal(err)
	}
	var p struct {
		Kind, Name, ID string
		Auth           struct{ Method, APIKey string }
		Endpoints      []struct {
			Protocol, BaseURL string
			Targets           []string
		}
	}
	if err := json.Unmarshal(b, &p); err != nil {
		t.Fatal(err)
	}
	if p.Kind != "custom" || p.ID != "magpie" || p.Auth.Method != "apiKey" || p.Auth.APIKey != "magpie-cindy" || len(p.Endpoints) != 3 {
		t.Fatalf("payload = %+v", p)
	}
	if p.Endpoints[0].BaseURL != "http://127.0.0.1:3425" || p.Endpoints[1].BaseURL != "http://127.0.0.1:3425/v1" {
		t.Fatalf("endpoints = %+v", p.Endpoints)
	}
}

func TestCindyHas(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cindy-local-v1.db")
	gw := "http://127.0.0.1:3425"
	if cindyHas(path, gw) {
		t.Fatal("no database, yet has magpie")
	}
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err := db.Exec(`CREATE TABLE custom_providers (id text PRIMARY KEY, name text, runtimes text DEFAULT '{}')`); err != nil {
		t.Fatal(err)
	}
	if cindyHas(path, gw) {
		t.Fatal("empty, yet has magpie")
	}
	if _, err := db.Exec(`INSERT INTO custom_providers VALUES ('p1', 'Other', '{"codex":{"baseUrl":"https://x"}}')`); err != nil {
		t.Fatal(err)
	}
	if cindyHas(path, gw) {
		t.Fatal("another provider taken for magpie")
	}
	if _, err := db.Exec(`INSERT INTO custom_providers VALUES ('p2', 'Mine', '{"codex":{"baseUrl":"http://127.0.0.1:3425/v1"}}')`); err != nil {
		t.Fatal(err)
	}
	if !cindyHas(path, gw) {
		t.Fatal("the gateway's provider not found")
	}
}

// Cindy's global build keeps a database per signed-in account in
// CindyGlobal: magpie added there is added.
func TestCindyAdded(t *testing.T) {
	local, global := t.TempDir(), t.TempDir()
	gw := "http://127.0.0.1:3425"
	if cindyAdded([]string{local, global}, gw) {
		t.Fatal("no databases, yet added")
	}
	db, err := sql.Open("sqlite", filepath.Join(global, "cindy-cmubb4059023mzr0152yezm4r.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err := db.Exec(`CREATE TABLE custom_providers (id text PRIMARY KEY, name text, runtimes text DEFAULT '{}');
		INSERT INTO custom_providers VALUES ('magpie-73271b6d', 'Magpie', '{}')`); err != nil {
		t.Fatal(err)
	}
	if !cindyAdded([]string{local, global}, gw) {
		t.Fatal("magpie in an account's database not found")
	}
}
