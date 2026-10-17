package cli

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/middleware"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/provider"
	"golang.org/x/term"
)

const pluginUsage = `usage: magpie plugin [list] [--json]
       magpie plugin add <npm | git | path>        install an OpenCode provider plugin, a pi package or a gateway hook/middleware (opencode-gemini-auth, pi-antigravity, github:owner/repo, ./my-plugin.js, ./alias.middleware.js, ./recall.hooks.ts)
       magpie plugin rm <name>                     remove one
       magpie plugin update                        install the newest version of each
       magpie plugin on|off <name>                 turn one on or off
       magpie plugin options <name> [<json> | off] show or set what a plugin is handed (a middleware's ctx.options)
       magpie plugin use <name> <provider>         have this plugin serve a provider another plugin signs in to as well
       magpie plugin login <provider> [<method>]   sign in to a provider a plugin adds
       magpie plugin logout <provider>             forget the sign-in
       magpie plugin move|migrate <subscription>   run a built-in subscription's accounts on its community plugin
       magpie plugin builtin [on|off <id>]         list magpie's built-in plugins (Sessions, Library, routing groups…), or turn one on or off
       magpie plugin preset essentials|all         start the built-in plugins from the Essentials preset, or from all of them
       magpie plugin dump [--json]                 what plugins.yml comes to: each built-in plugin, its config, and the layer each value is from`

// pluginCmd: `magpie plugin …` — OpenCode's provider plugins and pi's
// packages, which sign in to a subscription and carry its requests
// (internal/plugin), and gateway middleware (internal/middleware).
func pluginCmd(args []string) error {
	sub := "list"
	if len(args) > 1 {
		sub = args[1]
	}
	rest := args[min(len(args), 2):]
	ctx, stop := interruptContext()
	defer stop()
	switch sub {
	case "list", "ls", "--json":
		return listPlugins(ctx, sub == "--json" || len(rest) > 0 && rest[0] == "--json")
	case "add", "install":
		if len(rest) != 1 {
			return errors.New(pluginUsage)
		}
		if !plugin.IsPath(rest[0]) && !plugin.HasBun() {
			fmt.Println(muted.Render("Downloading Bun " + plugin.BunInUse() + ", which plugins run on…"))
		}
		e, err := provider.AddPlugin(ctx, rest[0])
		if err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), "added", e.Spec)
		// a deprecated built-in it serves, not signed in to, is its now
		provider.HandOver(ctx, false)
		return listPlugins(ctx, false)
	case "rm", "remove", "uninstall":
		if len(rest) != 1 {
			return errors.New(pluginUsage)
		}
		rest[0] = installedName(rest[0])
		moved := provider.MovedOnto(rest[0])
		if err := provider.RemovePlugin(ctx, rest[0]); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), "removed", rest[0])
		for _, id := range moved {
			fmt.Println(faint.Render("  " + id + " is off until the plugin is added again; its accounts are kept"))
		}
		return nil
	case "update", "upgrade":
		if err := plugin.Update(ctx); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), "plugins updated")
		provider.HandOver(ctx, false)
		return listPlugins(ctx, false)
	case "on", "off":
		if len(rest) != 1 {
			return errors.New(pluginUsage)
		}
		rest[0] = installedName(rest[0])
		if err := provider.SetPluginOff(ctx, rest[0], sub == "off"); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), rest[0], "is", sub)
		if sub == "off" {
			for _, id := range provider.MovedOnto(rest[0]) {
				fmt.Println(faint.Render("  " + id + " is off with it; its accounts are kept"))
			}
		}
		return nil
	case "options", "config":
		if len(rest) < 1 || len(rest) > 2 {
			return errors.New(pluginUsage)
		}
		return pluginOptions(rest[0], rest[1:])
	case "use", "prefer":
		if len(rest) != 2 {
			return errors.New(pluginUsage)
		}
		rest[0] = installedName(rest[0])
		if err := plugin.Prefer(rest[0], rest[1]); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), rest[0], "serves", rest[1])
		return listPlugins(ctx, false)
	case "login", "signin":
		if len(rest) < 1 || len(rest) > 2 {
			return errors.New(pluginUsage)
		}
		method := ""
		if len(rest) == 2 {
			method = rest[1]
		}
		return pluginLogin(ctx, rest[0], method)
	case "logout", "signout":
		if len(rest) != 1 {
			return errors.New(pluginUsage)
		}
		pp, err := pluginProvider(ctx, rest[0])
		if err != nil {
			return err
		}
		if err := plugin.SignOut(ctx, pp.ID, ""); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), "signed out of", pp.Name)
		return nil
	case "move", "migrate":
		if len(rest) != 1 {
			return errors.New(pluginUsage)
		}
		if !provider.Movable(rest[0]) {
			return fmt.Errorf("%s has no plugin to move to", rest[0])
		}
		if !plugin.HasBun() {
			fmt.Println(muted.Render("Downloading Bun " + plugin.BunInUse() + ", which plugins run on…"))
		}
		if err := provider.Move(ctx, rest[0]); err != nil {
			return fmt.Errorf("%s stays built-in: %w", rest[0], err)
		}
		fmt.Println(green.Render("✓"), rest[0], "runs on", provider.MovePackage(rest[0]))
		return nil
	case "move-back", "moveback", "unmigrate":
		// the built-ins are retired: there is no going back to one
		return errors.New("magpie's built-in subscriptions are retired: their community plugins run them")
	case "builtin", "builtins", "built-in":
		return builtinCmd(rest)
	case "preset":
		if len(rest) != 1 {
			return errors.New(pluginUsage)
		}
		if err := pluginrt.SetPreset(rest[0]); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), "built-in plugins start from", rest[0])
		return builtinCmd(nil)
	case "dump":
		return dumpPlugins(len(rest) > 0 && rest[0] == "--json")
	case "help", "-h", "--help":
		fmt.Println(pluginUsage)
		return nil
	}
	return errors.New(pluginUsage)
}

