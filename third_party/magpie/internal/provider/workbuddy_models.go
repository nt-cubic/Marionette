package provider

// PLUGIN-SERVED (see AGENTS.md): WorkBuddy ("workbuddy" and "workbuddy-ai")
// is a deprecated built-in subscription served by its plugin,
// @magpie-community/opencode-workbuddy-auth, once moved onto it
// (provider.Moved; the default for a new sign-in). A moved one's sign-ins,
// models, requests and usage are all the plugin's, never this code's (only
// the move, in migrate*.go, still reads its accounts). A fix here alone
// doesn't reach those users; fix the plugin
// (github.com/magpie-community/plugins, packages/workbuddy) and raise the
// mover's min in internal/provider/migrate_workbuddy.go.

// A WorkBuddy plan's models are the ones WorkBuddy's product config gives
// its CLI agent: GET /v3/config, signed as a chat is, answers with the
// product's agents (the "cli" one's models are WorkBuddy's picker) and each
// model's details. Which product answers goes by the User-Agent: WorkBuddy
// sends "CLI/<version> WorkBuddy/<version>", and a bare WorkBuddy/<version>
// gets CodeBuddy IDE's config, with no cli agent.

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"net/http"
	"slices"
	"strconv"
	"strings"

	"github.com/yetone/magpie/internal/catalog"
)

type wbProductConfig struct {
	Agents []struct {
		Name   string   `json:"name"`
		Models []string `json:"models"`
	} `json:"agents"`
	Models []struct {
		ID              string `json:"id"`
		Name            string `json:"name"`
		MaxInputTokens  int    `json:"maxInputTokens"`
		MaxOutputTokens int    `json:"maxOutputTokens"`
		SupportsImages  *bool  `json:"supportsImages"`
		OnlyReasoning   bool   `json:"onlyReasoning"`
		// what a request costs of the plan's credits, as WorkBuddy's
		// picker shows it: "x0.00" (free), "x0.03", "x1.00"
		Credits   json.RawMessage `json:"credits"`
		Reasoning struct {
			SupportedEfforts   []string `json:"supportedEfforts"`
			CanDisableThinking *bool    `json:"canDisableThinking"`
		} `json:"reasoning"`
	} `json:"models"`
}

// wbFetchModels asks WorkBuddy's product config for the plan's models,
// with sign signing the request as the account's chats are.
func wbFetchModels(ctx context.Context, w *wbSite, sign func(context.Context, *http.Request, []byte) error) ([]catalog.Model, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, w.api()+"/v3/config", nil)
	if err != nil {
		return nil, err
	}
	if err := sign(ctx, req, nil); err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "CLI/"+wbAppVersion+" WorkBuddy/"+wbUAVersion)
	req.Header.Set("X-Requested-With", "XMLHttpRequest")
	req.Header.Set("Accept", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	b, _ := io.ReadAll(io.LimitReader(res.Body, 8<<20))
	var env struct {
		Code int             `json:"code"`
		Msg  string          `json:"msg"`
		Data wbProductConfig `json:"data"`
	}
	if res.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("WorkBuddy's config: %s", APIError(b, res.Status))
	}
	if err := json.Unmarshal(b, &env); err != nil {
		return nil, fmt.Errorf("WorkBuddy's config: %w", err)
	}
	if env.Code != 0 {
		return nil, fmt.Errorf("WorkBuddy's config: %s (%d)", env.Msg, env.Code)
	}
	ms := w.textOnly(env.Data.cliModels())
	if len(ms) == 0 {
		return nil, fmt.Errorf("WorkBuddy's config lists no models for its CLI agent")
	}
	return ms, nil
}

func (c wbProductConfig) cliModels() []catalog.Model {
	var out []catalog.Model
	for _, a := range c.Agents {
		if a.Name != "cli" {
			continue
		}
		for _, id := range a.Models {
			m := catalog.Model{ID: id, Name: id}
			for _, d := range c.Models {
				if d.ID != id {
					continue
				}
				if d.Name != "" {
					m.Name = d.Name
				}
				m.Context, m.Output = d.MaxInputTokens, d.MaxOutputTokens
				if r, ok := wbCredits(d.Credits); ok {
					m.Free, m.Rate = r == 0, r
				}
				if d.SupportsImages != nil {
					m.Images, m.ImageInput = *d.SupportsImages, d.SupportsImages
				}
				// the levels WorkBuddy offers the model, and off when it
				// offers that; a model it gives no levels takes any
				if es := d.Reasoning.SupportedEfforts; len(es) > 0 {
					if !d.OnlyReasoning && (d.Reasoning.CanDisableThinking == nil || *d.Reasoning.CanDisableThinking) {
						m.Efforts = append(m.Efforts, "none")
					}
					m.Efforts = append(m.Efforts, es...)
				}
				break
			}
			out = append(out, m)
		}
	}
	return wbDistinctNames(out)
}

// textOnly marks the models of ms whose images w drops as taking none,
// whatever WorkBuddy's config says: WorkBuddy AI's GLM models. On
// 2026-10-13, with a real WorkBuddy AI account (Yirasumi on Discord),
// www.workbuddy.ai's /v3/config marked every model supportsImages: true,
// but a chat to glm-5.3-flash, glm-5.3 or glm-5.2 came back saying
// "[System reminder: the current model does not support images. Content
// filtered. ...]": the server had taken the image out. Its other models
// (deepseek-v4.1-flash, kimi, gemini, grok, gpt) read them, and WorkBuddy
// (China)'s glm-5.3-flash does too, so this is WorkBuddy AI's GLM family
// alone. Said text-only, magpie's image-describing model reads the image
// for them; a user's own answer (settings' ModelImages) still wins over
// this, in entryFor. ms is not changed; the result is a copy.
func (w *wbSite) textOnly(ms []catalog.Model) []catalog.Model {
	if w.id != WorkBuddyAIID {
		return ms
	}
	out := slices.Clone(ms)
	for i := range out {
		if strings.HasPrefix(out[i].ID, "glm-") {
			no := false
			out[i].Images, out[i].ImageInput = false, &no
		}
	}
	return out
}

// wbCredits reads a model's credits, the multiple WorkBuddy's picker shows
// by it: "x0.03", "0" or 0; "x0.00" is free. A rate it can't read (or
// none) is not ok, and the model is neither free nor rated.
func wbCredits(raw json.RawMessage) (float64, bool) {
	var s string
	if json.Unmarshal(raw, &s) != nil {
		s = string(raw)
	}
	s = strings.TrimPrefix(strings.ToLower(strings.TrimSpace(s)), "x")
	f, err := strconv.ParseFloat(s, 64)
	return f, err == nil && f >= 0 && !math.IsInf(f, 1)
}

// wbDistinctNames tells apart models the config gives the same name:
// WorkBuddy AI lists deepseek-v4.1-flash (free) and deepseek-v4.1-flash-sg
// (Singapore, x0.03 credits) both as "Deepseek-V4.1-Flash". A later one is
// named for what its id adds to the first's ("… (SG)"), or else its id.
func wbDistinctNames(ms []catalog.Model) []catalog.Model {
	first := map[string]string{} // name → the id that has it
	for i, m := range ms {
		id, ok := first[m.Name]
		if !ok {
			first[m.Name] = m.ID
			continue
		}
		tag := m.ID
		if rest, ok := strings.CutPrefix(m.ID, id+"-"); ok && rest != "" {
			tag = strings.ToUpper(rest)
		}
		ms[i].Name = m.Name + " (" + tag + ")"
	}
	return ms
}
