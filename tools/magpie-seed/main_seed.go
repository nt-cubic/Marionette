//go:build marionette_seed

// Marionette engine seed — the slim gateway-only magpie.
//
// Marionette embeds a magpie build as the factory engine seed and runs it
// exclusively as `magpie serve` (the model-share gateway on /v1). Stock
// magpie is a full multi-agent manager: `internal/cli.Main` keeps a dozen
// agent installers, the TUI, session importers, OAuth flows, WebDAV sync…
// reachable from main, and the Go linker keeps every byte of it (58MB raw /
// 19.6MB UPX at v0.1.1877). This main bypasses internal/cli entirely and
// mounts only the plugins Marionette's model-share actually uses — measured
// ~38MB raw / ~12MB UPX, a 7.6MB smaller shipped exe.
//
// The seed still talks the full magpie gateway surface: /v1/models health,
// OpenAI + Anthropic endpoints, provider catalog, marionette cross-use
// routes, /v1/magpie/quotas. Custom sources and preset keys are registered
// through a slim `provider add|set|key|rm|refresh|models` that writes
// providers.json via internal/provider — the same package serve already
// imports. Agent management, TUI, OAuth, and `magpie quota --json` stay
// out (quotas go over HTTP /v1/magpie/quotas).
//
// Kept out on purpose, with the reason:
//   - agent.KeepCodexDaemonCurrent / KeepCodexRolloutsPortable — they drag
//     the whole internal/agent manager back into the binary; catalog
//     freshness still runs via catalog.KeepFresh + provider.FetchNew.
//   - 11 of 14 built-in plugins — no consumer on the Marionette side.
//   - `magpie quota --json` CLI — Marionette reads quotas over HTTP
//     (engine.rs: /v1/magpie/quotas first, CLI only as a fallback when the
//     gateway is down, in which case the seed isn't running either).
//
// Maintenance: this file is applied by tools\import-magpie.ps1 after every
// upstream import (the vendor tree is wiped and re-copied). Upstream
// refactors of the startup sequence surface as compile errors here — loud,
// never silent. Review tools\README.md ("magpie seed") when touching this.
package main

import (
	"context"
	"fmt"
	"os"
	"slices"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/netproxy"
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/stats"

	// Only the plugins Marionette's model-share consumes. The gateway
	// plugin itself, the cross-use routing the bridges write into, and the
	// quota endpoint the usage panel polls. Adding one here costs what its
	// dependency tree costs — measure before growing this list.
	_ "github.com/yetone/magpie/plugins/gateway"
	_ "github.com/yetone/magpie/plugins/routing"
	_ "github.com/yetone/magpie/plugins/usage"
)

// set at build time: -ldflags "-X main.version=…" (build-portable.bat).
var version = "dev"

func main() {
	args := os.Args[1:]
	if len(args) > 0 {
		switch args[0] {
		case "-v", "--version", "version":
			fmt.Println("magpie", version)
			return
		case "-h", "--help", "help":
			fmt.Println("magpie", version, "· Marionette engine seed (gateway only)")
			fmt.Println("usage: magpie [serve]              (env MAGPIE_ADDR=127.0.0.1:<port>)")
			fmt.Println("       magpie group add <name> models=<m> [id=] [pick=]")
			fmt.Println("       magpie group rm <id>")
			fmt.Println("       magpie provider add|set|key|rm|refresh|models …")
			fmt.Println("       magpie models")
			return
		case "serve":
			// the one long-running mode this seed exists for
		case "provider":
			// Custom sources and preset keys: Marionette drives
			// provider add/set/key/rm/refresh/models. Stock CLI lives
			// in internal/cli; this is the slim Save/Add/Fetch path.
			if err := seedPrepare(); err != nil {
				fmt.Fprintln(os.Stderr, "magpie:", err)
				os.Exit(1)
			}
			if err := seedProvider(args[1:]); err != nil {
				fmt.Fprintln(os.Stderr, "magpie:", err)
				os.Exit(1)
			}
			return
		case "models":
			if err := seedPrepare(); err != nil {
				fmt.Fprintln(os.Stderr, "magpie:", err)
				os.Exit(1)
			}
			seedListModels()
			return
		case "group":
			// Codex share aliases: Marionette maps a Codex builtin name
			// (gpt-5.6-luna) onto an engine model via provider.SaveGroup.
			// Stock `group` lives in internal/cli, which the seed excludes;
			// this is the slim upsert/rm the alias path actually needs.
			if err := seedPrepare(); err != nil {
				fmt.Fprintln(os.Stderr, "magpie:", err)
				os.Exit(1)
			}
			if err := seedGroup(args[1:]); err != nil {
				fmt.Fprintln(os.Stderr, "magpie:", err)
				os.Exit(1)
			}
			return
		default:
			// Unknown verbs (TUI, quota CLI, agent installers) stay out of
			// the seed. Falling through to serve would block the caller
			// forever and squat the gateway port.
			fmt.Fprintln(os.Stderr, "magpie:", args[0], "is not in the Marionette engine seed")
			os.Exit(2)
		}
	}
	if err := seedPrepare(); err != nil {
		fmt.Fprintln(os.Stderr, "magpie:", err)
		os.Exit(1)
	}
	netproxy.Install()
	// Built-in plugins default to on (plugins.yml), same as stock startup.
	if _, err := pluginrt.Settle(); err != nil {
		fmt.Fprintln(os.Stderr, "magpie: plugins.yml:", err)
	}
	if _, err := pluginrt.Refresh(); err != nil {
		fmt.Fprintln(os.Stderr, "magpie: plugins.yml:", err)
	}
	server := gateway.New()
	go stats.Run(version, "serve")
	go catalog.KeepFresh()
	// A signed-in account's list only exists at its vendor; stock serve()
	// keeps asking while one is outstanding. Same loop here.
	go func() {
		for provider.FetchNew(20 * time.Second) {
			time.Sleep(time.Minute)
		}
	}()
	if n := len(provider.Catalog()); n == 0 {
		fmt.Println("! no models yet — add providers in Marionette's model share")
	} else {
		fmt.Printf("magpie gateway on %s · %d models\n", gateway.URL(), n)
	}
	if err := server.ListenAndServe(context.Background()); err != nil {
		fmt.Fprintln(os.Stderr, "serve:", err)
		os.Exit(1)
	}
}