func listPlugins(ctx context.Context, asJSON bool) error {
	l := plugin.Load()
	if len(l.Plugins) == 0 {
		if asJSON {
			fmt.Println("[]")
			return nil
		}
		fmt.Println("No plugins. Add one: magpie plugin add <npm package | git repo | path>")
		return nil
	}
	loaded, lerr := plugin.Plugins(ctx)
	errs := map[string]string{}
	for _, p := range loaded {
		errs[p.Spec] = p.Error
	}
	clashes := plugin.Clashes(loaded)
	ps, perr := plugin.Providers(ctx)
	mws := middleware.States()
	var pts []plugin.PluginTools
	for _, p := range loaded {
		if len(p.Tools) > 0 {
			pts = append(pts, plugin.PluginTools{Spec: p.Spec, Tools: p.Tools})
		}
	}
	toolNames := gateway.ToolNames(pts)
	if asJSON {
		b, _ := json.MarshalIndent(map[string]any{"plugins": l.Plugins, "loaded": loaded, "providers": ps, "middleware": mws}, "", "  ")
		fmt.Println(string(b))
		return nil
	}
	// a built-in with accounts the plugin could run: not signed in to the
	// plugin is its normal state, not something to fix
	onBuiltin := map[string]provider.MoveCandidate{}
	for _, c := range provider.MoveCandidates() {
		onBuiltin[c.ID] = c
	}
	for _, e := range l.Plugins {
		state := green.Render("on")
		switch {
		case e.Off:
			state = muted.Render("off")
		case errs[e.Spec] != "":
			state = "failed: " + errs[e.Spec]
		}
		what := bold.Render(e.Spec)
		// a git repository's package is named by its own package.json
		if n := plugin.Name(e.Spec); plugin.IsGit(e.Spec) && n != e.Spec {
			what += " " + muted.Render("("+strings.TrimSpace(n+" "+plugin.Installed(e.Spec))+")")
		}
		fmt.Printf("%s  %s\n", what, state)
		if plugin.Gateway(plugin.Target(e.Spec)) != "" {
			fmt.Println("  " + muted.Render("async gateway hook") + "  onRequest")
		}
		// its calls are counted in the gateway's process, not this one
		if m, ok := mws[e.Spec]; ok && !e.Off {
			if m.Error != "" {
				fmt.Printf("  %s  %s\n", muted.Render("gateway middleware"), amber.Render("didn't load: "+m.Error))
			} else {
				fmt.Printf("  %s  %s\n", muted.Render("gateway middleware"), mwPoints(m))
			}
		}
		// the tools it gives agents, by their names on the plugin tools
		// MCP server (the host loads no plugin that is off, so one has
		// none), and the page it adds to the window
		if ts := toolsLine(toolNames[e.Spec]); ts != "" {
			fmt.Printf("  %s  %s\n", muted.Render("tools"), ts)
		}
		if pg, _ := plugin.WindowPage(plugin.Target(e.Spec)); pg != nil {
			fmt.Printf("  %s  %s\n", muted.Render("page"), pageLine(pg))
		}
		for _, p := range ps {
			if p.Spec != e.Spec {
				continue
			}
			who := muted.Render("not signed in · magpie plugin login " + p.ID)
			if c, ok := onBuiltin[p.ID]; ok && !p.SignedIn {
				who = muted.Render(fmt.Sprintf("runs on magpie's built-in (%d accounts) · magpie plugin move %s", c.Accounts, p.ID))
			}
			if p.SignedIn {
				who = green.Render("signed in")
				if p.AccountID != "" {
					who += " as " + p.AccountID
				}
			}
			fmt.Printf("  %s %s  %s\n", provider.PluginID(p.ID), muted.Render("("+p.Name+", "+strconv.Itoa(len(p.Models))+" models)"), who)
		}
		// a provider another plugin signs in to as well: one of them runs
		// it, and the other's is said, not left out
		for _, c := range clashes[e.Spec] {
			if e.Off || errs[e.Spec] != "" {
				continue
			}
			if c.By == e.Spec {
				fmt.Printf("  %s  %s\n", provider.PluginID(c.ID), muted.Render("also signed in to by "+strings.Join(c.With, ", ")+"; this one serves it"))
				continue
			}
			fmt.Printf("  %s  %s\n", provider.PluginID(c.ID), amber.Render("served by "+c.By+", which signs in to it too · magpie plugin use "+e.Spec+" "+c.ID))
		}
	}
	if lerr != nil {
		return lerr
	}
	return perr
}

