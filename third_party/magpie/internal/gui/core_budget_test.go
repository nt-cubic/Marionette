package gui

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

// What the window's core may weigh, in bytes of script: everything
// index.html loads that isn't a plugin's (assets/plugins/<id>/) and isn't
// the words (i18n.js). A page belongs in a plugin. Each page moved out
// lowers these to what is left, with about 1.5% room for the core's own work,
// so a page can't quietly grow back into the core; a change that needs more
// raises them and says why in its commit.
const (
	appScriptBudget  = 1_338_084 // app.js: 1,198,239 after Usage left (P2); 1,218,098 with the 10-15 fixes to pages still in the core; 1,232,466 with the provider editor's Advanced fold and every dialog holding its place, the core's own (10-16); 1,250,533 with the quota refresh kept through redraws, the core's own since the tray panel draws it too, plus 1.5% (#2397); 1,269,083 with Free first's option in the provider editor's ROUTINGS, which the Routing page reads too, plus 1.5%; 1,272,726 with the Providers page drawing its last list at once (#1519), the core's page, and the Routing trace's account names read without Object.entries (#1481), plus 1.5%; a dialog growing out of the row that opened it and folding back into it, the core's own since every page's dialogs open through it (10-20); Codex's and Claude Code's own settings moved from Settings into the agent's row on the Agents page, the core's own since the tray panel and the window both draw them (10-20); 1,314,432 with Settings › Privacy's record of what was masked (its summary, the records view and the local model's controls), which belongs to Settings, the core's own, plus 1.5%; 1,336,481 with a provider's Names & levels saying Provider in model names is Settings' and opening it there (joe), and Settings' Image recognition saying where a custom provider's model is marked (nexnull), both in the core's own pages, with no room added (10-23); 1,337,633 with Settings › Models › Replies' Intent classifier timeout (#2934, reyoung), Settings' own row, drawn by waitRow, which the Image generation timeout's row now shares, with no room added; 1,337,735 with the provider editor's link naming a partner's page by its own host (siteHost) instead of usemagpie.ai's /go link, the core's own editor, with no room added (10-23); 1,338,084 with Check in now saying what the press came to (claimed +credits, already today, not available yet) where it said "refreshed" (#3326), the check-in row the tray panel draws too, the core's own, with no room added (10-23)
	coreScriptBudget = 1_529_844 // every core script: 1,358,034 after Usage left; 1,380,003 with them; 1,405,937 with the fold; 1,426,102 with 10-16's core work (the GitHub list loading, the move to plugins, Settings › Long conversations), plus 1.5% (#2612); 1,447,368 with loadBack, the window's one state read per return, which every page needs and so is the core's own (1,446,845 before it, 155 bytes under), plus 1.5%; OpenCode Zen's card saying OpenCode tells no key its balance adds ~390 bytes inside it; 1,472,589 with the dialog that grows out of its row and the agents' own settings in their rows, on main's 10-20 core work (the Providers page's last list, the Routing trace's names), plus 1.5% (10-20); 1,506,192 with Settings › Privacy's record of what was masked, plus 1.5%; 1,529,393 with the Intent classifier timeout row (#2934), with no room added; 1,529,495 with the editor's partner link host, with no room added; 1,529,844 with Check in now's press summary (#3326), with no room added
)

func TestCoreScriptBudget(t *testing.T) {
	page, err := os.ReadFile("assets/index.html")
	if err != nil {
		t.Fatal(err)
	}
	total, n := 0, 0
	for _, m := range regexp.MustCompile(`<script src="([^"?]+)`).FindAllStringSubmatch(string(page), -1) {
		name := m[1]
		if strings.HasPrefix(name, "plugins/") || name == "i18n.js" {
			continue
		}
		b, err := os.ReadFile(filepath.Join("assets", name))
		if os.IsNotExist(err) {
			continue // made by the server as it serves the page (boot.js)
		}
		if err != nil {
			t.Fatal(err)
		}
		if name == "app.js" && len(b) > appScriptBudget {
			t.Errorf("app.js is %d bytes, over its budget of %d: put the page's code in its plugin, or raise the budget and say why", len(b), appScriptBudget)
		}
		total += len(b)
		n++
	}
	if n < 5 {
		t.Fatalf("found only %d core scripts in index.html", n)
	}
	if total > coreScriptBudget {
		t.Errorf("the core's scripts are %d bytes, over their budget of %d: put the page's code in its plugin, or raise the budget and say why", total, coreScriptBudget)
	}
}
