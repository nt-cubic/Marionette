package gateway

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net/http"
	"slices"
	"strings"

	"github.com/yetone/magpie/internal/access"
)

// magpie's web search, the one a model that can't search is given
// (webSearch: the searcher's model, a Kimi Code plan's service, the
// search APIs), is also an MCP server of its own (Kayphoon on Discord),
// so Cursor, Claude Desktop and any other agent can mount it as a
// web_search tool: Streamable HTTP at SearchMCPPath, JSON answered to each
// POST. It is a gateway route like the others: this computer calls it with
// any key, another one only while magpie is shared and with an enabled
// gateway key (lanGuard), a gateway key's limit holds each search
// (reserveFor), and a web page is turned away, as the MCP spec has a
// server check Origin.

// SearchMCPPath is where the web search MCP server is served. It has two
// segments under /mcp/, so it can't hide a signed-in server's /mcp/{name}.
const SearchMCPPath = "/mcp/magpie/web-search"

// OwnMCPServers are the MCP servers the gateway serves itself, by the
// names `magpie tools` lists their tools under, in the order a tool name
// two of them have is looked for.
var OwnMCPServers = []struct{ Name, Path string }{
	{"image", ImageMCPPath},
	{"vision", VisionMCPPath},
	{"web-search", SearchMCPPath},
	{"tools", ToolsMCPPath},
}

// searchMCPVersions are the MCP versions the server speaks, newest first:
// a client asking for one of them is answered in it, any other in the first.
var searchMCPVersions = []string{"2025-06-18", "2025-03-26", "2024-11-05"}

const searchMCPTool = "web_search"

type mcpMessage struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
}

func (s *Server) searchMCP(w http.ResponseWriter, r *http.Request) {
	serveMCP(w, r, "magpie's web search", 1<<20, s.searchMCPCall)
}

// serveMCP answers the JSON-RPC messages POSTed to one of magpie's own MCP
// servers (what names it in a refusal) with call, a body up to limit bytes
// long: guarded as a gateway route, and a web page turned away.
func serveMCP(w http.ResponseWriter, r *http.Request, what string, limit int64, call func(*http.Request, mcpMessage) (any, map[string]any)) {
	if !local(r) && !sharedWith(r) {
		http.Error(w, what+" is served to another machine only when magpie is shared on the local network and the request carries a gateway key (Authorization: Bearer <key>)", http.StatusForbidden)
		return
	}
	// a page on this computer (localhost) gets past corsGuard; only one the
	// user listed in Settings, which corsGuard held to a gateway key, may
	// call this
	if o := r.Header.Get("Origin"); o != "" && !corsAllowed(o) {
		http.Error(w, what+" MCP server answers agents, not web pages", http.StatusForbidden)
		return
	}
	if r.Method != http.MethodPost {
		// no stream of the server's own (GET) and no session to end (DELETE)
		w.Header().Set("Allow", "POST")
		http.Error(w, "POST JSON-RPC messages here (MCP Streamable HTTP)", http.StatusMethodNotAllowed)
		return
	}
	body, ok := readMCPBody(w, r, limit)
	if !ok {
		return
	}
	body = bytes.TrimSpace(body)
	batch := len(body) > 0 && body[0] == '['
	var msgs []mcpMessage
	var err error
	if batch {
		err = json.Unmarshal(body, &msgs)
	} else {
		var m mcpMessage
		err = json.Unmarshal(body, &m)
		msgs = []mcpMessage{m}
	}
	if err != nil || len(msgs) == 0 {
		writeJSON(w, http.StatusBadRequest, map[string]any{"jsonrpc": "2.0", "id": nil, "error": map[string]any{"code": -32700, "message": "parse error: the body isn't a JSON-RPC message"}})
		return
	}
	var out []any
	for _, m := range msgs {
		// a notification (no id) or a response to the server is answered
		// with nothing
		if len(m.ID) == 0 || bytes.Equal(m.ID, []byte("null")) || m.Method == "" {
			continue
		}
		result, rpcErr := call(r, m)
		reply := map[string]any{"jsonrpc": "2.0", "id": m.ID}
		if rpcErr != nil {
			reply["error"] = rpcErr
		} else {
			reply["result"] = result
		}
		out = append(out, reply)
	}
	if len(out) == 0 {
		w.WriteHeader(http.StatusAccepted)
		return
	}
	if batch {
		writeJSON(w, http.StatusOK, out)
		return
	}
	writeJSON(w, http.StatusOK, out[0])
}

