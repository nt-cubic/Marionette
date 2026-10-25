package gateway

import (
	"net/http"
	"path"
	"strings"
)

// Front is what the gateway's own listener serves: Handler behind
// lanGuard, built once. `magpie web`'s port hands the gateway's client
// routes (ClientPath) to it too, so a request there is judged exactly as
// one on the gateway's port: the allow list, a key from another machine
// or through a proxy, a rebound page's Host, and corsGuard's Origin rules.
func (s *Server) Front() http.Handler {
	s.frontOnce.Do(func() { s.front = lanGuard(s.Handler()) })
	return s.front
}

// clientPrefixes are the gateway's client routes (Handler) that a page
// served on a port of its own passes on to the gateway: everything an
// agent or an SDK calls with the gateway as its base URL.
var clientPrefixes = []string{
	"/v1", "/v1beta",
	"/models", "/chat/completions", "/responses", "/messages",
	"/images", "/embeddings", "/rerank", "/audio", "/videos",
	"/muse-code/models", CodexPath, AlphaSearchPath, "/mcp",
}

// ClientPath: p is one of the gateway's client routes, which `magpie web`
// answers on its own port too by handing it to the gateway, so one address
// (one reverse-proxy location) serves the page and the agents (zgcwkj on
// Discord). The page's own routes keep its key: /api/* is the page's, so
// the Ollama and Claude Desktop routes there, and the Claude helper's
// callbacks under /_magpie/, stay on the gateway's port alone. A path that
// isn't clean (a "..", a "//") is none, and stays behind the page's key.
func ClientPath(p string) bool {
	if p == "" || path.Clean(p) != strings.TrimSuffix(p, "/") && p != "/" {
		return false
	}
	for _, pre := range clientPrefixes {
		if p == pre || strings.HasPrefix(p, pre+"/") {
			return true
		}
	}
	return false
}
