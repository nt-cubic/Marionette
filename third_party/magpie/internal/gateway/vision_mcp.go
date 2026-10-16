package gateway

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
)

// magpie's image recognition, the model that describes images to a model
// that can't see them (seer: Settings › Models › Image recognition, else
// AutoVision's), is also an MCP server of its own beside the web search's
// (JonahFu on X), so an agent on a model that can't see, or any agent at
// all, can mount it as an analyze_image tool: Streamable HTTP at
// VisionMCPPath, served and guarded as SearchMCPPath is (serveMCP), a
// gateway key's limit holding each look (reserveFor).

// VisionMCPPath is where the image recognition MCP server is served.
const VisionMCPPath = "/mcp/magpie/vision"

const visionMCPTool = "analyze_image"

// visionMCPBody is how long a message to the server may be: an image is
// sent in it as a data URL.
const visionMCPBody = 32 << 20

// visionFileMax is the largest image file the tool reads from a path.
const visionFileMax = 20 << 20

// visionAskSystem is what the model is told when the agent asks something
// of the image, in place of visionSystem's full description.
const visionAskSystem = `You look at an image for an AI agent that cannot see it, and answer what it asks about the image from what the image shows.
- Transcribe text exactly as written wherever the answer needs it: code, terminal output, error messages, UI labels, numbers.
- Say so when what is asked can't be read or isn't in the image; don't guess.
No preamble.`

var visionMCPSchema = map[string]any{
	"type": "object",
	"properties": map[string]any{
		"image": map[string]any{
			"type":        "string",
			"description": "The image: an absolute path to an image file on this computer (PNG, JPEG, GIF or WebP), an http(s) URL, or a data URL (data:image/png;base64,...).",
		},
		"prompt": map[string]any{
			"type":        "string",
			"description": "What to find out about the image. Without it the image is described in full: its text transcribed, its layout, charts and diagrams.",
		},
	},
	"required": []string{"image"},
}

func (s *Server) visionMCP(w http.ResponseWriter, r *http.Request) {
	serveMCP(w, r, "magpie's image recognition", visionMCPBody, s.visionMCPCall)
}

// visionMCPCall answers one request: its result, or its JSON-RPC error.
func (s *Server) visionMCPCall(r *http.Request, m mcpMessage) (any, map[string]any) {
	switch m.Method {
	case "initialize":
		return map[string]any{
			"protocolVersion": mcpVersion(m),
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": "magpie-vision", "title": "Magpie Image Recognition", "version": Version},
			"instructions":    "analyze_image looks at an image with the model magpie describes images with (Settings → Models → Image recognition) and answers in text: for a model or agent that can't see images, or to read a screenshot, chart or diagram.",
		}, nil
	case "ping":
		return map[string]any{}, nil
	case "tools/list":
		return map[string]any{"tools": []any{map[string]any{
			"name":        visionMCPTool,
			"title":       "Analyze image",
			"description": "Looks at an image and answers in text: what the prompt asks of it, or, with no prompt, a full description with all its text transcribed. Use it to read screenshots, error dialogs, charts, diagrams and photos when you can't see images yourself.",
			"inputSchema": visionMCPSchema,
			"annotations": map[string]any{"readOnlyHint": true, "openWorldHint": false},
		}}}, nil
	case "tools/call":
		var p struct {
			Name      string `json:"name"`
			Arguments struct {
				Image  string `json:"image"`
				Prompt string `json:"prompt"`
			} `json:"arguments"`
		}
		if err := json.Unmarshal(m.Params, &p); err != nil {
			return nil, map[string]any{"code": -32602, "message": "invalid params: " + err.Error()}
		}
		if p.Name != visionMCPTool {
			return nil, map[string]any{"code": -32602, "message": "unknown tool " + p.Name + ": this server has " + visionMCPTool}
		}
		said, err := s.visionMCPRun(r, strings.TrimSpace(p.Arguments.Image), strings.TrimSpace(p.Arguments.Prompt))
		if err != nil {
			return toolResult(err.Error(), true), nil
		}
		return toolResult(said, false), nil
	}
	return nil, map[string]any{"code": -32601, "message": "method not found: " + m.Method}
}

