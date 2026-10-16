package appdir

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
)

// ZCodeDataIn is the folder ZCode (Zhipu's desktop app) keeps its .zcode
// data in, with home as the user's home folder: the home folder, unless the
// user moved it in ZCode's settings (数据存储路径, "data storage path").
// ZCode then copies its .zcode/v2 there and reads it from there from its
// next start, providers (provider_config.json, config.json) and sign-in
// (credentials.json) included; the choice stays in the home folder's
// .zcode/v2/setting.json as dataBaseDir, which ZCode's main process reads
// before anything else (readBootstrapDataBaseDirFromDisk). A value that
// isn't an absolute path is ignored, as Getenv ignores one.
//
// Skills, AGENTS.md and the CLI's config.json stay under the home folder
// whatever this says: ZCode reads those from HOME/USERPROFILE.
func ZCodeDataIn(home string) string {
	var s struct {
		DataBaseDir any `json:"dataBaseDir"`
	}
	b, err := os.ReadFile(filepath.Join(home, ".zcode", "v2", "setting.json"))
	if err != nil || json.Unmarshal(b, &s) != nil {
		return home
	}
	d, _ := s.DataBaseDir.(string)
	if d = strings.TrimSpace(d); d == "" || !rooted(d) {
		return home
	}
	return d
}
