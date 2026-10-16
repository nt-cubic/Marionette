package gateway

import (
	"net/http"
	"slices"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/middleware"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
)

// serveAgent is serve for a request an agent sent: the agent middleware
// of the gateway's middleware point (gwmw.Agents: the JS gateway middleware
// plugins, internal/middleware, while plugins/middleware is on) see its
// body first, and its reply on the way back, in the agent's own API.
// magpie's own requests through serve (titles, the router's classifier,
// search and vision stand-ins) go without them, as their replies are
// magpie's to read.
func (s *Server) serveAgent(w http.ResponseWriter, r *http.Request, from provider.Protocol, body []byte) {
	withGatewaySession(w, r)
	w, recorded := recordConversation(w, r, from, body)
	defer recorded()
	w, body, hooked := gatewayHooks(w, r, from, body)
	defer hooked()
	agents := gwmw.AgentsWanted(middlewareWanted)
	if len(agents) == 0 {
		s.serve(w, r, from, body)
		return
	}
	info := gwmw.Info{Protocol: string(from), Model: modelOf(body), Stream: streamOf(body), Path: r.URL.Path, Agent: agentOf(r)}
	for _, a := range agents {
		run := a.Begin(info)
		if run == nil {
			continue
		}
		defer run.End()
		var turned *gwmw.Reject
		body, turned = run.Request(body)
		if turned != nil {
			writeError(w, from, turned.Status, turned.Message)
			return
		}
		var done func()
		w, done = run.Wrap(w)
		defer done()
	}
	s.serve(w, r, from, body)
}

// middlewareWanted: plugins.json has a JS middleware on, so plugins/middleware
// is in use.
func middlewareWanted() bool { return middleware.On() > 0 }

// gatewayHooks runs the asynchronous request hooks (OpenCode plugins'
// gateway entries, in the plugin host) on an agent's request: the body they
// leave is the one served, and they see the reply the agent got once it is
// whole. Its writer wraps the middleware's, so what they see is what the
// agent was given. With no hook it is the request as it came.
func gatewayHooks(w http.ResponseWriter, r *http.Request, from provider.Protocol, body []byte) (http.ResponseWriter, []byte, func()) {
	if !plugin.HasGateway() {
		return w, body, func() {}
	}
	info := plugin.GatewayContext{Origin: gatewayOrigin(r), Protocol: string(from), Model: modelOf(body), Stream: streamOf(body), Path: r.URL.Path, Agent: agentOf(r), Session: sessionOf(r.Header)}
	if req, err := parse(from, body); err == nil {
		info.Cwd = workDir(req)
		info.Query = gatewayQuery(req)
	}
	run := plugin.GatewayBegin(r.Context(), body, info)
	if run == nil {
		return w, body, func() {}
	}
	w, done := gatewayResponse(w, r, from, run.Body, run)
	return w, run.Body, func() { done(); run.Close() }
}

// gatewayQuery uses the same user-authored words as routing, excluding
// agent context, tool results and images. Tool-only increments have none.
func gatewayQuery(req *Request) string {
	for i := len(req.Messages) - 1; i >= 0; i-- {
		m := req.Messages[i]
		if m.Role != "user" {
			continue
		}
		if query := userSaid(m); query != "" {
			return query
		}
	}
	return ""
}

// gatewayOrigin exposes provenance, never credentials or peer addresses.
// Forwarded traffic remains remote even when the administrator trusts its
// proxy for authentication: that trust does not establish conversation ownership.
func gatewayOrigin(r *http.Request) string {
	if access.Caller(r.Context()).KeyID != "" || managedKey(r) {
		return "gateway-key"
	}
	if local(r) && !slices.ContainsFunc(forwardHeaders, func(h string) bool { return r.Header.Get(h) != "" }) {
		return "local"
	}
	return "remote"
}
