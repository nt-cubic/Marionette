package gateway

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// A model that can't see images is given what a model that can says of
// them: each image in the request is described once by the vision model
// (the Settings' Vision, or one magpie picks) and the description goes in
// the image's place, in the current turn, older turns and tool results
// alike. With Vision off, or no model that sees, such an image is turned
// away as before (textOnlyBody).

// Agents are told a model that can't see takes images while one that can
// describes them to it.
func init() {
	provider.Described = func() bool {
		_, ok := seer()
		return ok
	}
}

// VisionAgent is the User-Agent of the descriptions magpie asks for.
const VisionAgent = "magpie-vision/1"

// visionTimeout is how long the vendor has to describe an image, from when
// the request went out to it (each try's own), and visionWaitMost how long
// a description may take in all, the waits for a slot or a resting provider
// with it (sideDeadline, #2318): a reasoning model describing a dense
// screenshot can take more than two minutes by itself.
var visionTimeout, visionWaitMost = 3 * time.Minute, 10 * time.Minute

// visionParallel is how many images of a request are described at once.
const visionParallel = 4

// sightsKept is how many descriptions are kept, the least recently used
// dropped first: an agent sends a conversation's images again every turn,
// and a long Computer Use one holds hundreds of screenshots (fflake33).
// A description is a few kilobytes.
const sightsKept = 2048

const visionSystem = `You describe images for an AI model that cannot see them. It will answer the user from your description alone, so leave nothing out that it may need.
- Transcribe all text exactly as written, keeping its layout: code, terminal output, error messages, logs, UI labels, menus, file names, numbers.
- For a screenshot of an app or page: which app or page it is, its layout, and the state of what is on it (selected, disabled, checked, highlighted, error states).
- For a chart or table: its kind, axes and labels, and every value you can read.
- For a diagram: its elements and how they are connected.
- For a photo or drawing: what it shows, with the details that matter.
Describe only what is there. Don't guess at what can't be read, say it can't be read. Don't answer questions or give advice. No preamble.`

// describingKey marks a description magpie asks for, which must not have
// its image described again.
type describingKey struct{}

func describing(ctx context.Context) bool {
	v, _ := ctx.Value(describingKey{}).(bool)
	return v
}

// describeForKey holds the request an image is described for: the Routing
// view names it by the description's row, in that request's session.
type describeForKey struct{}

type describeFor struct {
	call    *CallFor
	session string
}

func withDescribeFor(ctx context.Context, agent, model, session string, unknown bool) context.Context {
	return context.WithValue(ctx, describeForKey{}, describeFor{&CallFor{Agent: agent, Model: model, Unknown: unknown, Missing: VisionMissing()}, session})
}

func describedFor(ctx context.Context) *CallFor {
	f, _ := ctx.Value(describeForKey{}).(describeFor)
	return f.call
}

// blindTo is whether a describer is to describe images for pid/model: its
// list says it takes none (in), or says nothing and magpie counts it
// text-only — as agents were told before Vision had them told every model
// takes images (provider.Described): models.dev doesn't say it sees either.
func blindTo(pid, model string, in *bool) bool {
	if in != nil {
		return !*in
	}
	e, ok := provider.ServedEntryOf(pid + "/" + model)
	return ok && !e.Images
}

// seer is the model that describes images: the Settings' Vision while it
// resolves, else AutoVision's. None when Vision is off or no model sees.
// A Vision that no longer resolves (its provider removed or off, the model
// gone) is not described with in silence by AutoVision's: VisionMissing
// names it, the description's row in Routing and Settings › Models say it
// is missing, and a request turned away for want of a describer says so.
func seer() (string, bool) {
	switch v := provider.HeldSettings().Vision; v {
	case "off":
		return "", false
	case "":
	default:
		if pickedMissing(v) == "" {
			return v, true
		}
	}
	m := AutoVision()
	return m, m != ""
}

// VisionMissing is the model Settings › Models › Image recognition names
// when magpie can't find it any more, so that AutoVision's describes in its
// place. "" when none is named, Vision is off, or it resolves.
func VisionMissing() string { return pickedMissing(provider.HeldSettings().Vision) }

