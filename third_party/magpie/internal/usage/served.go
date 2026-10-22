package usage

import (
	"regexp"
	"strings"
	"unicode"

	"github.com/yetone/magpie/internal/provider"
)

// Which model answered: a vendor may serve a request with another model
// than the one asked for — a cheaper one when it's busy — and its reply
// says so in its model field. Most echo the name asked for, or its dated
// or pinned version (gpt-5 as gpt-5-2025-08-07, claude-sonnet-4-5 as
// claude-sonnet-4-5-20250929, gemini-2.5-pro as models/gemini-2.5-pro-001),
// which is the same model.

// versionTail is what a vendor puts after a model's name for the version
// it answered with: a date, a build number, Bedrock's v1:0, latest,
// preview. Not v3 alone: deepseek-v3 is another model than deepseek-v2.
var versionTail = regexp.MustCompile(`(?:[-_@:](?:\d{4}-\d{2}-\d{2}|\d{2}-\d{2}|\d{6,8}|\d{3,4}|v\d+:\d+|latest|preview|exp))+$`)

// vendorDot is Bedrock's region and maker before a model's name
// (us.anthropic.claude-…), the geography of an inference profile as
// catalog's bedrockGeos has them: global. and us-gov. among them.
var vendorDot = regexp.MustCompile(`^(?:(?:global|us-gov|[a-z]{2,4})\.)?(?:anthropic|amazon|meta|mistral|cohere|ai21|deepseek|qwen|openai|google|moonshotai|minimax|zai)\.`)

// contextTail is what Claude Code writes after a model's name for the size of
// its context: claude-opus-5[1m].
var contextTail = regexp.MustCompile(`\[[^\]]*\]$`)

// windowTail is the window magpie offers a model at beside its default
// (grok-4.7@500k, Cursor's grok-4.7@256k): the vendor answers under the
// plain name, which is the same model (#1645).
var windowTail = regexp.MustCompile(`@\d+[km]?$`)

// bareModel is a model's name without its maker or path, its version, the
// size of its context or its case.
func bareModel(m string) string {
	m = contextTail.ReplaceAllString(strings.ToLower(strings.TrimSpace(m)), "")
	m = windowTail.ReplaceAllString(m, "")
	if i := strings.LastIndexByte(m, '/'); i >= 0 {
		m = m[i+1:]
	}
	m = vendorDot.ReplaceAllString(m, "")
	if loc := versionTail.FindStringIndex(m); loc != nil && loc[0] > 0 {
		m = m[:loc[0]]
	}
	return m
}

// Swapped reports whether served is another model than sent: not the same
// name, however dated, pinned or prefixed. A call that went out as one level
// of a model (an Antigravity account's gemini-3.8-flash-medium) answered
// under the model's own name is that model at that level: Antigravity's
// reply names the family, not the variant (#462). Another level is another
// model, as AntigravitySentID has it. A vendor's "auto" (Copilot's,
// Cursor's) asked it to pick, so whichever answers wasn't swapped in, nor
// is the member another magpie's routing group sent it to (GroupRouted),
// nor a vendor's own name for the deployment serving the model
// (ServingName), nor the model a relay sold under a name of its own
// (RelayRenamed).
func Swapped(sent, served string) bool {
	a, b := bareModel(sent), bareModel(served)
	return a != "" && b != "" && squash(a) != squash(b) && a != "auto" && squash(bareModel(provider.EffortFamily(a))) != squash(b) && !GroupRouted(sent, served) && !ServingName(sent, served) && !RelayRenamed(sent, served)
}

// RelayRenamed reports whether sent is served's own name with a relay's
// word in front of it: a relay that sells kimi-k3 as moonshot-kimi-k3 is
// answered as kimi-k3 (#1498, liuweifeng), one that sells
// deepseek-v4.1-flash as claude-deepseek-v4.1-flash (#1383) as
// deepseek-v4.1-flash. What is left must name a version, so a bare word
// (flash, mini) is never taken for the model sent.
func RelayRenamed(sent, served string) bool {
	a, b := bareModel(sent), squash(bareModel(served))
	if b == "" || !strings.ContainsAny(b, "0123456789") || !strings.ContainsFunc(b, unicode.IsLetter) {
		return false
	}
	for i := 1; i < len(a); i++ {
		if a[i-1] == '-' && squash(a[i:]) == b {
			return true
		}
	}
	return false
}

