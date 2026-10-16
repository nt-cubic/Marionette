package agent

import (
	"encoding/base64"
	"encoding/json"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"

	"github.com/yetone/magpie/internal/gateway"
)

// LodyScheme opens Lody's local-machine provider confirmation dialog.
const LodyScheme = "lody://provider/import?"

// LodyLink offers all supported runtimes. Lody accepts only loopback HTTP
// and its public attribution token, never a LAN key or a vendor credential.
func LodyLink(base string) string {
	u, err := url.Parse(base)
	if err != nil || u.Scheme != "http" || u.User != nil || u.ForceQuery || u.RawQuery != "" || u.Fragment != "" || (u.Path != "" && u.Path != "/") {
		return ""
	}
	switch u.Hostname() {
	case "127.0.0.1", "localhost", "::1":
	default:
		return ""
	}
	if strings.HasSuffix(u.Host, ":") || (u.Hostname() == "::1" && !strings.HasPrefix(u.Host, "[::1]")) {
		return ""
	}
	host := u.Hostname()
	if host == "::1" {
		host = "[::1]"
	}
	if port := u.Port(); port != "" {
		n, err := strconv.Atoi(port)
		if err != nil || n < 1 || n > 65535 {
			return ""
		}
		if n != 80 {
			host += ":" + strconv.Itoa(n)
		}
	}
	base = "http://" + host
	type endpoint struct {
		Protocol  string   `json:"protocol"`
		BaseURL   string   `json:"baseUrl"`
		Targets   []string `json:"targets"`
		ModelsURL string   `json:"modelsUrl"`
	}
	payload := struct {
		Kind      string            `json:"kind"`
		ID        string            `json:"id"`
		Name      string            `json:"name"`
		Auth      map[string]string `json:"auth"`
		Endpoints []endpoint        `json:"endpoints"`
	}{
		Kind: "custom", ID: "magpie", Name: "Magpie",
		Auth: map[string]string{"method": "apiKey", "apiKey": gateway.TokenFor("lody")},
		Endpoints: []endpoint{
			{"anthropic-messages", base, []string{"claude-code"}, base + "/v1/models"},
			{"openai-responses", base + "/v1", []string{"codex"}, base + "/v1/models"},
			{"openai-chat", base + "/v1", []string{"pi", "dsh", "kimi-code", "grok", "bub"}, base + "/v1/models"},
		},
	}
	b, _ := json.Marshal(payload)
	return LodyScheme + "v=1&data=" + base64.RawURLEncoding.EncodeToString(b)
}

// lodyMarks are entries Lody makes in its home folder (~/.lody, or
// ~/.lody-oss for its OSS build) once it has run: a folder of that name
// another tool made, with none of them in it, isn't Lody.
var lodyMarks = []string{"workspace-catalog.json", "loro-repo", "agent-binaries", "code-collab-v2"}

// lodyNames are the names Lody's builds go by: its app bundle on a Mac and
// its data folder under the OS's config folder (Electron's userData:
// ~/Library/Application Support/Lody, %APPDATA%\Lody, ~/.config/Lody).
var lodyNames = []string{"Lody", "Lody OSS", "Lody Nightly"}

// lodyBundle reports whether an app bundle in folders is one of Lody's:
// ai.lody.desktop (or another ai.lody. build), or a plist that isn't XML,
// which says nothing either way. A Lody.app of another vendor's isn't.
func lodyBundle(folders []string) bool {
	for _, dir := range folders {
		for _, name := range lodyNames {
			b, err := os.ReadFile(filepath.Join(dir, name+".app", "Contents", "Info.plist"))
			if err != nil {
				continue
			}
			if id := plistIDRe.FindSubmatch(b); id == nil || strings.HasPrefix(strings.TrimSpace(string(id[1])), "ai.lody.") {
				return true
			}
		}
	}
	return false
}

func lody(home string) *Agent {
	homes := []string{filepath.Join(home, ".lody"), filepath.Join(home, ".lody-oss")}
	var dirs []string
	if cfg, err := os.UserConfigDir(); err == nil {
		for _, name := range lodyNames {
			dirs = append(dirs, filepath.Join(cfg, name))
		}
	}
	return &Agent{
		ID: "lody", Name: "Lody", Icon: "lody", UA: []string{"lody"},
		detect: func() bool {
			for _, dir := range homes {
				for _, m := range lodyMarks {
					if _, err := os.Stat(filepath.Join(dir, m)); err == nil {
						return true
					}
				}
			}
			for _, dir := range dirs {
				if isDir(dir) {
					return true
				}
			}
			return runtime.GOOS == "darwin" && lodyBundle(macAppFolders(home))
		},
		Import: func() string { return LodyLink(gateway.URL()) },
		// There is no public status API. Never infer import success from a
		// click, generated runtime files, or Lody's private document store.
	}
}