// pickedMissing is a model the user picked in Settings (v) that doesn't
// resolve: "" for none picked, off, or one magpie finds.
func pickedMissing(v string) string {
	if v == "" || v == "off" {
		return ""
	}
	if _, _, ok := provider.Resolve(v); ok {
		return ""
	}
	return v
}

// missingSeerNote is what a request turned away, or whose image wasn't
// described, says of a picked Image recognition model that is missing.
func missingSeerNote() string {
	if v := VisionMissing(); v != "" {
		return fmt.Sprintf("; the Image recognition model picked in magpie's Settings, %q, isn't set up any more: pick another in Settings → Models → Image recognition", v)
	}
	return ""
}

// textOnlyNote is what a request turned away for its image says of why
// magpie takes the model for text-only and where that is changed, so the
// reader turns on the model's switch rather than trying another model or
// reinstalling the agent (KimYx0207 on X, over mark_li21's Codex paste): the
// user's own answer, else what the provider's list says; for a group, that
// none of its members is marked as taking images.
func textOnlyNote(p provider.Provider, model string, group string) string {
	if group != "" {
		return fmt.Sprintf("; no model in the group %s is marked as taking images: turn on Accepts images for one that does, in its provider's Names & levels in magpie", group)
	}
	name := p.Name
	if name == "" {
		name = p.ID
	}
	if v, ok := provider.ImageOverride(p.ID, model); ok && !v {
		return fmt.Sprintf("; Accepts images is turned off for it in magpie, in Providers → %s → Names & levels", name)
	}
	return fmt.Sprintf("; %s's model list marks it as not taking images: if it does, turn on Accepts images for it in magpie, in Providers → %s → Names & levels", name, name)
}

// noSeerNote is what a request turned away for an image says of why no
// model described it: the picked one is missing, Image recognition is
// off, or none of the models the user enabled can see.
func noSeerNote() string {
	if n := missingSeerNote(); n != "" {
		return n
	}
	if provider.HeldSettings().Vision == "off" {
		return "; Image recognition is off in magpie's Settings → Models"
	}
	if _, ok := seer(); !ok {
		return "; none of the models enabled in magpie can see images to describe it: enable one that can, or pick one in Settings → Models → Image recognition"
	}
	return ""
}

// autoSeerNote is what a request whose image wasn't described says of a
// describer magpie picked itself, which the user may not know of.
func autoSeerNote(see string) string {
	if provider.HeldSettings().Vision != "" {
		return ""
	}
	return fmt.Sprintf("; magpie picked %s of the models you enabled: pick another in Settings → Models → Image recognition", see)
}

// AutoVision is the model magpie picks to describe images when the
// Settings name none: a small model that sees, of the models the user
// enabled on the providers switched on (smallModel) — a
// Claude account's, a Codex account's, the cheapest of an API key's,
// another account's. One with allowance left goes before one whose every
// account or key is out of it (smallReady), whatever their order: the
// one out is picked only when every model that sees is, so that agents
// are still told images are described (provider.Described), and
// askVisionWith says when it is back instead of asking (#1801). "" when
// no model sees.
func AutoVision() string {
	m, _ := autoVision()
	return m
}

// autoVision is AutoVision's pick, and whether it has allowance left.
func autoVision() (string, bool) {
	tier := func(p provider.Provider) int {
		switch {
		case p.Account != nil && p.Account.Agent == "claude":
			return 0
		case p.Account != nil && p.Account.Agent == "codex":
			return 1
		case p.Account == nil:
			return 2
		}
		return 3
	}
	best, bestTier, bestCost, bestReady := "", 0, 0.0, false
	for _, p := range provider.All() {
		if !p.On() || p.DecideOnly() {
			continue
		}
		t := tier(p)
		if bestReady && t > bestTier {
			continue
		}
		m, cost, ready := smallModelWhere(p, "sees", seesAt(p.ID))
		if m == "" {
			continue
		}
		switch {
		case best == "", ready && !bestReady:
		case ready != bestReady, t > bestTier:
			continue
		case t == bestTier && cost >= bestCost:
			continue
		}
		best, bestTier, bestCost, bestReady = p.ID+"/"+m, t, cost, ready
	}
	return best, bestReady
}

