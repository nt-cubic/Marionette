package provider

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// gitSaidRe is whether the agent's folder is in a git repository, as the
// agent says it beside the folder: Claude Code's " - Is a git repository:
// false" (2.1.296; "Is directory a git repo: Yes" before), OpenCode's
// "  Is directory a git repo: yes".
var gitSaidRe = regexp.MustCompile(`(?mi)^\s*(?:- )?Is (?:a git repository|directory a git repo): (true|false|yes|no)\s*$`)

// GitSaid is the last "yes" or "no" an agent's text says of its folder
// being in a git repository, "" when it says neither.
func GitSaid(text string) string {
	m := gitSaidRe.FindAllStringSubmatch(text, -1)
	if len(m) == 0 {
		return ""
	}
	switch strings.ToLower(m[len(m)-1][1]) {
	case "true", "yes":
		return "yes"
	}
	return "no"
}

// GitOnDisk says whether dir, a folder on this computer, is in a git
// repository: a .git in it or in a folder above it. "" when dir isn't a
// folder here (an agent on another computer, through a shared magpie):
// unknown, never "no".
func GitOnDisk(dir string) string {
	if !filepath.IsAbs(dir) {
		return ""
	}
	if fi, err := os.Stat(dir); err != nil || !fi.IsDir() {
		return ""
	}
	for d := filepath.Clean(dir); ; {
		if _, err := os.Stat(filepath.Join(d, ".git")); err == nil {
			return "yes"
		}
		up := filepath.Dir(d)
		if up == d {
			return "no"
		}
		d = up
	}
}