// pluginOptions prints a plugin's options, or sets them from a JSON
// object; off takes them away. With none set, it prints what its
// package suggests.
// installedName is the package name of the plugin a short name, as the
// community's READMEs write it (think-tags for
// @magpie-community/middleware-think-tags), stands for; a name a plugin
// has itself, or one no plugin has, is kept as it is.
func installedName(name string) string {
	ps := plugin.Load().Plugins
	for _, x := range ps {
		if plugin.Name(x.Spec) == name || x.Spec == name {
			return name
		}
	}
	for _, x := range ps {
		if plugin.Called(plugin.Name(x.Spec), name) {
			return plugin.Name(x.Spec)
		}
	}
	return name
}

func pluginOptions(name string, set []string) error {
	var e *plugin.Entry
	ps := plugin.Load().Plugins
	// the one added as name exactly, else the one of that package
	for i, x := range ps {
		if e == nil && x.Spec == name {
			e = &ps[i]
		}
	}
	for i, x := range ps {
		if e == nil && plugin.Name(x.Spec) == name {
			e = &ps[i]
		}
	}
	// a short name, as the community's READMEs write it
	for i, x := range ps {
		if e == nil && plugin.Called(plugin.Name(x.Spec), name) {
			e = &ps[i]
		}
	}
	if e == nil {
		return fmt.Errorf("no plugin %q", name)
	}
	if len(set) == 0 {
		if len(e.Options) == 0 {
			ex := plugin.OptionsExample(plugin.Target(e.Spec))
			if ex == nil {
				fmt.Println(muted.Render("No options set."))
				return nil
			}
			b, _ := json.MarshalIndent(ex, "", "  ")
			fmt.Println(muted.Render("No options set. Its package suggests:"))
			fmt.Println(string(b))
			return nil
		}
		b, _ := json.MarshalIndent(e.Options, "", "  ")
		fmt.Println(string(b))
		return nil
	}
	var opts map[string]any
	if set[0] != "off" {
		if err := json.Unmarshal([]byte(set[0]), &opts); err != nil || opts == nil {
			return errors.New(`options are a JSON object ('{"key": "value"}'), or off`)
		}
	}
	if err := plugin.SetOptions(e.Spec, opts); err != nil {
		return err
	}
	if opts == nil {
		fmt.Println(green.Render("✓"), name, "has no options")
	} else {
		fmt.Println(green.Render("✓"), name, "options set")
	}
	return nil
}

