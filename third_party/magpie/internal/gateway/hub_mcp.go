package gateway

import (
	"context"
	"encoding/json"
	"net/http"
	"strconv"
	"time"

	"github.com/yetone/magpie/internal/access"
)

// The MCP hub (#2679): with the Library's switch on, magpie is the one MCP
// server an agent is given, at HubMCPPath<agent>, and it hands each call
// on to the Library server behind it, as Composio does. Its tools are
// those of the Library servers ticked for that agent, each named
// <server>__<tool>; the servers themselves (a command started once for
// every agent, a URL reached with magpie's sign-in) are the library's to
// run (library/mcp_hub.go), which sets MCPHub. It is guarded as
// magpie's other MCP servers are (serveMCP), and each call is held to the
// calling gateway key's limit (reserveFor). Tools only, for now: no
// resources or prompts.

// HubMCPPath is where the hub is served, the agent's id after it.
const HubMCPPath = "/mcp/magpie/hub/"

// HubMCPAll is the hub for any other app (Cursor, VS Code, a script): one
// address with every server in the Library, ticked for an agent or not.
// It is the agent "" to Hub.
const HubMCPAll = "/mcp/magpie/hub"

// HubTool is one tool a Library server lists, as the hub gives it.
type HubTool struct {
	Server string // the Library server's name
	Tool   string // the tool's name on that server
	// Spec is the tool as the server listed it: its description,
	// inputSchema and the rest go to the agent as they are, its name
	// replaced
	Spec map[string]json.RawMessage
}

// Hub is the servers behind the hub: the library's.
type Hub interface {
	// On is whether the Library's switch is on: off, the hub lists no
	// tool and runs none
	On() bool
	// Tools are the tools of the servers ticked for agent (every server
	// in the Library for agent "", HubMCPAll's), but those turned off; a
	// server that couldn't be reached by ctx's end is left out
	Tools(ctx context.Context, agent string) []HubTool
	// Call runs one tool of server for agent, and is its result as the
	// server gave it; an error is why it couldn't be run
	Call(ctx context.Context, agent, server, tool string, args json.RawMessage) (json.RawMessage, error)
	// Stop ends every server the hub started
	Stop()
	// Prune ends each server the hub started that the Library no longer
	// gives: the switch turned off, the server removed or edited, here or
	// by another magpie (the window, the CLI)
	Prune()
}

// MCPHub is set by the library; nil, the hub has no server.
var MCPHub Hub

// The hub's budgets: tools/list waits for the servers this long (a server
// still starting is left out, and listed once it has), and a call this
// long, under the 60s an MCP client waits for an answer, for the agent to
// be told which server didn't answer rather than its own timeout.
var (
	hubListWait = 40 * time.Second
	hubCallWait = 50 * time.Second
)

const hubBody = 4 << 20

const errHubOff = "magpie's MCP hub is off, so it runs no server's tools: turn it on in magpie's Library → MCP"

func (s *Server) hubMCP(w http.ResponseWriter, r *http.Request) {
	agent := r.PathValue("agent")
	serveMCP(w, r, "magpie's MCP hub", "magpie", hubBody, func(r *http.Request, m mcpMessage) (any, map[string]any) {
		return s.hubMCPCall(r, agent, m)
	})
}

// hubTool is one tool as the hub names it.
type hubTool struct {
	HubTool
	name string
}

// hubSep is between a server's name and its tool's in the hub's names.
const hubSep = "__"

// hubNamed names ts <server>__<tool>, in the characters MCP clients take
// and 64 of them at most, a name two would share told apart by a number.
func hubNamed(ts []HubTool) []hubTool {
	out := make([]hubTool, 0, len(ts))
	seen := map[string]bool{}
	for _, t := range ts {
		base := unsafeToolChars.ReplaceAllString(t.Server, "-") + hubSep + unsafeToolChars.ReplaceAllString(t.Tool, "-")
		name := cutName(base, 64)
		for i := 2; seen[name]; i++ {
			suf := "_" + strconv.Itoa(i)
			name = cutName(base, 64-len(suf)) + suf
		}
		seen[name] = true
		out = append(out, hubTool{HubTool: t, name: name})
	}
	return out
}

func hubTools(ctx context.Context, agent string) []hubTool {
	if MCPHub == nil || !MCPHub.On() {
		return nil
	}
	ctx, cancel := context.WithTimeout(ctx, hubListWait)
	defer cancel()
	return hubNamed(MCPHub.Tools(ctx, agent))
}

