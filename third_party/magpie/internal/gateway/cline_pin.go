package gateway

import (
	"bytes"
	"encoding/json"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
)

// clinePin is body with its DeepSeek or GLM model pinned to its maker's
// own API (DeepSeek's, Z.ai's) on the Cline API, when the provider asks it
// (provider.PinUpstream). A model Cline sends through Vercel's AI Gateway
// gets providerOptions.gateway.only, which that gateway reads to serve the
// model from that host alone, as ClinePass + ProviderPin set it
// (linux.do/t/topic/2918145); one it sends through OpenRouter gets
// provider.only, OpenRouter's own field (sdytman on Discord). A ClinePass
// DeepSeek model's carries the gateway's has filter beside only, which
// ClinePass needs for the pin to hold (provider.ClinePassHas). A body
// naming provider or providerOptions itself is left as the client sent
// it. The field goes at the end, so the prefix a cache keys on doesn't
// change.
func clinePin(p provider.Provider, to provider.Protocol, body []byte) []byte {
	if to != provider.Chat {
		return body
	}
	up, openRouter := p.ClinePin(gjson.GetBytes(body, "model").String())
	if up == "" || gjson.GetBytes(body, "providerOptions").Exists() || gjson.GetBytes(body, "provider").Exists() {
		return body
	}
	end := bytes.LastIndexByte(body, '}')
	start := bytes.IndexByte(body, '{')
	if end < 0 || start < 0 || start > end {
		return body
	}
	gw := map[string]any{"only": []string{up}}
	if has := p.ClinePassHas(gjson.GetBytes(body, "model").String(), chatImages(body)); has != nil {
		gw["has"] = has
	}
	key, opts := "providerOptions", map[string]any{"gateway": gw}
	if openRouter {
		key, opts = "provider", map[string]any{"only": []string{up}}
	}
	val, _ := json.Marshal(opts)
	out := make([]byte, 0, len(body)+len(val)+24)
	out = append(out, body[:end]...)
	if len(bytes.TrimSpace(body[start+1:end])) > 0 {
		out = append(out, ',')
	}
	out = append(out, '"')
	out = append(out, key...)
	out = append(out, `":`...)
	out = append(out, val...)
	return append(out, body[end:]...)
}

// chatImages is whether a Chat body carries an image part
func chatImages(body []byte) bool {
	found := false
	gjson.GetBytes(body, "messages").ForEach(func(_, m gjson.Result) bool {
		m.Get("content").ForEach(func(_, part gjson.Result) bool {
			if t := part.Get("type").String(); t == "image_url" || t == "image" || t == "input_image" {
				found = true
			}
			return !found
		})
		return !found
	})
	return found
}