// pluginProvider is the plugins' provider named by OpenCode's id or
// magpie's.
func pluginProvider(ctx context.Context, name string) (plugin.Provider, error) {
	ps, err := plugin.Providers(ctx)
	if err != nil {
		return plugin.Provider{}, err
	}
	for _, p := range ps {
		if p.ID == name || provider.PluginID(p.ID) == name || strings.EqualFold(p.Name, name) {
			return p, nil
		}
	}
	var ids []string
	for _, p := range ps {
		ids = append(ids, p.ID)
	}
	if len(ids) == 0 {
		return plugin.Provider{}, fmt.Errorf("no plugin signs in to %q — add one: magpie plugin add <npm package>", name)
	}
	return plugin.Provider{}, fmt.Errorf("no plugin signs in to %q; they sign in to %s", name, strings.Join(ids, ", "))
}

// pluginSignInCLI finds and installs the vendor CLI a plugin's sign-in
// runs through; tests stand in for it.
var pluginSignInCLI = struct {
	missing func(id string) string
	install func(ctx context.Context, id string) error
}{provider.PluginCLIMissing, provider.InstallPluginCLI}

// pluginLogin signs in to a plugin's provider as OpenCode's `auth login`
// does: the method, its questions, then a browser (and a code pasted
// back) or a key.
func pluginLogin(ctx context.Context, name, method string) error {
	pp, err := pluginProvider(ctx, name)
	if err != nil {
		return err
	}
	if len(pp.Methods) == 0 {
		return fmt.Errorf("%s's plugin has no way to sign in", pp.Name)
	}
	m := 0
	switch {
	case method != "":
		m = -1
		for i, x := range pp.Methods {
			if strconv.Itoa(i+1) == method || strings.EqualFold(x.Label, method) || x.Type == method {
				m = i
				break
			}
		}
		if m < 0 {
			return fmt.Errorf("%s has no sign-in method %q", pp.Name, method)
		}
	case len(pp.Methods) > 1:
		fmt.Println("How do you sign in to", pp.Name+"?")
		for i, x := range pp.Methods {
			fmt.Printf("  %d. %s\n", i+1, x.Label)
		}
		n, err := askNumber(len(pp.Methods))
		if err != nil {
			return err
		}
		m = n
	}
	if pp.Methods[m].Type != "api" {
		// the vendor's CLI it signs in with, installed first as the
		// window's sign-in installs it (#2223: Grok Build in Docker)
		if name := pluginSignInCLI.missing(pp.ID); name != "" {
			fmt.Println(amber.Render("!"), name, "isn't installed here: "+pp.Name+" signs in through it. Installing it, as its vendor's installer does…")
			if err := pluginSignInCLI.install(ctx, pp.ID); err != nil {
				return err
			}
			fmt.Println(green.Render("✓"), name, "installed")
		}
	}
	inputs := map[string]string{}
	for {
		q, err := plugin.NextPrompt(ctx, pp.ID, m, inputs)
		if err != nil {
			return err
		}
		if q == nil {
			break
		}
		for {
			v, err := askPrompt(q)
			if err != nil {
				return err
			}
			msg, err := plugin.Validate(ctx, pp.ID, m, q.Key, v)
			if err != nil {
				return err
			}
			if msg == "" {
				inputs[q.Key] = v
				break
			}
			fmt.Println(msg)
		}
	}
	// signed in through the proxy its requests will go through (#1663)
	ctx = provider.Via(ctx, provider.PluginID(pp.ID))
	var saved plugin.Saved
	if pp.Methods[m].Type == "api" {
		key, err := secret("key", keyPrompt(pp.Name, pp.Methods[m]), false)
		if err != nil {
			return err
		}
		if saved, err = plugin.APIKey(ctx, pp.ID, m, inputs, key, plugin.NewAccount); err != nil {
			return err
		}
	} else {
		a, err := plugin.Authorize(ctx, pp.ID, m, inputs, plugin.NewAccount)
		if err != nil {
			return err
		}
		if a.URL != "" {
			fmt.Println("Sign in in your browser. If it didn't open, go to:")
			fmt.Println(faint.Render(a.URL))
			openInBrowser(a.URL)
		}
		if a.Instructions != "" {
			fmt.Println(a.Instructions)
		}
		code := ""
		if a.Method == "code" {
			fmt.Print("Code: ")
			line, err := stdin.ReadString('\n')
			if err != nil && line == "" {
				return err
			}
			code = strings.TrimSpace(line)
		}
		wait, cancel := context.WithTimeout(ctx, 10*time.Minute)
		defer cancel()
		if a.Method != "code" && provider.PluginPastesCallback(pp.ID, a.URL) {
			// a browser on another computer (magpie on a server or in
			// Docker) ends on a page that won't load: its address finishes it
			fmt.Println("If the page the browser ends on won't load (magpie on a server or in Docker), paste its whole address here and press Enter:")
			go func() {
				for wait.Err() == nil {
					line, err := stdin.ReadString('\n')
					if strings.TrimSpace(line) != "" && wait.Err() == nil {
						if next, err := provider.PastePluginCallback(wait, a.URL, line); err != nil {
							fmt.Println(err)
						} else if next != "" {
							fmt.Println("Go on signing in at:")
							fmt.Println(faint.Render(next))
						}
					}
					if err != nil {
						return
					}
				}
			}()
		}
		if saved, err = plugin.Finish(wait, a.Session, code); err != nil {
			return err
		}
	}
	// as the window's sign-in does: its lapsed mark goes and, removed from
	// magpie, it comes back
	id := provider.PluginSignedIn(saved)
	fmt.Println(green.Render("✓"), "signed in to", pp.Name, muted.Render("· its models are "+id+"/<model>"))
	return nil
}

