package agent

// Cindy (a desktop app that runs Claude Code, Codex and Pi) keeps its
// providers in its own database, their keys encrypted, and takes a new one
// only through its import link, which the user confirms in Cindy:
//
//	cindy://provider/import?v=1&data=<base64url of the provider's JSON>
//
// So magpie has nothing to set there: its row is the link, magpie as one
// custom provider with an endpoint for each of Cindy's runtimes, and
// whether Cindy has it already, read from Cindy's database and never
// written.

import (
	"database/sql"
	"encoding/base64"
	"encoding/json"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	"github.com/yetone/magpie/internal/gateway"
	_ "github.com/yetone/magpie/internal/sqlitedb"
)

// CindyScheme begins every Cindy import link.
const CindyScheme = "cindy://provider/import?"

type cindyEndpoint struct {
	Protocol  string   `json:"protocol"`
	BaseURL   string   `json:"baseUrl"`
	Targets   []string `json:"targets"`
	ModelsURL string   `json:"modelsUrl"`
}

// CindyLink is the link that adds the gateway at url to Cindy as the
// provider Magpie, keyed so that its requests are counted as Cindy's.
func CindyLink(url string) string {
	v1 := url + "/v1"
	models := v1 + "/models"
	payload := struct {
		Kind      string          `json:"kind"`
		Name      string          `json:"name"`
		ID        string          `json:"id"`
		Auth      map[string]any  `json:"auth"`
		Endpoints []cindyEndpoint `json:"endpoints"`
	}{
		Kind: "custom", Name: "Magpie", ID: magpieID,
		Auth: map[string]any{"method": "apiKey", "apiKey": agentKeyAt("cindy", url)},
		Endpoints: []cindyEndpoint{
			{"anthropic-messages", url, []string{"claude-code"}, models},
			{"openai-responses", v1, []string{"codex"}, models},
			{"openai-chat", v1, []string{"pi"}, models},
		},
	}
	b, _ := json.Marshal(payload)
	return CindyScheme + "v=1&data=" + base64.RawURLEncoding.EncodeToString(b)
}

// cindyHas reports whether Cindy's database at path has magpie among its
// custom providers: by the id or name the link gives it, or by the
// gateway's address in its endpoints. Opened read-only; no database, or
// one that can't be read, is no.
func cindyHas(path, gw string) bool {
	if _, err := os.Stat(path); err != nil {
		return false
	}
	p := filepath.ToSlash(path)
	if !strings.HasPrefix(p, "/") {
		p = "/" + p // C:/… on Windows
	}
	u := url.URL{Scheme: "file", Path: p, RawQuery: "mode=ro&_pragma=busy_timeout(1000)"}
	db, err := sql.Open("sqlite", u.String())
	if err != nil {
		return false
	}
	defer db.Close()
	var n int
	err = db.QueryRow(`SELECT count(*) FROM custom_providers
		WHERE id = ? OR lower(name) = ? OR instr(runtimes, ?) > 0`, magpieID, magpieID, gw).Scan(&n)
	return err == nil && n > 0
}

// cindyDirs are where Cindy keeps its data (~/Library/Application
// Support/Cindy on a Mac, CindyGlobal for its global build), those there
// first: there once Cindy was installed and opened.
func cindyDirs() []string {
	d, err := os.UserConfigDir()
	if err != nil {
		return nil
	}
	var have, not []string
	for _, n := range []string{"Cindy", "CindyGlobal"} {
		p := filepath.Join(d, n)
		if _, err := os.Stat(p); err == nil {
			have = append(have, p)
		} else {
			not = append(not, p)
		}
	}
	return append(have, not...)
}

// cindyAdded reports whether any of Cindy's databases in dirs has magpie:
// one per account (cindy-<account>.db) once signed in, cindy-local-v1.db
// before.
func cindyAdded(dirs []string, gw string) bool {
	for _, d := range dirs {
		dbs, _ := filepath.Glob(filepath.Join(d, "cindy-*.db"))
		for _, db := range dbs {
			if cindyHas(db, gw) {
				return true
			}
		}
	}
	return false
}

func cindy() *Agent {
	dirs := cindyDirs()
	dir := ""
	if len(dirs) > 0 {
		dir = dirs[0]
	}
	return &Agent{
		ID: "cindy", Name: "Cindy", Icon: "cindy",
		UA:  []string{"cindy"},
		Dir: dir, Path: dir,
		detect: func() bool {
			for _, d := range dirs {
				if isDir(d) {
					return true
				}
			}
			return false
		},
		Import: func() string { return CindyLink(gateway.URL()) },
		Added:  func() bool { return cindyAdded(dirs, gateway.URL()) },
	}
}