// ServingName reports whether served is the vendor's own name for the
// deployment serving the model sent, which no request asks for by: Google's
// for a Gemini model (GeminiServing), xAI's for a Grok one (GrokServing).
func ServingName(sent, served string) bool {
	return GeminiServing(sent, served) || GrokServing(sent, served)
}

// GrokServing reports whether served is xAI's name for the build serving
// the Grok model sent: the model's own name with "-build" after its
// version. xAI answers grok-4.7 as grok-4.7-build, and grok-4.6 as
// grok-4.6-build (#1455, cybershape; two relays' logs say the same), and
// its own list (SuperGrok's, 2026-10-02) offers grok-4.7 and its fast
// variant grok-4.7-build-fast, never a grok-4.7-build to ask for. It holds
// for Grok models only, and for the same variant: grok-4.7 answered as
// grok-4.7-build-fast (the fast one), grok-4.6-build or grok-4.7-mini is
// still a swap, as is gpt-5 answered as gpt-5-build.
func GrokServing(sent, served string) bool {
	a := squash(bareModel(provider.EffortFamily(bareModel(sent))))
	b := bareModel(served)
	m := grokBuild.FindStringSubmatchIndex(b)
	if m == nil || !strings.HasPrefix(a, "grok") {
		return false
	}
	return squash(b[:m[2]]+b[m[3]:]) == a
}

// grokBuild is the "-build" after a Grok model's version (grok-4.7-build,
// grok-4.7-build-fast), the part GrokServing reads past.
var grokBuild = regexp.MustCompile(`^grok-[0-9][0-9.]*(-build)(?:-|$)`)

// GeminiServing reports whether served is the name Google's backend gives
// the deployment that serves the Gemini model sent: the model's own name
// with "-n" after it. Antigravity answers every level of gemini-3.8-flash
// (-low, -medium, -high) with modelVersion gemini-3.8-flash-n, an id no
// request can ask for (404) and fetchAvailableModels never lists, so it is
// that model, not another one swapped in (dumplings on Discord). It holds
// for Gemini models only: gpt-5 answered as gpt-5-n is still a swap, as is
// gemini-3.8-flash answered as gemini-3.8-flash-lite or gemini-3.7-flash-n.
func GeminiServing(sent, served string) bool {
	a := squash(bareModel(provider.EffortFamily(bareModel(sent))))
	b, ok := strings.CutSuffix(bareModel(served), "-n")
	return ok && strings.HasPrefix(a, "gemini") && squash(b) == a
}

// squash is a bare name without its separators, so a vendor that writes
// the dots of a version as hyphens (Volcengine Ark: deepseek-v4.1-flash
// answered as deepseek-v4-1-flash, vincentzhang on Discord) or underscores
// names the same model.
var squash = strings.NewReplacer(".", "", "-", "", "_", "", " ", "").Replace

// SameSpelled reports whether served is sent spelled with other separators
// or case: a row or route kept marked swapped before Swapped knew it is
// read as not swapped.
func SameSpelled(sent, served string) bool {
	a := squash(bareModel(sent))
	return a != "" && a == squash(bareModel(served))
}

// GroupRouted reports whether sent is a routing group of another magpie
// (a remote magpie's "group/<id>", the one name only a magpie answers
// to) and served the model its reply named: the member the group routed
// the request to, which is the group doing its job, not the vendor
// serving another model than asked (莫 on Discord: group/auto-… answered
// by deepseek/deepseek-v4.1-flash, marked a swap).
func GroupRouted(sent, served string) bool {
	return strings.TrimSpace(served) != "" && remoteGroup(strings.TrimSpace(sent))
}

// remoteGroup reports whether id is a routing group's id as a magpie
// gives it: group/<id>, or auto/<model> for a group it found.
func remoteGroup(id string) bool {
	return strings.HasPrefix(id, provider.GroupPrefix) || strings.HasPrefix(id, provider.AutoPrefix)
}