func seedPrepare() error {
	if _, err := appdir.CheckEnv(); err != nil {
		return err
	}
	// Stock startup's file migrations — keep them or a future upstream
	// settings format reads stale after the seed's first run.
	settings.Migrate()
	return nil
}

// seedGroup is the slim `magpie group add|rm` the Codex share alias needs.
// SaveGroup upserts by id (stock `group add` errors if the id exists, which
// would break effort re-pins and rebinds).
func seedGroup(args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("group add <name> models=<m> [id=] [pick=]  |  group rm <id>")
	}
	switch args[0] {
	case "add", "new":
		if len(args) < 2 {
			return fmt.Errorf("group add <name> models=<m> [id=] [pick=]")
		}
		return seedGroupAdd(args[1:])
	case "rm", "remove", "delete":
		if len(args) < 2 {
			return fmt.Errorf("group rm <id>")
		}
		return provider.DeleteGroup(args[1])
	default:
		return fmt.Errorf("group %s is not in the Marionette engine seed", args[0])
	}
}

func seedGroupAdd(args []string) error {
	g := provider.Group{
		Name:    strings.TrimSpace(args[0]),
		Routing: provider.Manual,
	}
	for _, kv := range args[1:] {
		key, val, ok := strings.Cut(kv, "=")
		if !ok {
			return fmt.Errorf("group add: want k=v, got %q", kv)
		}
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "models", "model":
			g.Members = splitSeedMembers(val)
		case "id":
			g.ID = strings.ToLower(strings.TrimSpace(val))
		case "pick", "use":
			g.Pick = strings.TrimSpace(val)
			g.Routing = provider.Manual
		default:
			return fmt.Errorf("group add: unknown field %q", key)
		}
	}
	if g.Pick != "" && !slices.Contains(g.Members, g.Pick) {
		g.Members = append([]string{g.Pick}, g.Members...)
	}
	if len(g.Members) == 0 {
		return fmt.Errorf("group add: models=<provider/model> is required")
	}
	if strings.TrimSpace(g.Name) == "" {
		g.Name = g.ID
	}
	return provider.SaveGroup(g)
}

func splitSeedMembers(v string) []string {
	var out []string
	for _, p := range strings.Split(v, ",") {
		p = strings.TrimSpace(p)
		if p != "" {
			out = append(out, p)
		}
	}
	return out
}