// searchMCPCall answers one request: its result, or its JSON-RPC error.
func (s *Server) searchMCPCall(r *http.Request, m mcpMessage) (any, map[string]any) {
	switch m.Method {
	case "initialize":
		return map[string]any{
			"protocolVersion": mcpVersion(m),
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": "magpie-web-search", "title": "Magpie Web Search", "version": Version},
			"instructions":    "web_search searches the web with the search magpie gives its models: a provider that can search, or the search APIs set up in magpie (Settings → Models → Web search). web_fetch reads a page it found, by its URL.",
		}, nil
	case "ping":
		return map[string]any{}, nil
	case "tools/list":
		t := searchTool(nil)
		return map[string]any{"tools": []any{map[string]any{
			"name":        searchMCPTool,
			"title":       "Web search",
			"description": t.Description,
			"inputSchema": t.Schema,
			"annotations": map[string]any{"readOnlyHint": true, "openWorldHint": true},
		}, fetchTool()}}, nil
	case "tools/call":
		var p struct {
			Name      string `json:"name"`
			Arguments struct {
				Query string `json:"query"`
				fetchArgs
			} `json:"arguments"`
		}
		if err := json.Unmarshal(m.Params, &p); err != nil {
			return nil, map[string]any{"code": -32602, "message": "invalid params: " + err.Error()}
		}
		var said string
		var err error
		switch p.Name {
		case searchMCPTool:
			said, err = s.searchMCPRun(r, strings.TrimSpace(p.Arguments.Query))
		case fetchMCPTool:
			said, err = s.fetchMCPRun(r, p.Arguments.fetchArgs)
		default:
			return nil, map[string]any{"code": -32602, "message": "unknown tool " + p.Name + ": this server has " + searchMCPTool + " and " + fetchMCPTool}
		}
		if err != nil {
			return toolResult(err.Error(), true), nil
		}
		return toolResult(said, false), nil
	}
	return nil, map[string]any{"code": -32601, "message": "method not found: " + m.Method}
}

// mcpVersion is the MCP version an initialize is answered in: the one it
// asks for when the server speaks it, else the newest.
func mcpVersion(m mcpMessage) string {
	var p struct {
		ProtocolVersion string `json:"protocolVersion"`
	}
	json.Unmarshal(m.Params, &p)
	if slices.Contains(searchMCPVersions, p.ProtocolVersion) {
		return p.ProtocolVersion
	}
	return searchMCPVersions[0]
}

func toolResult(text string, isError bool) map[string]any {
	return map[string]any{"content": []any{map[string]any{"type": "text", "text": text}}, "isError": isError}
}

type searchMCPError string

func (e searchMCPError) Error() string { return string(e) }

// searchMCPRun searches for the query as a model's web_search would be:
// held to the calling gateway key's limit, and named in the Routing view
// as the agent's MCP call.
func (s *Server) searchMCPRun(r *http.Request, query string) (string, error) {
	if query == "" {
		return "", searchMCPError("web_search needs a query")
	}
	release, err := reserveFor(access.Caller(r.Context()), int64(len(query)), "", limitClock())
	if err != nil {
		return "", err
	}
	defer release()
	ctx := context.WithValue(r.Context(), searchForKey{}, &CallFor{Agent: callerOf(r).agent, MCP: true})
	said, _, err := s.webSearch(ctx, query)
	if err != nil {
		if r.Context().Err() == nil {
			log.Printf("web search MCP: %v", err)
		}
		msg := "magpie couldn't search the web: " + err.Error()
		if errors.Is(err, errNoSearcher) {
			msg += ": add a search API in magpie's Settings → Models → Web search (or magpie search add <api> <key>), or a provider that can search"
		}
		return "", searchMCPError(msg)
	}
	// what was masked on the way out (maskedQuery, or the searcher's
	// request) is the agent's own: it gets it back
	return restored(said, false), nil
}

// readMCPBody reads a request body of at most limit bytes, answering 413
// for a longer one. A LimitReader alone ends at the limit as if the body
// had, so a valid message padded past it was read as its prefix and run
// (#1766); one byte more tells a body that ended from one that was cut.
func readMCPBody(w http.ResponseWriter, r *http.Request, limit int64) ([]byte, bool) {
	if r.Body == nil {
		return nil, true
	}
	tooLarge := func() ([]byte, bool) {
		w.Header().Set("Connection", "close")
		http.Error(w, "the request body is longer than this server takes", http.StatusRequestEntityTooLarge)
		return nil, false
	}
	if r.ContentLength > limit {
		return tooLarge()
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, limit+1))
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return nil, false
	}
	if int64(len(body)) > limit {
		return tooLarge()
	}
	return body, true
}
