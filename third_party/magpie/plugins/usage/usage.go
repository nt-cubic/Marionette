// Package usage is the Usage page: the allowances, what was spent and on
// what, the sessions, each request, and what filled the context window.
// Off, the page leaves the window; the gateway still counts what it
// passes on, and the tray panel's Usage tab still shows the allowances.
package usage

import "github.com/yetone/magpie/internal/pluginrt"

func init() {
	pluginrt.Register(pluginrt.Manifest{
		ID: "usage", API: "^1", Requires: []string{"window"}, Essential: true,
		Name: "Usage", Sub: "Your allowances, what was spent and on what, each request, and what filled the context window",
		Parts:  []pluginrt.Part{{Slot: "nav.page", Key: "usage"}},
		Client: []string{"i18n.js", "client.js", "page.js"},
	})
}
