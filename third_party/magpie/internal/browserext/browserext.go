// Package browserext is Magpie Browser, the browser extension through which
// the agents magpie serves use the user's own browser: their tabs, signed
// in to the sites they use (gateway/browser.go relays the calls). magpie
// writes it to a folder of its own, pointed at its gateway, for the user to
// load in chrome://extensions.
package browserext

import (
	"bytes"
	"embed"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

// ID is the extension's ID in every Chromium browser, fixed by the public
// key in its manifest; the gateway takes a connection from this origin
// only.
const ID = "dmlhojidlfnkgamkekjhacgkpanbkeko"

// Origin is what the extension's requests carry as Origin.
const Origin = "chrome-extension://" + ID

//go:embed ext
var files embed.FS

// Version is the extension's version, from its manifest.
func Version() string {
	b, _ := files.ReadFile("ext/manifest.json")
	_, v, ok := bytes.Cut(b, []byte(`"version": "`))
	if !ok {
		return ""
	}
	v, _, _ = bytes.Cut(v, []byte(`"`))
	return string(v)
}

// File is one of the extension's files as magpie has it.
func File(name string) ([]byte, error) { return files.ReadFile("ext/" + name) }

// Write puts the extension in dir, pointed at the gateway at gatewayURL
// (http://127.0.0.1:3425), replacing what an older magpie wrote there.
// A file is written only when it changed, so a browser that has the folder
// loaded sees no change it has to reload for.
func Write(dir, gatewayURL string) error {
	ws := "ws" + strings.TrimPrefix(strings.TrimRight(gatewayURL, "/"), "http")
	if !strings.HasPrefix(ws, "ws://") && !strings.HasPrefix(ws, "wss://") {
		return fmt.Errorf("the gateway's address %q isn't http", gatewayURL)
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return fs.WalkDir(files, "ext", func(p string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		b, err := files.ReadFile(p)
		if err != nil {
			return err
		}
		name := strings.TrimPrefix(p, "ext/")
		if name == "config.js" {
			b = fmt.Appendf(nil, "// The gateway this extension talks to, written by magpie.\nexport const GATEWAY = %q;\n", ws)
		}
		to := filepath.Join(dir, filepath.FromSlash(name))
		if old, err := os.ReadFile(to); err == nil && bytes.Equal(old, b) {
			return nil
		}
		tmp := to + ".tmp"
		if err := os.WriteFile(tmp, b, 0o644); err != nil {
			return err
		}
		return os.Rename(tmp, to)
	})
}