func (s *Server) hubMCPCall(r *http.Request, agent string, m mcpMessage) (any, map[string]any) {
	switch m.Method {
	case "initialize":
		say := "The tools of the MCP servers in magpie's Library that are switched on for this agent, each named <server>__<tool>. Add servers, or switch them on for an agent, in magpie's Library → MCP."
		if agent == "" {
			say = "The tools of every MCP server in magpie's Library, each named <server>__<tool>. Add servers in magpie's Library → MCP."
		}
		if MCPHub == nil || !MCPHub.On() {
			say = errHubOff
		}
		return map[string]any{
			"protocolVersion": mcpVersion(m),
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": "magpie", "title": "Magpie MCP hub", "version": Version},
			"instructions":    say,
		}, nil
	case "ping":
		return map[string]any{}, nil
	case "tools/list":
		ts := hubTools(r.Context(), agent)
		list := make([]any, 0, len(ts))
		for _, t := range ts {
			spec := make(map[string]json.RawMessage, len(t.Spec)+1)
			for k, v := range t.Spec {
				spec[k] = v
			}
			name, _ := json.Marshal(t.name)
			spec["name"] = name
			if s, ok := spec["inputSchema"]; !ok || !json.Valid(s) {
				spec["inputSchema"] = json.RawMessage(`{"type":"object","properties":{}}`)
			}
			list = append(list, spec)
		}
		return map[string]any{"tools": list}, nil
	case "tools/call":
		var p struct {
			Name      string          `json:"name"`
			Arguments json.RawMessage `json:"arguments"`
		}
		if err := json.Unmarshal(m.Params, &p); err != nil {
			return nil, map[string]any{"code": -32602, "message": "invalid params: " + err.Error()}
		}
		if MCPHub == nil || !MCPHub.On() {
			return toolResult(errHubOff, true), nil
		}
		for _, t := range hubTools(r.Context(), agent) {
			if t.name == p.Name {
				return s.hubMCPRun(r, agent, t, p.Arguments), nil
			}
		}
		if agent == "" {
			return nil, map[string]any{"code": -32602, "message": "unknown tool " + p.Name + ": no MCP server in magpie's Library gives it"}
		}
		return nil, map[string]any{"code": -32602, "message": "unknown tool " + p.Name + ": no MCP server switched on for " + agent + " in magpie's Library gives it"}
	}
	return nil, map[string]any{"code": -32601, "message": "method not found: " + m.Method}
}

// hubMCPRun runs one tool on its server, held to the calling gateway key's
// limit. A server that is down or doesn't answer in time is an error
// result naming it, never a wait past the client's.
func (s *Server) hubMCPRun(r *http.Request, agent string, t hubTool, args json.RawMessage) any {
	// the request log names the Library's server, and the agent the hub is mounted for
	use := mcpUseOf(r.Context())
	use.on(t.Server)
	use.as(agent)
	release, err := reserveFor(access.Caller(r.Context()), int64(len(args)), "", limitClock())
	if err != nil {
		return toolResult(err.Error(), true)
	}
	defer release()
	if len(args) == 0 || string(args) == "null" {
		args = json.RawMessage(`{}`)
	}
	ctx, cancel := context.WithTimeout(r.Context(), hubCallWait)
	defer cancel()
	res, err := MCPHub.Call(ctx, agent, t.Server, t.Tool, args)
	if err != nil {
		why := err.Error()
		if ctx.Err() != nil && r.Context().Err() == nil {
			why = "it didn't answer in " + hubCallWait.String()
		}
		return toolResult("The MCP server "+t.Server+" couldn't run "+t.Tool+": "+why, true)
	}
	return res
}

// hubPruneEvery is how often the gateway ends the servers the hub no
// longer gives. An agent given the servers' own entries again never asks
// the hub, so it is looked for, not waited for.
var hubPruneEvery = 5 * time.Second

// keepHubPruned prunes the hub's servers while ctx lasts.
func keepHubPruned(ctx context.Context) {
	tick := time.NewTicker(hubPruneEvery)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
			if MCPHub != nil {
				MCPHub.Prune()
			}
		}
	}
}

// StopHub ends the servers the hub started: magpie is stopping.
func StopHub() {
	if MCPHub != nil {
		MCPHub.Stop()
	}
}