// autoSpent is why AutoVision's model isn't asked to describe an image:
// it is out of its allowance, as every model that sees is (spentModel),
// and would only be refused again. nil when it has allowance, or when
// the Settings name the model, which is asked as the user picked it.
func autoSpent(model string) error {
	if !autoDescribes() {
		return nil
	}
	p, m, ok := provider.Resolve(model)
	if !ok {
		return nil
	}
	back, spent := spentModel(p, m, provider.Chat, time.Now())
	if !spent {
		return nil
	}
	when := ""
	if !back.IsZero() {
		when = " until " + heldClock(back, time.Now())
	}
	return fmt.Errorf("%s is out of its allowance%s, as is every other model magpie could describe it with, and none is asked until one is back", model, when)
}

// autoDescribes is whether the model that describes images is
// AutoVision's: the Settings name none, or one that is missing.
func autoDescribes() bool {
	v := provider.HeldSettings().Vision
	return v == "" || pickedMissing(v) != ""
}

// sees is whether a model takes images.
func sees(m catalog.Model) bool {
	return m.Images && (m.ImageInput == nil || *m.ImageInput)
}

// seesAt is sees for provider pid's models, the user's answer first
// (provider.SetModelImage, magpie model spec --vision): a model they
// marked text-only is never picked to describe images, and one they
// marked as seeing is. smallKept is stamped by settings.json too, so a
// change is read on the next pick.
func seesAt(pid string) func(catalog.Model) bool {
	return func(m catalog.Model) bool {
		if v, ok := provider.ImageOverride(pid, m.ID); ok {
			return v
		}
		return sees(m)
	}
}

// sight is a description, once it is done.
type sight struct {
	done chan struct{}
	text string
	err  error
}

// describe is what model says of the image at src, asked once for each
// image: a request that sends it again, or sends it while it is being
// described, is given the same description.
func (s *Server) describe(ctx context.Context, model, src string) (string, error) {
	// a level picked since is asked again, not given what was said at the last
	sum := sha256.Sum256([]byte(model + "\x00" + visionEffort(model) + "\x00" + src))
	key := hex.EncodeToString(sum[:])
	s.sightMu.Lock()
	if e, ok := s.sights[key]; ok {
		// used again, so the last to go: a long conversation's first
		// images, still sent every turn, outlast those of conversations
		// that have ended
		if i := slices.Index(s.sightOrder, key); i >= 0 {
			s.sightOrder = append(append(s.sightOrder[:i:i], s.sightOrder[i+1:]...), key)
		}
		s.sightMu.Unlock()
		select {
		case <-e.done:
			return e.text, e.err
		case <-ctx.Done():
			return "", ctx.Err()
		}
	}
	if s.sights == nil {
		s.sights = map[string]*sight{}
	}
	e := &sight{done: make(chan struct{})}
	s.sights[key] = e
	s.sightOrder = append(s.sightOrder, key)
	for len(s.sightOrder) > sightsKept {
		delete(s.sights, s.sightOrder[0])
		s.sightOrder = s.sightOrder[1:]
	}
	s.sightMu.Unlock()
	e.text, e.err = s.askVision(ctx, model, src)
	if e.err != nil {
		// a failure isn't kept: the next request asks again
		s.sightMu.Lock()
		if s.sights[key] == e {
			delete(s.sights, key)
		}
		s.sightMu.Unlock()
	}
	close(e.done)
	return e.text, e.err
}

// askVision asks model through the gateway itself, as a client would, what
// the image at src shows.
func (s *Server) askVision(ctx context.Context, model, src string) (string, error) {
	return s.askVisionWith(ctx, model, src, visionSystem, "Describe this image.")
}