// visionMCPRun has the image looked at as an image of a model's request
// would be, held to the calling gateway key's limit, and named in the
// Routing view as the agent's MCP call.
func (s *Server) visionMCPRun(r *http.Request, image, prompt string) (string, error) {
	if image == "" {
		return "", searchMCPError("analyze_image needs an image: a file path, an http(s) URL or a data URL")
	}
	src, err := visionSrc(r, image)
	if err != nil {
		return "", err
	}
	model, ok := seer()
	if !ok {
		if provider.HeldSettings().Vision == "off" {
			return "", searchMCPError("Image recognition is off in magpie: pick a model for it in Settings → Models → Image recognition")
		}
		return "", searchMCPError("magpie has no model that sees images: add a provider with one, or pick one in Settings → Models → Image recognition")
	}
	release, err := reserveFor(access.Caller(r.Context()), int64(len(src)+len(prompt)), "", limitClock())
	if err != nil {
		return "", err
	}
	defer release()
	ctx := context.WithValue(r.Context(), describeForKey{}, describeFor{call: &CallFor{Agent: callerOf(r).agent, Missing: VisionMissing(), MCP: true}})
	var said string
	if prompt == "" {
		said, err = s.describe(ctx, model, src)
	} else {
		said, err = s.askVisionWith(ctx, model, src, visionAskSystem, prompt)
	}
	if err != nil {
		if r.Context().Err() == nil {
			log.Printf("vision MCP: %v", err)
		}
		return "", searchMCPError(fmt.Sprintf("magpie couldn't look at the image with %s: %v%s", model, err, missingSeerNote()))
	}
	return said, nil
}

// visionSrc is the image as the describer is sent it: a URL or data URL as
// it is, a file read into a data URL. A file is read only for an agent on
// this computer: another machine's path names none of its own here, and a
// shared magpie's files aren't its callers'.
func visionSrc(r *http.Request, image string) (string, error) {
	low := strings.ToLower(image)
	switch {
	case strings.HasPrefix(low, "data:"):
		if !strings.HasPrefix(low, "data:image/") || !strings.Contains(low[:min(len(low), 64)], ";base64,") {
			return "", searchMCPError("analyze_image takes a data URL of an image in base64 (data:image/png;base64,...)")
		}
		return image, nil
	case strings.HasPrefix(low, "http://"), strings.HasPrefix(low, "https://"):
		return image, nil
	}
	path := image
	if strings.HasPrefix(low, "file://") {
		path = image[len("file://"):]
	}
	if strings.HasPrefix(path, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			path = filepath.Join(home, path[2:])
		}
	}
	if !filepath.IsAbs(path) {
		return "", searchMCPError("analyze_image takes an absolute file path (the gateway doesn't know your agent's working folder), an http(s) URL or a data URL: " + image + " is none of them")
	}
	if !local(r) {
		return "", searchMCPError("analyze_image reads a file only for an agent on the computer magpie runs on: from another computer, send the image as a data URL (data:image/png;base64,...) or an http(s) URL")
	}
	f, err := os.Open(path)
	if err != nil {
		return "", searchMCPError("analyze_image couldn't read " + path + ": " + err.Error())
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil && st.IsDir() {
		return "", searchMCPError(path + " is a folder, not an image")
	} else if err == nil && st.Size() > visionFileMax {
		return "", searchMCPError(fmt.Sprintf("%s is %d MB: analyze_image reads images up to %d MB", path, st.Size()>>20, visionFileMax>>20))
	}
	b, err := io.ReadAll(io.LimitReader(f, visionFileMax+1))
	if err != nil {
		return "", searchMCPError("analyze_image couldn't read " + path + ": " + err.Error())
	}
	if len(b) > visionFileMax {
		return "", searchMCPError(fmt.Sprintf("%s is over %d MB: analyze_image reads images up to %d MB", path, visionFileMax>>20, visionFileMax>>20))
	}
	mime := http.DetectContentType(b)
	if !strings.HasPrefix(mime, "image/") {
		return "", searchMCPError(path + " isn't an image analyze_image can read (PNG, JPEG, GIF or WebP): it reads as " + mime)
	}
	return "data:" + mime + ";base64," + base64.StdEncoding.EncodeToString(b), nil
}
