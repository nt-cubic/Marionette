package gateway

import (
	"bytes"
	"slices"
	"strings"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
)

// Claude Code's auto mode checks each action with a classifier (#250).
// Behind a gateway, its first requests ask the API to do it: the
// dangerous-tool-use beta and a `safeguards` field, answered by
// `safeguard_results` in the reply: a whole reply's own, a stream's in its
// message_delta. Anthropic's own API (and a relay in front of it) gets them
// as they were sent, relayed or built (buildAnthropic), and the results
// come back as it gave them, on a stream made of a whole reply too
// (anthropicWholeEvents). Any other model neither sees them nor answers
// them (serverReview), and Claude Code, finding no result, classifies
// locally for the rest of the session, naming the gateway in a notice.
// magpie says nothing on the API's behalf. Where no model Claude Code can
// reach through magpie is reviewed (AutoModeReviewed), the agent's wiring
// turns the review off in Claude Code's settings instead
// (CLAUDE_CODE_AUTO_MODE_SERVER=0, as Anthropic's page on the notice says to
// for a gateway that can't run the checks), so it classifies locally from
// the start and says nothing about the gateway (1EFLOPS on X). Where one
// is, it is left on: /model can take the same session to that model,
// which reviews it.
//
// Locally, the classifier is a request of its own on the session's model:
// the transcript in <transcript> blocks, a system prompt asking for
// <block>yes</block> or <block>no</block>, thinking turned off, and for its
// first stage 64 tokens to answer in (stopping at </block>). A model that
// reasons whatever it is told spends those 64 tokens reasoning and answers
// nothing, which Claude Code reads as no verdict and blocks the action.

// serverReviewBeta begins the beta Claude Code asks the review by
// (dangerous-tool-use-2026-09-03).
const serverReviewBeta = "dangerous-tool-use-"

func isServerReviewBeta(b string) bool { return strings.HasPrefix(b, serverReviewBeta) }

// serverReview fits auto mode's review to what a request to an Anthropic
// endpoint reaches: the betas asked, fitted to p (betas), and the body.
// Only Claude reviews actions, so a body naming another vendor's model
// (DeepSeek's, Kimi's, behind their Anthropic APIs) goes without the beta
// and the safeguards: a vendor that checks the fields it is sent turns the
// whole request away over them. Neither does a body p is no longer asked
// the beta for, having turned it away (Bedrock's "Unexpected value(s)"):
// the safeguards ask for a review that the beta turns on. A body that names
// no model (the model in Bedrock's path), or goes to Anthropic's own API,
// is Claude's.
func (s *Server) serverReview(p provider.Provider, asked []string, body []byte) ([]string, []byte) {
	model := gjson.GetBytes(body, "model").String()
	claude := model == "" || AutoModeReviewed(p, model)
	if !claude {
		asked = slices.DeleteFunc(slices.Clone(asked), isServerReviewBeta)
	}
	betas := s.betas(p, asked)
	if !claude || slices.ContainsFunc(asked, isServerReviewBeta) && !slices.ContainsFunc(betas, isServerReviewBeta) {
		if gjson.GetBytes(body, "safeguards").Exists() {
			body = withoutFields(body, "safeguards")
		}
	}
	return betas, body
}

// AutoModeReviewed reports whether a request for model on p can have its
// actions reviewed by Anthropic for Claude Code's auto mode: the Claude
// subscription (the bridge hands the review to Claude Code's own binary),
// a Claude model (Anthropic's API, a relay in front of it, Bedrock's or
// Vertex's names for it), or anything on Anthropic's own API. No other
// vendor runs Anthropic's classifier, and magpie answers none on its
// behalf.
func AutoModeReviewed(p provider.Provider, model string) bool {
	return p.Account != nil && p.Account.Agent == "claude" || anthropicModel.MatchString(model) || provider.HostOf(p.Base(provider.Anthropic)) == "api.anthropic.com"
}

// classifierRoom is what a model that reasons is given on top of the
// classifier's own budget, as Claude Code gives a Claude model that can't
// turn its thinking off.
const classifierRoom = 2048

// autoModeClassifier reports whether req is Claude Code's auto mode
// classifier asking about an action.
func autoModeClassifier(req *Request) bool {
	if len(req.Tools) > 0 || !strings.Contains(req.System, "<block>") && !strings.Contains(req.System, "<severity>") {
		return false
	}
	for _, m := range req.Messages {
		if m.Role != "user" {
			continue
		}
		for _, p := range m.Parts {
			if p.Kind == Text && strings.HasPrefix(strings.TrimSpace(p.Text), "<transcript>") {
				return true
			}
		}
	}
	return false
}

// hasReply reports whether msgs hold a reply: without one, a request is
// a one-off ask however many user messages it comes in.
func hasReply(msgs []Message) bool {
	for _, m := range msgs {
		if m.Role == "assistant" {
			return true
		}
	}
	return false
}

// fitAutoModeClassifier asks a model that isn't Claude, for Claude Code's
// auto mode classifier, to reason least — none when it can go without, its
// lowest level otherwise, left to the vendor when its levels aren't known —
// with room for that reasoning and the verdict.
func fitAutoModeClassifier(p provider.Provider, model string, req *Request) {
	if anthropicModel.MatchString(model) || !autoModeClassifier(req) {
		return
	}
	if req.MaxTokens > 0 && req.MaxTokens < classifierRoom {
		req.MaxTokens += classifierRoom
	}
	if req.Effort == "" {
		if levels := p.Efforts(model); len(levels) > 0 {
			req.Effort = fitEffort("none", levels)
		}
	}
}

// autoModeClassifierBody is fitAutoModeClassifier for a request relayed to
// an Anthropic endpoint as it was sent: a model that isn't Claude gets the
// room to reason in. The rest of the body is left alone.
func autoModeClassifierBody(model string, body []byte) []byte {
	if anthropicModel.MatchString(model) || !bytes.Contains(body, []byte("<transcript>")) || !bytes.Contains(body, []byte("<block>")) && !bytes.Contains(body, []byte("<severity>")) {
		return body
	}
	req, err := parseAnthropic(body)
	if err != nil || !autoModeClassifier(req) {
		return body
	}
	asked := gjson.GetBytes(body, "max_tokens").Int()
	if asked <= 0 || asked >= classifierRoom {
		return body
	}
	return withFields(body, map[string]any{"max_tokens": asked + classifierRoom})
}