// seedProvider is the slim `magpie provider add|set|key|rm|refresh|models`
// Marionette uses to register custom sources and preset keys. Writes go
// through provider.Save; the running gateway sees providers.json on the
// next request (heldFreshLocked).
func seedProvider(args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("provider add|set|key|rm|refresh|models …")
	}
	verb, rest := args[0], args[1:]
	switch verb {
	case "add":
		return seedProviderAdd(rest)
	case "set":
		if len(rest) < 2 {
			return fmt.Errorf("provider set <id> k=v…")
		}
		p, err := provider.Find(rest[0])
		if err != nil {
			return err
		}
		if err := seedApplyPairs(p, rest[1:]); err != nil {
			return err
		}
		return provider.Save(*p)
	case "key":
		if len(rest) != 2 {
			return fmt.Errorf("provider key <id> <key>")
		}
		p, err := provider.Find(rest[0])
		if err != nil {
			return err
		}
		p.Key = rest[1]
		return provider.Save(*p)
	case "rm", "remove", "delete":
		if len(rest) != 1 {
			return fmt.Errorf("provider rm <id>")
		}
		return provider.Delete(rest[0])
	case "refresh", "fetch":
		if len(rest) != 1 {
			return fmt.Errorf("provider %s <id>", verb)
		}
		return seedProviderFetch(rest[0])
	case "models":
		if len(rest) < 1 {
			return fmt.Errorf("provider models <id> [all|ids…]")
		}
		if len(rest) == 1 {
			return seedProviderFetch(rest[0])
		}
		if len(rest) == 2 && rest[1] == "all" {
			return provider.SetModels(rest[0], nil)
		}
		return provider.SetModels(rest[0], rest[1:])
	default:
		return fmt.Errorf("provider %s is not in the Marionette engine seed", verb)
	}
}

func seedProviderAdd(rest []string) error {
	if len(rest) == 0 {
		return fmt.Errorf("provider add <preset> <key>   or   provider add <name> k=v…")
	}
	var p provider.Provider
	if pr, err := provider.FromPreset(strings.ToLower(rest[0])); err == nil {
		p = pr
		if len(rest) > 1 && !strings.Contains(rest[1], "=") {
			p.Key = rest[1]
			rest = rest[2:]
		} else {
			rest = rest[1:]
		}
	} else {
		p = provider.Provider{Name: rest[0]}
		rest = rest[1:]
	}
	if err := seedApplyPairs(&p, rest); err != nil {
		return err
	}
	id, err := provider.Add(p)
	if err != nil {
		return err
	}
	saved, err := provider.Find(id)
	if err != nil {
		return err
	}
	fmt.Println("added", saved.Name, "("+saved.ID+")")
	_ = seedProviderFetch(saved.ID)
	return nil
}

func seedProviderFetch(id string) error {
	p, err := provider.Find(id)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	ms, err := p.Fetch(ctx)
	if err != nil {
		return err
	}
	fmt.Println(len(ms), "models from", id)
	return nil
}

func seedListModels() {
	for _, e := range provider.Catalog() {
		if e.ID != "" {
			fmt.Println(e.ID)
		}
	}
}

func seedApplyPairs(p *provider.Provider, pairs []string) error {
	for _, kv := range pairs {
		k, v, ok := strings.Cut(kv, "=")
		if !ok {
			return fmt.Errorf("expected key=value, got %q", kv)
		}
		switch strings.ToLower(k) {
		case "id":
			p.ID = v
		case "name":
			p.Name = v
		case "url", "chat", "openai":
			p.Chat = v
		case "responses":
			p.Responses = v
		case "anthropic":
			p.Anthropic = v
		case "gemini":
			p.Gemini = v
		case "key":
			p.Key = v
		case "models":
			p.Models = splitSeedMembers(v)
		case "catalog":
			p.Catalog = v
		case "region", "plan":
			if err := seedApplyRegion(p, v); err != nil {
				return err
			}
		default:
			if len(k) > len("header.") && strings.EqualFold(k[:len("header.")], "header.") {
				name := k[len("header."):]
				if p.Headers == nil {
					p.Headers = map[string]string{}
				}
				for h := range p.Headers {
					if strings.EqualFold(h, name) {
						delete(p.Headers, h)
					}
				}
				if strings.TrimSpace(v) != "" {
					p.Headers[name] = v
				}
				continue
			}
			return fmt.Errorf("unknown field %q", k)
		}
	}
	return nil
}

func seedApplyRegion(p *provider.Provider, v string) error {
	pr := provider.Preset(p.Preset)
	if pr == nil || len(pr.Regions) == 0 {
		return fmt.Errorf("%s has no regions or plans to pick", p.Name)
	}
	var ids []string
	for _, r := range pr.Regions {
		ids = append(ids, r.ID)
	}
	i := slices.IndexFunc(pr.Regions, func(r provider.Region) bool {
		return strings.EqualFold(r.ID, v)
	})
	if i < 0 {
		return fmt.Errorf("region=%s: %s's are %s", v, pr.Name, strings.Join(ids, ", "))
	}
	r := pr.Regions[i]
	p.Chat, p.Responses, p.Anthropic, p.Decide = r.Chat, r.Responses, r.Anthropic, r.Decide
	if r.KeysURL != "" {
		p.KeysURL = r.KeysURL
	}
	if r.Catalog != "" {
		p.Catalog = r.Catalog
	}
	return nil
}
