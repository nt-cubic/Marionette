package provider

import "slices"

// NativeEndpoints are the paths a request for the model is relayed on to
// its provider as it is (/v1/models' and an agent plugin's connect's
// "native_endpoints"): the APIs the provider serves it on. None for a
// routing group, whose members may speak any, or a model every request
// to is translated anyway (a subscription served through its agent's own
// API).
func NativeEndpoints(e Entry) []string {
	p := e.Provider
	if e.Group != "" || p.Native(e.Model) == "" {
		return nil
	}
	apis := p.APIs(e.Model)
	var out []string
	for _, pr := range p.Speaks() {
		if slices.Contains(Protocols, pr) && (apis == nil || slices.Contains(apis, pr)) {
			out = append(out, map[Protocol]string{Chat: "/v1/chat/completions", Responses: "/v1/responses", Anthropic: "/v1/messages"}[pr])
		}
	}
	return out
}
