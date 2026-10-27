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
	appScriptBudget  = 1_325_845 // app.js: 1,198,239 after Usage left (P2); 1,218,098 with the 10-15 fixes to pages still in the core; 1,232,466 with the provider editor's Advanced fold and every dialog holding its place, the core's own (10-16); 1,250,533 with the quota refresh kept through redraws, the core's own since the tray panel draws it too, plus 1.5% (#2397); 1,269,083 with Free first's option in the provider editor's ROUTINGS, which the Routing page reads too, plus 1.5%; 1,272,726 with the Providers page drawing its last list at once (#1519), the core's page, and the Routing trace's account names read without Object.entries (#1481), plus 1.5%; a dialog growing out of the row that opened it and folding back into it, the core's own since every page's dialogs open through it (10-20); Codex's and Claude Code's own settings moved from Settings into the agent's row on the Agents page, the core's own since the tray panel and the window both draw them (10-20); 1,314,432 with Settings › Privacy's record of what was masked (its summary, the records view and the local model's controls), which belongs to Settings, the core's own, plus 1.5%; 1,336,481 with a provider's Names & levels saying Provider in model names is Settings' and opening it there (joe), and Settings' Image recognition saying where a custom provider's model is marked (nexnull), both in the core's own pages, with no room added (10-23); 1,337,633 with Settings › Models › Replies' Intent classifier timeout (#2934, reyoung), Settings' own row, drawn by waitRow, which the Image generation timeout's row now shares, with no room added; 1,337,735 with the provider editor's link naming a partner's page by its own host (siteHost) instead of usemagpie.ai's /go link, the core's own editor, with no room added (10-23); 1,338,084 with Check in now saying what the press came to (claimed +credits, already today, not available yet) where it said "refreshed" (#3326), the check-in row the tray panel draws too, the core's own, with no room added (10-23); 1,338,929 with a new partner's tile keeping a small dot until it is opened, in the add sheet, the core's own, with no room added (10-23); 1,343,324 with the sidebar's open-page row marked by one shape that moves on a spring (navTopGo), the window's own shell, which every page sits beside, with no room added (10-23); 1,343,465 with a dialog kept centred while its content comes in, until it is pressed (holdModal, every page's dialogs), the core's own, with no room added (10-24); 1,317,486 with what masking did moved out to Insights › Masking (plugins/redact), the budget lowered to it (10-24); 1,321,336 with Codex's one subagent model, its card's square saying plainly what each choice does and a row with a name in its sheet, where two squares read alike (the owner), the Agents page's, which the tray panel draws too, the core's own, with no room added (10-24); 1,321,424 with Settings › Models' Image generation MCP server row beside Image recognition's (JonahFu on X), the two drawn by one helper (mcpRow), Settings' own, with no room added (10-25); 1,321,994 with a provider of the user's set never to rest for a limit (zoecats), the provider editor's Advanced row, the core's own editor, with no room added (10-25); 1,322,289 with Settings › Privacy's Keep a key's form under Mask secrets (Charles Cao on X), Settings' own row, with no room added (10-25); 1,322,537 with no Codex squares beside its named settings rows (the owner), with no room added (10-25); 1,323,401 with the tray panel drawing its last allowances at once and asking for them beside the state (#3160, #3134), the tray panel's, the core's own, and the 4 bytes 84c50f032 added, with no room added (10-25); 1,326,105 with many releases' notes merged into one list by kind, each line with its version (the owner), in the update sheet and What's new, with no room added (10-25); 1,321,319 with Settings › Observability moved out to its plugin (plugins/otel/page.js) and its Langfuse setup there (an X user), the budget lowered to it (10-26); a quota card whose window is used but names no reset no longer saying "Not used yet" (windy on Discord) raised it to 1,321,347, and back to 1,321,319 with Settings' model pickers sharing one row helper (modelOpt) where six kept their own copy, no raise (10-26); 1,325,845 with every slow page's skeletons getting magpie's bird hopping bar to bar (skelBird, the owner after the Plugins page's README), one bird for the window over whatever page or dialog is loading, the core's own, with no room added (10-26)
	coreScriptBudget = 1_541_967 // every core script: 1,358,034 after Usage left; 1,380,003 with them; 1,405,937 with the fold; 1,426,102 with 10-16's core work (the GitHub list loading, the move to plugins, Settings › Long conversations), plus 1.5% (#2612); 1,447,368 with loadBack, the window's one state read per return, which every page needs and so is the core's own (1,446,845 before it, 155 bytes under), plus 1.5%; OpenCode Zen's card saying OpenCode tells no key its balance adds ~390 bytes inside it; 1,472,589 with the dialog that grows out of its row and the agents' own settings in their rows, on main's 10-20 core work (the Providers page's last list, the Routing trace's names), plus 1.5% (10-20); 1,506,192 with Settings › Privacy's record of what was masked, plus 1.5%; 1,529,393 with the Intent classifier timeout row (#2934), with no room added; 1,529,495 with the editor's partner link host, with no room added; 1,529,844 with Check in now's press summary (#3326), with no room added; 1,530,689 with the new partner tile's dot, with no room added; 1,535,084 with the sidebar's spring, with no room added; 1,549,478 with the context window card's map (tiles by tokens, the default look, the block grid a switch away) and its text view showing an inline image, in context.js, which the Routing page draws as well as Usage, so the core's own, with no room added (10-24); 1,550,768 with a usage card's refresh opening after its text (#840's card, which the tray panel draws too), the Add key spinner, and a dialog kept centred while its content comes in, with no room added (10-24); 1,524,789 with what masking did moved out to Insights › Masking, lowered to it; 1,528,525 with Codex's one subagent model, with no room added (10-24); 1,528,972 with the Image generation MCP server row (JonahFu on X), with no room added (10-25); 1,529,542 with the limit-rest row (zoecats), with no room added (10-25); 1,529,837 with Keep a key's form (Charles Cao on X), with no room added (10-25); 1,530,050 with no Codex squares beside its named settings rows (the owner), with no room added (10-25); 1,530,631 with a plugin's page growing to its README on the dialog's spring instead of in one jump (the owner), the Plugins page's, in plugins.js, with no room added (10-25); 1,533,445 with the context map counting its tokens on as a live request grows, a tile pointed at keeping its tip, a tool's gathered results saying how many and the small files their folder (the owner, after flohoeller's map on X), in context.js, which the Routing page draws as well as Usage, and the 4 bytes 84c50f032 added to app.js (Settings' Version row) without a raise, with no room added (10-25); 1,534,312 with the tray panel's last allowances drawn at once (#3160, #3134), with no room added (10-25); 1,537,016 with many releases' notes merged into one list by kind (the owner), in app.js, with no room added (10-25); 1,542,244 with a plugin's README loading under magpie's bird bouncing down its lines instead of pulsing bars (the owner), the Plugins page's, in plugins.js, with no room added (10-26); 1,542,418 with a page plugin's sidebar tab coming and going as it is installed, switched on or off, or removed, instead of on the Plugins page's next load (the owner), in plugins.js, with no room added (10-26); 1,537,645 with Settings › Observability moved out to plugins/otel, lowered to it (10-26); 1,537,673 with a used window with no reset not "Not used yet" (windy on Discord), and back to 1,537,645 with Settings' model pickers sharing modelOpt (10-26); 1,541,967 with every slow page's skeletons getting magpie's bird (skelBird, the owner), in app.js, the README's bird sharing its drawing and styles, with no room added (10-26)
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
