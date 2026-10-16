package plugin

import "testing"

// #2229: plugins.json travels between machines in a backup or a sync, so
// a folder plugin's path written on macOS or Linux is a path on Windows
// too, and a Windows one is a path off Windows, or the restoring machine
// asks bun to install it as an npm package.
func TestIsPathWhicheverSystemWroteIt(t *testing.T) {
	for spec, want := range map[string]bool{
		"/Users/me/plugin":                     true,
		"/home/me/plugin/index.js":             true,
		`\Users\me\plugin`:                     true,
		`\\server\share\plugin`:                true,
		`C:\plugins\x`:                         true,
		"D:/plugins/x":                         true,
		"file:///Users/me/plugin":              true,
		"./my-plugin.js":                       true,
		"opencode-gemini-auth":                 false,
		"@magpie-community/opencode-kiro-auth": false,
		"owner/repo":                           false,
		"github:owner/repo":                    false,
	} {
		if got := IsPath(spec); got != want {
			t.Errorf("IsPath(%q) = %v, want %v", spec, got, want)
		}
	}
}
