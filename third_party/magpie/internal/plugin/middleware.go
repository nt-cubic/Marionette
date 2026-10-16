package plugin

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
)

// Middleware is the gateway module a plugin at target has: the file its
// package.json's magpie.middleware (or magpie.gateway, the same) names, or
// the file itself for a path to a *.middleware.js or *.gateway.js (or
// .mjs). only is whether it is nothing else: no OpenCode plugin for the
// host to load (no main, no exports).
func Middleware(target string) (file string, only bool) {
	st, err := os.Stat(target)
	if err != nil {
		return "", false
	}
	if !st.IsDir() {
		base := strings.ToLower(filepath.Base(target))
		for _, ext := range []string{".middleware.js", ".middleware.mjs", ".gateway.js", ".gateway.mjs"} {
			if strings.HasSuffix(base, ext) {
				return target, true
			}
		}
		return "", false
	}
	b, err := os.ReadFile(filepath.Join(target, "package.json"))
	if err != nil {
		return "", false
	}
	var pkg struct {
		Main    string          `json:"main"`
		Exports json.RawMessage `json:"exports"`
		Magpie  struct {
			Middleware string `json:"middleware"`
			Gateway    string `json:"gateway"`
		} `json:"magpie"`
	}
	if json.Unmarshal(b, &pkg) != nil {
		return "", false
	}
	name := strings.TrimSpace(pkg.Magpie.Middleware)
	if name == "" {
		name = strings.TrimSpace(pkg.Magpie.Gateway)
	}
	if name == "" {
		return "", false
	}
	file = filepath.Join(target, filepath.FromSlash(name))
	return file, pkg.Main == "" && len(pkg.Exports) == 0
}

// ListStamp changes when plugins.json does.
func ListStamp() string { return listStamp() }

// OptionsExample is the options a plugin at target suggests, from its
// package.json's magpie.options: what its options editor starts from
// before any are set.
func OptionsExample(target string) map[string]any {
	b, err := os.ReadFile(filepath.Join(target, "package.json"))
	if err != nil {
		return nil
	}
	var pkg struct {
		Magpie struct {
			Options map[string]any `json:"options"`
		} `json:"magpie"`
	}
	if json.Unmarshal(b, &pkg) != nil {
		return nil
	}
	return pkg.Magpie.Options
}