// keyPrompt asks an "api" method's key: its title, then the hint the
// plugin gives (its placeholder), as a question's is.
func keyPrompt(name string, m plugin.Method) string {
	p := m.KeyTitle(name)
	if m.Placeholder != "" {
		p += " " + muted.Render("("+m.Placeholder+")")
	}
	return p + ": "
}

func askNumber(n int) (int, error) {
	for {
		fmt.Print("> ")
		line, err := stdin.ReadString('\n')
		if i, e := strconv.Atoi(strings.TrimSpace(line)); e == nil && i >= 1 && i <= n {
			return i - 1, nil
		}
		if err != nil {
			return 0, err
		}
		fmt.Printf("A number from 1 to %d\n", n)
	}
}

// promptTerminal and promptHidden read a hidden answer; tests stand in for
// the terminal.
var (
	promptTerminal = term.IsTerminal
	promptHidden   = term.ReadPassword
)

func askPrompt(q *plugin.Prompt) (string, error) {
	if q.Type == "select" {
		fmt.Println(q.Message)
		for i, o := range q.Options {
			hint := ""
			if o.Hint != "" {
				hint = muted.Render("  " + o.Hint)
			}
			fmt.Printf("  %d. %s%s\n", i+1, o.Label, hint)
		}
		i, err := askNumber(len(q.Options))
		if err != nil {
			return "", err
		}
		return q.Options[i].Value, nil
	}
	p := q.Message
	if q.Placeholder != "" {
		p += " " + muted.Render("("+q.Placeholder+")")
	}
	fmt.Print(p + " ")
	// a "password" question (a token, a PAT) is typed unseen on a
	// terminal, as an API key is (#1982); piped in, it is a line
	if q.Type == "password" && promptTerminal(int(os.Stdin.Fd())) {
		b, err := promptHidden(int(os.Stdin.Fd()))
		fmt.Println()
		return strings.TrimSpace(string(b)), err
	}
	line, err := stdin.ReadString('\n')
	if err != nil && line == "" {
		return "", err
	}
	return strings.TrimSpace(line), nil
}

// mwPoints is what a gateway module serves, as the Plugins page's mwLine
// tells it: its hooks, the routings its strategies serve, and whether it
// has onUsage.
func mwPoints(m middleware.State) string {
	var parts []string
	if len(m.Hooks) > 0 {
		parts = append(parts, strings.Join(m.Hooks, ", "))
	}
	if len(m.Strategies) > 0 {
		parts = append(parts, "routings "+strings.Join(m.Strategies, ", "))
	}
	if m.Usage {
		parts = append(parts, "onUsage")
	}
	return strings.Join(parts, " · ")
}

// toolsLine is a plugin's tools by their MCP names, sorted.
func toolsLine(names map[string]string) string {
	var out []string
	for _, n := range names {
		out = append(out, n)
	}
	sort.Strings(out)
	return strings.Join(out, ", ")
}

// pageLine is a plugin's page: its title and what it may read of magpie's.
func pageLine(pg *plugin.PageSpec) string {
	reads := "reads nothing"
	if len(pg.Reads) > 0 {
		reads = "reads " + strings.Join(pg.Reads, ", ")
	}
	return pg.Title + " " + muted.Render("("+reads+")")
}
