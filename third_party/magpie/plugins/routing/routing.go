// Package routing is the Routing page: each request the gateway routes,
// played as it decides, the accounts it weighed and what each answered,
// and the routing groups' editor. The gateway routes the same with the
// page off; while the user has a routing group, the page that edits it
// stays on.
package routing

import (
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/provider"
)

func init() {
	pluginrt.Register(pluginrt.Manifest{
		ID: "routing", API: "^1", Requires: []string{"window", "gateway"}, Essential: true,
		Name: "Routing", Sub: "Each request as the gateway routes it, the accounts it weighed and what each answered, and your routing groups",
		One: "1 group in use", Many: "{n} groups in use",
		InUse:  provider.NamedGroups,
		Parts:  []pluginrt.Part{{Slot: "nav.page", Key: "routing"}},
		Client: []string{"i18n.js", "routing.css", "client.js"},
	})
}