// askVisionWith asks model about the image at src: system, and the user's
// text beside the image.
func (s *Server) askVisionWith(ctx context.Context, model, src, system, text string) (string, error) {
	if err := autoSpent(model); err != nil {
		return "", err
	}
	ctx, cancel := sideDeadline(context.WithValue(ctx, describingKey{}, true), "image description", visionTimeout, visionWaitMost)
	defer cancel()
	req := map[string]any{
		"model":  model,
		"stream": false,
		"messages": []map[string]any{
			{"role": "system", "content": system},
			{"role": "user", "content": []map[string]any{
				{"type": "text", "text": text},
				{"type": "image_url", "image_url": map[string]string{"url": src}},
			}},
		},
		"max_tokens": 4096,
	}
	if effort := visionEffort(model); effort != "" {
		req["reasoning_effort"] = effort
		// a level picked in Settings is given room to think in
		if picked := sideEffort(model, provider.HeldSettings().VisionEffort); picked != "" {
			req["max_tokens"] = replyRoom(4096, picked)
		}
	}
	body, _ := json.Marshal(req)
	r, err := http.NewRequestWithContext(magpieChose(ctx), http.MethodPost, "http://magpie/v1/chat/completions", nil)
	if err != nil {
		return "", err
	}
	r.Header.Set("Content-Type", "application/json")
	r.Header.Set("User-Agent", VisionAgent)
	if f, _ := ctx.Value(describeForKey{}).(describeFor); f.session != "" {
		// in the Routing view beside the request it describes for
		r.Header.Set(SessionHeader, f.session)
	}
	w := httptest.NewRecorder()
	s.serve(w, r, provider.Chat, body)
	if t, ok := sideTimeout(ctx); ok {
		return "", t
	}
	if ctx.Err() != nil {
		return "", ctx.Err()
	}
	var out struct {
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
		Error *struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &out); err != nil {
		return "", fmt.Errorf("%d, not an answer", w.Code)
	}
	if w.Code >= 300 || out.Error != nil {
		msg := http.StatusText(w.Code)
		if out.Error != nil && out.Error.Message != "" {
			msg = out.Error.Message
		}
		return "", errors.New(msg)
	}
	if len(out.Choices) == 0 || strings.TrimSpace(out.Choices[0].Message.Content) == "" {
		return "", errors.New("no description")
	}
	return strings.TrimSpace(out.Choices[0].Message.Content), nil
}

// seenBody is the body with each of its images described by model in its
// place. An image of the latest turn that can't be described is an error:
// the model would be asked about what it isn't given. One in an older turn
// or a tool's result is left out, as textOnlyBody leaves it.
func (s *Server) seenBody(ctx context.Context, proto provider.Protocol, body []byte, model string) ([]byte, error) {
	var srcs []string
	have := map[string]bool{}
	walkImages(proto, body, func(im imageAt) (json.RawMessage, error) {
		if im.Src != "" && !have[im.Src] {
			have[im.Src] = true
			srcs = append(srcs, im.Src)
		}
		return nil, nil
	})
	type result struct {
		text string
		err  error
	}
	results := make([]result, len(srcs))
	var wg sync.WaitGroup
	slots := make(chan struct{}, visionParallel)
	for i, src := range srcs {
		wg.Add(1)
		go func() {
			defer wg.Done()
			slots <- struct{}{}
			defer func() { <-slots }()
			text, err := s.describe(ctx, model, src)
			results[i] = result{text, err}
		}()
	}
	wg.Wait()
	of := map[string]result{}
	for i, src := range srcs {
		of[src] = results[i]
	}
	return walkImages(proto, body, func(im imageAt) (json.RawMessage, error) {
		if im.Src == "" {
			if im.Current() {
				return nil, errors.New("the image is a file only its vendor can open")
			}
			return textBlock(proto, imageOmitted), nil
		}
		r := of[im.Src]
		if r.err != nil {
			if im.Current() {
				return nil, r.err
			}
			return textBlock(proto, "[Image omitted: this model accepts text only, and the image couldn't be described.]"), nil
		}
		// on its own lines: a translation may join it to the text around it
		return textBlock(proto, "\n[Image, as "+model+" describes it for this model, which can't see images:]\n"+r.text+"\n[End of the image's description]\n"), nil
	})
}

// smallModelWhere is smallModel among the provider's models that keep
// holds, what it costs, and whether it has allowance left (smallReady);
// "" when there is none. kind names keep (keptSmallCands).
func smallModelWhere(p provider.Provider, kind string, keep func(catalog.Model) bool) (string, float64, bool) {
	c, ready := smallReadyOf(p, keptSmallCands(p, kind, keep), provider.Chat)
	if c.id == "" {
		return "", 0, false
	}
	return c.id, c.cost, ready
}
