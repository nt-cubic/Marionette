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
// routes, /v1/magpie/quotas. Anything beyond serving (agent management,
// `magpie quota` CLI, login flows) is intentionally absent — "更新引擎"
// installs the full upstream magpie and the seed steps aside.
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
	for _, arg := range os.Args[1:] {
		switch arg {
		case "-v", "--version", "version":
			fmt.Println("magpie", version)
			return
		case "-h", "--help", "help":
			fmt.Println("magpie", version, "· Marionette engine seed (gateway only)")
			fmt.Println("usage: magpie [serve]   (env MAGPIE_ADDR=127.0.0.1:<port>)")
			return
		case "serve":
			// the one mode this seed exists for
		default:
			// Marionette sometimes drives the engine binary as a CLI
			// (`provider set/add/rm`…). Those live in internal/cli, which
			// this seed deliberately excludes — answer with a loud error
			// and a non-zero exit so the caller fails fast. Falling through
			// to serve here would instead block the caller forever (it
			// waits for process exit) and squat the gateway port.
			fmt.Fprintln(os.Stderr, "magpie:", arg, "is not in the Marionette engine seed — update the engine (Marionette model share) for the full CLI")
			os.Exit(2)
		}
	}
	if _, err := appdir.CheckEnv(); err != nil {
		fmt.Fprintln(os.Stderr, "magpie:", err)
		os.Exit(1)
	}
	netproxy.Install()
	// Stock startup's file migrations — keep them or a future upstream
	// settings format reads stale after the seed's first run.
	settings.Migrate()
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
