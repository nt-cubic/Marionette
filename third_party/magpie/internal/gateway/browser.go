package gateway

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/yetone/magpie/internal/browserext"
)

// Magpie Browser: the agents magpie serves use the user's own browser,
// its tabs signed in to whatever the user is, through the Magpie Browser
// extension (internal/browserext). Not a browser of magpie's own: a fresh
// profile is signed in to nothing, and an agent's own browser tool stops
// working when its account changes.
//
// The extension holds a WebSocket to BrowserExtensionPath, taken only from
// its own origin on this computer. An agent calls the tools at
// BrowserMCPPath (Streamable HTTP, or `magpie mcp browser` over stdio),
// and each call is handed to the extension, which carries it out with the
// DevTools protocol in the tab and answers with MCP content. Only this
// computer's agents may: unlike the other servers of magpie's own, it is
// never served to another machine, shared or not, since it acts as the
// user wherever they are signed in.

// BrowserMCPPath is where the browser MCP server is served.
const BrowserMCPPath = "/mcp/magpie/browser"

// BrowserExtensionPath is where the extension connects.
const BrowserExtensionPath = "/v1/magpie/browser/extension"

// BrowserStatusPath tells whether the extension is connected.
const BrowserStatusPath = "/v1/magpie/browser"

// BrowserSessionHeader names the agent session a call is from, so each
// one goes on in the tab it was working in; `magpie mcp browser` sends one
// a process.
const BrowserSessionHeader = "X-Magpie-Browser-Session"

// browserCallWait is how long a call waits for the extension: a
// browser_wait waits up to 60s, a load 30s, and each acts then waits for the
// page to settle.
const browserCallWait = 100 * time.Second

// browserRelay is the extensions connected, the last one first served.
type browserRelay struct {
	mu      sync.Mutex
	conns   []*browserConn
	pending map[string]chan json.RawMessage
	next    uint64
}

type browserConn struct {
	c       *websocket.Conn
	version string
	agent   string
	since   time.Time
	wmu     sync.Mutex
}

func (c *browserConn) send(ctx context.Context, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	return c.c.Write(ctx, websocket.MessageText, b)
}

func (b *browserRelay) current() *browserConn {
	b.mu.Lock()
	defer b.mu.Unlock()
	if len(b.conns) == 0 {
		return nil
	}
	return b.conns[len(b.conns)-1]
}

func (b *browserRelay) add(c *browserConn) {
	b.mu.Lock()
	b.conns = append(b.conns, c)
	b.mu.Unlock()
}

func (b *browserRelay) remove(c *browserConn) {
	b.mu.Lock()
	for i, x := range b.conns {
		if x == c {
			b.conns = append(b.conns[:i], b.conns[i+1:]...)
			break
		}
	}
	b.mu.Unlock()
}

func (b *browserRelay) answered(id string, result json.RawMessage) {
	b.mu.Lock()
	ch := b.pending[id]
	delete(b.pending, id)
	b.mu.Unlock()
	if ch != nil {
		ch <- result
	}
}

// call hands a tool call to the extension and waits for its MCP result.
func (b *browserRelay) call(ctx context.Context, session, tool string, args json.RawMessage) (json.RawMessage, error) {
	c := b.current()
	if c == nil {
		return nil, errBrowserAway
	}
	b.mu.Lock()
	if b.pending == nil {
		b.pending = map[string]chan json.RawMessage{}
	}
	b.next++
	id := strconv.FormatUint(b.next, 10)
	ch := make(chan json.RawMessage, 1)
	b.pending[id] = ch
	b.mu.Unlock()
	defer func() {
		b.mu.Lock()
		delete(b.pending, id)
		b.mu.Unlock()
	}()
	if len(args) == 0 || string(args) == "null" {
		args = json.RawMessage("{}")
	}
	if err := c.send(ctx, map[string]any{"id": id, "session": session, "tool": tool, "args": args}); err != nil {
		return nil, errors.New("couldn't reach Magpie Browser: " + err.Error())
	}
	t := time.NewTimer(browserCallWait)
	defer t.Stop()
	select {
	case r := <-ch:
		return r, nil
	case <-t.C:
		return nil, errors.New("Magpie Browser didn't answer in " + browserCallWait.String() + ": the page may be stuck; take a snapshot or a screenshot to see where it is")
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

var errBrowserAway = errors.New("Magpie Browser isn't connected, so there is no browser to use. Ask the user to set it up: magpie's Library → Magpie Browser → Set up, which loads the extension in Chrome, Edge or another Chromium browser. Once installed it connects by itself whenever that browser and magpie are both running.")

// browserExtension takes the extension's WebSocket.
func (s *Server) browserExtension(w http.ResponseWriter, r *http.Request) {
	if !local(r) || !loopbackHost(r.Host) || r.Header.Get("Origin") != browserext.Origin {
		http.Error(w, "only the Magpie Browser extension, on this computer, connects here", http.StatusForbidden)
		return
	}
	ws, err := websocket.Accept(w, r, &websocket.AcceptOptions{OriginPatterns: []string{"chrome-extension://" + browserext.ID}})
	if err != nil {
		log.Printf("browser extension socket: %v", err)
		return
	}
	ws.SetReadLimit(64 << 20) // a full-page screenshot
	c := &browserConn{c: ws, since: time.Now()}
	ctx := r.Context()
	defer ws.CloseNow()
	added := false
	defer func() {
		if added {
			s.browser.remove(c)
			log.Printf("Magpie Browser disconnected")
		}
	}()
	for {
		_, data, err := ws.Read(ctx)
		if err != nil {
			return
		}
		var m struct {
			Type    string          `json:"type"`
			ID      string          `json:"id"`
			Result  json.RawMessage `json:"result"`
			Version string          `json:"version"`
			Agent   string          `json:"agent"`
		}
		if json.Unmarshal(data, &m) != nil {
			continue
		}
		switch {
		case m.Type == "hello":
			c.version, c.agent = m.Version, m.Agent
			if !added {
				s.browser.add(c)
				added = true
				log.Printf("Magpie Browser %s connected", m.Version)
			}
		case m.Type == "ping":
			c.send(ctx, map[string]string{"type": "pong"})
		case m.ID != "":
			s.browser.answered(m.ID, m.Result)
		}
	}
}

// browserStatus says whether the extension is connected, for the GUI.
func (s *Server) browserStatus(w http.ResponseWriter, r *http.Request) {
	if !local(r) || fromBrowser(r) {
		http.Error(w, "Magpie Browser's state is told to this computer's magpie only", http.StatusForbidden)
		return
	}
	out := map[string]any{"connected": false, "version": browserext.Version()}
	if c := s.browser.current(); c != nil {
		out["connected"] = true
		out["extension"] = c.version
		out["browser"] = c.agent
		out["since"] = c.since
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) browserMCP(w http.ResponseWriter, r *http.Request) {
	// never another machine's, a gateway key or not: the tools act as the
	// user wherever their browser is signed in; and no web page's, whatever
	// Settings lists
	if !local(r) || fromBrowser(r) {
		http.Error(w, "magpie's browser MCP server answers agents on this computer only", http.StatusForbidden)
		return
	}
	serveMCP(w, r, "magpie's browser", "magpie-browser", 1<<20, s.browserMCPCall)
}

func (s *Server) browserMCPCall(r *http.Request, m mcpMessage) (any, map[string]any) {
	switch m.Method {
	case "initialize":
		return map[string]any{
			"protocolVersion": mcpVersion(m),
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": "magpie-browser", "title": "Magpie Browser", "version": Version},
			"instructions":    browserInstructions,
		}, nil
	case "ping":
		return map[string]any{}, nil
	case "tools/list":
		return map[string]any{"tools": browserTools()}, nil
	case "tools/call":
		var p struct {
			Name      string          `json:"name"`
			Arguments json.RawMessage `json:"arguments"`
		}
		if err := json.Unmarshal(m.Params, &p); err != nil {
			return nil, map[string]any{"code": -32602, "message": "invalid params: " + err.Error()}
		}
		if !browserToolNamed(p.Name) {
			return nil, map[string]any{"code": -32602, "message": "unknown tool " + p.Name}
		}
		out, err := s.browser.call(r.Context(), browserSession(r), p.Name, p.Arguments)
		if err != nil {
			return toolResult(err.Error(), true), nil
		}
		var result map[string]any
		if json.Unmarshal(out, &result) != nil || result["content"] == nil {
			return toolResult("Magpie Browser answered with something that isn't a tool result: update it (magpie's Library → Magpie Browser)", true), nil
		}
		return result, nil
	}
	return nil, map[string]any{"code": -32601, "message": "method not found: " + m.Method}
}

// browserSession is which agent session a call is from: the header the
// stdio server sends, else the MCP session, else the agent.
func browserSession(r *http.Request) string {
	for _, h := range []string{BrowserSessionHeader, "Mcp-Session-Id"} {
		if v := boundedSession(r.Header.Get(h)); v != "" {
			return v
		}
	}
	if a := callerOf(r).agent; a != "" {
		return "agent:" + a
	}
	return "default"
}
