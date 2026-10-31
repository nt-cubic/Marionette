package gateway

// A rule with an intent (provider.Rule.Intent) is for a kind of message:
// as a user's turn begins, the group's classifier model is asked which of
// the intents the user's message is — once, before the turn goes anywhere,
// and never again within it. The call goes through magpie itself, so the
// classifier is any model magpie has, with its keys, failover and usage;
// it shows in the usage as magpie's own call. When it can't say
// (it fails, is slow, or answers something else) no intent matches, and
// the trace tells why.

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"net/http/httptest"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// RouterAgent is the User-Agent magpie's own classifier calls carry.
const RouterAgent = "magpie-router/1"

var (
	classifyKeep = 10 * time.Minute // an answer, for the same message and intents
	classifyRest = 30 * time.Second // after a failure, before the classifier is asked again
)

// classifyTimeout is how long the classifier is given to answer, its
// provider's failover included. It is Settings' Classifier timeout (#2934:
// a Kiro subscription's haiku needed more than the 8 seconds it was fixed
// at), read as each turn asks, in classifyUnit seconds.
func classifyTimeout() time.Duration {
	return settings.Shared().ClassifyWait() / time.Second * classifyUnit
}

// classifyUnit is a second of classifyTimeout: a test makes it shorter,
// rather than wait the seconds out.
var classifyUnit = time.Second

// classifier asks a model which of intents text is — the intent, or ""
// for none of them — and, when effort, how much reasoning the turn wants. prev is what
// the conversation's turn before was said to be: a message that only
// carries on from it is the same.
type classifier func(model string, intents []string, prev before, effort bool, text string) (verdict, error)

// before is what the classifier said of a conversation's turn before, and
// what was said in it (turnBefore): a short follow-up (继续, fix that) says
// little of its own, and is judged with what it follows.
type before struct {
	Intent string // one of the intents asked about now, or ""
	Effort string // the reasoning picked for it, or ""
	User   string // the user's message before this one (userSaid), cut to priorCap; "" for none
	Reply  string // the assistant's last words before this message, its text alone, cut to priorCap; "" for none
}

// verdict is what a classifier said.
type verdict struct {
	Intent string  // "" for none
	Effort string  // the reasoning the turn wants; "" when not asked
	Score  float64 // how hard the classifier took the turn to be, from 0 (low) to 3 (xhigh)
	Sure   float64 // how confident Jev was of the intent, from 0 to 1
}

var classified = struct {
	sync.Mutex
	m      map[string]classifiedAs
	failed map[string]classifyFailure // classifier model → its last failure
	// last: classifier model → why it couldn't say when last asked, an
	// answer that wasn't one among them; gone once it answers
	// (ClassifierFailure, #1929)
	last map[string]classifyFailure
	// group: routing group id → why its classifier couldn't say when that
	// group last asked it; gone once it answers that group
	// (GroupClassifierFailure)
	group    map[string]groupFailure
	inflight map[string]*classifyCall
}{
	m:        map[string]classifiedAs{},
	failed:   map[string]classifyFailure{},
	last:     map[string]classifyFailure{},
	group:    map[string]groupFailure{},
	inflight: map[string]*classifyCall{},
}

type groupFailure struct {
	classifyFailure
	model string
}

type classifyCall struct {
	done chan struct{}
	v    verdict
	err  error
}

type classifiedAs struct {
	v  verdict
	at time.Time
}

type classifyFailure struct {
	err string
	at  time.Time
}

// answerError is a classifier that answered, but not with one of the
// numbers: it is up, so it isn't left to rest for it.
type answerError struct{ error }

// classify is ask's answer for the message, from what was asked before
// when it can be: the same message and intents within classifyKeep. A
// classifier that failed is left to rest for classifyRest rather than
// making every turn wait out its timeout. Concurrent calls asking the
// same question share the answer in flight.
func classify(ask classifier, model string, intents []string, prev before, effort bool, text string) (v verdict, cached bool, err error) {
	h := sha256.New()
	h.Write([]byte(model + "\x00" + strings.ToLower(strings.Join(intents, "\x00")) + "\x00" + prev.Intent + "\x00" + prev.Effort + "\x00" + prev.User + "\x00" + prev.Reply + "\x00" + strconv.FormatBool(effort) + "\x00" + text))
	key := hex.EncodeToString(h.Sum(nil))
	now := time.Now()
	classified.Lock()
	if c, ok := classified.m[key]; ok && now.Sub(c.at) < classifyKeep {
		classified.Unlock()
		return c.v, true, nil
	}
	if c, ok := classified.inflight[key]; ok {
		classified.Unlock()
		<-c.done
		return c.v, c.err == nil, c.err
	}
	if f, ok := classified.failed[model]; ok && now.Sub(f.at) < classifyRest {
		classified.Unlock()
		return verdict{}, false, fmt.Errorf("%s failed %s ago (%s); not asked again for now", model, now.Sub(f.at).Round(time.Second), f.err)
	}
	c := &classifyCall{done: make(chan struct{}), err: errors.New("classifier did not finish")}
	classified.inflight[key] = c
	classified.Unlock()
	// Release waiters even if ask panics; its caller still sees the panic.
	defer func() {
		classified.Lock()
		delete(classified.inflight, key)
		close(c.done)
		classified.Unlock()
	}()
	v, err = ask(model, intents, prev, effort, text)
	classified.Lock()
	defer classified.Unlock()
	c.err = err
	if err != nil {
		f := classifyFailure{err: err.Error(), at: time.Now()}
		if !errors.As(err, new(answerError)) {
			classified.failed[model] = f
		}
		if was, ok := classified.last[model]; !ok || was.err != f.err {
			// said once in the log, not at every turn it fails the same way
			log.Printf("routing: the classifier %s couldn't say: %s", model, f.err)
		}
		classified.last[model] = f
		return verdict{}, false, err
	}
	c.v = v
	delete(classified.failed, model)
	delete(classified.last, model)
	classified.m[key] = classifiedAs{v: v, at: time.Now()}
	if len(classified.m) > 4096 {
		for k, c := range classified.m {
			if time.Since(c.at) > classifyKeep {
				delete(classified.m, k)
			}
		}
	}
	return v, false, nil
}

// classifiedFor notes what group's classifier, model, gave the group's
// turn: err, or an answer. A failure is the one the classifier last had
// (classified.last), resting from it or not, so the group's card says the
// vendor's words and when they were said, not the rest's.
func classifiedFor(group, model string, err error) {
	classified.Lock()
	defer classified.Unlock()
	if err == nil {
		delete(classified.group, group)
		return
	}
	f, ok := classified.last[model]
	if !ok {
		f = classifyFailure{err: err.Error(), at: time.Now()}
	}
	classified.group[group] = groupFailure{classifyFailure: f, model: model}
}

// GroupClassifierFailure is why group's classifier, model, couldn't say
// which intent or effort a turn of that group's was, the last time the
// group asked it, and when; ok is false once it answers the group, before
// the group asks it, and after the group's classifier changed. The Routing
// page says it on that group's card alone: keyed by the classifier, a
// group no request reached showed the failures of another group that
// shares its classifier, as though its own turns were being judged (jjt on
// Discord: "claude难易", not in use, said Claude Haiku's weekly limit).
func GroupClassifierFailure(group, model string) (why string, at time.Time, ok bool) {
	classified.Lock()
	defer classified.Unlock()
	f, ok := classified.group[group]
	if !ok || f.model != model {
		return "", time.Time{}, false
	}
	return f.err, f.at, true
}

// ClassifierFailure is why model, a routing group's classifier, couldn't
// say which intent or effort a turn is the last time it was asked, and
// when; ok is false once it answers, and before it is asked. The Routing
// page says it on the group's card (#1929, versun): a classifier that fails
// at every turn otherwise only showed in each request's own details, while
// its group's rules with an intent quietly never matched.
func ClassifierFailure(model string) (why string, at time.Time, ok bool) {
	classified.Lock()
	defer classified.Unlock()
	f, ok := classified.last[model]
	return f.err, f.at, ok
}

// userText is what the user said to begin the turn: the words (userSaid)
// of the last user message that has any — not what agents add to it, or
// put in after it, for the model (<system-reminder>…, a task
// notification, an agent list) — its middle left out when long.
func userText(req *Request) string {
	_, text := lastSaid(req, len(req.Messages))
	const head, tail = 3000, 1000
	return cutMiddle(text, head, tail)
}

// lastSaid is the last user message before index end that has words of
// the user's (userSaid): its index and those words, or -1 and "".
func lastSaid(req *Request, end int) (int, string) {
	for i := min(end, len(req.Messages)) - 1; i >= 0; i-- {
		if m := req.Messages[i]; m.Role == "user" {
			if text := userSaid(m); text != "" {
				return i, text
			}
		}
	}
	return -1, ""
}

// cutMiddle is text with its middle left out when it is longer than
// head+tail runes.
func cutMiddle(text string, head, tail int) string {
	if r := []rune(text); len(r) > head+tail {
		return string(r[:head]) + "\n…\n" + string(r[len(r)-tail:])
	}
	return text
}

// priorCap is how much of each of the turn before's two texts the
// classifier is told (turnBefore), in runes: a quarter of the message's
// own 4000 (userText), so the two together add at most half of what the
// message may, and room for a request or an answer's last paragraph.
const priorCap = 1000

// turnBefore is what was said in the conversation before the user's
// message the classifier is asked about (userText's): the user's message
// before it (userSaid, its middle left out past priorCap, as the
// message's is) and the assistant's last reply between the two — its
// text alone, its end kept past priorCap, where an answer sums up or
// asks. A tool call's arguments, a tool's result, thinking, a search's
// hits and the system prompt are never in either; a reply of only tool
// calls is none.
func turnBefore(req *Request) (user, reply string) {
	at, _ := lastSaid(req, len(req.Messages))
	if at < 0 {
		return "", ""
	}
	from, user := lastSaid(req, at)
	for i := at - 1; i > from && reply == ""; i-- {
		if m := req.Messages[i]; m.Role == "assistant" {
			reply = assistantSaid(m)
		}
	}
	if r := []rune(reply); len(r) > priorCap {
		reply = "…" + string(r[len(r)-priorCap:])
	}
	return cutMiddle(user, priorCap*3/4, priorCap/4), reply
}

// assistantSaid is the text an assistant message holds: no tool call,
// thinking or search in it.
func assistantSaid(m Message) string {
	var said []string
	for _, p := range m.Parts {
		if p.Kind == Text {
			if t := strings.TrimSpace(p.Text); t != "" {
				said = append(said, t)
			}
		}
	}
	return strings.Join(said, "\n")
}

const classifyPrompt = "You route a user's message to a coding assistant. " +
	"Given numbered kinds and the user's message, answer with the number of the kind that fits the message best. " +
	"The kinds may be topics, or levels such as how hard or how big a request is; " +
	"when they are levels, every message has one, a greeting or a question about the assistant included. " +
	"Answer 0 only when the message is plainly none of the kinds. Answer with the number only."

// classifyBody is the Chat request asking model which of intents text is,
// at effort when it isn't "". A model that reasons does so before it
// answers, and how long varies: 2048 leaves room for that and the number.
// When the turn before was one of intents (prev), it is told so: "go on"
// says nothing of its own, and is of the kind of what it goes on with.
// What was said before the message (prev.User, prev.Reply) goes with it,
// as Jev's state carries it (jevState).
func classifyBody(model, effort string, intents []string, prev before, text string) []byte {
	var b strings.Builder
	b.WriteString("Kinds:\n")
	for i, in := range intents {
		fmt.Fprintf(&b, "%d. %s\n", i+1, in)
	}
	writePrior(&b, prev)
	if i := slices.Index(intents, prev.Intent); i >= 0 {
		fmt.Fprintf(&b, "\nThe user's message before this one, in the same conversation, was of kind %d. "+
			"A message that only carries on from it — go on, yes, do it, fix that — is of kind %d too; "+
			"one that asks for something of its own is of the kind that fits it.\n", i+1, i+1)
	}
	b.WriteString("\nThe user's message:\n<message>\n")
	b.WriteString(text)
	b.WriteString("\n</message>\n\nThe number of the kind that fits it best (0 only if none does):")
	req := map[string]any{
		"model": model,
		"messages": []map[string]string{
			{"role": "system", "content": classifyPrompt},
			{"role": "user", "content": b.String()},
		},
		"stream":      false,
		"temperature": 0,
		"max_tokens":  2048,
	}
	if effort != "" {
		req["reasoning_effort"] = effort
	}
	body, _ := json.Marshal(req)
	return body
}

// writePrior tells a classifier asked in words what was said before the
// message (turnBefore), when anything was: a short follow-up is judged
// with what it follows.
func writePrior(b *strings.Builder, prev before) {
	if prev.User == "" && prev.Reply == "" {
		return
	}
	b.WriteString("\nWhat came before the user's message, in the same conversation, to judge a short follow-up (go on, yes, fix that) by:\n")
	if prev.User != "" {
		b.WriteString("<previous_user_message>\n" + prev.User + "\n</previous_user_message>\n")
	}
	if prev.Reply != "" {
		b.WriteString("<previous_assistant_reply>\n" + prev.Reply + "\n</previous_assistant_reply>\n")
	}
}

var firstNumber = regexp.MustCompile(`\d+`)

// readIntent is the intent a classifier's answer names: its first number,
// 0 for none.
func readIntent(answer string, intents []string) (string, error) {
	n, err := strconv.Atoi(firstNumber.FindString(answer))
	if err != nil || n < 0 || n > len(intents) {
		a := []rune(strings.TrimSpace(answer))
		if len(a) > 80 {
			a = append(a[:80], '…')
		}
		return "", answerError{fmt.Errorf("it answered %q, not a number from 0 to %d", string(a), len(intents))}
	}
	if n == 0 {
		return "", nil
	}
	return intents[n-1], nil
}

// classifyEffort is the least reasoning model takes: none when it can go
// without, else its lowest level; "" when its levels aren't known, which
// leaves it to the vendor.
func classifyEffort(model string) string {
	p, m, ok := provider.Resolve(model)
	if !ok {
		return ""
	}
	levels := p.Efforts(m)
	if len(levels) == 0 {
		return ""
	}
	return fitEffort("none", levels)
}

// askClassifier asks a decision provider's model through its own API, and
// any other model through the gateway itself, as a client would: which of
// intents the message is and, when effort, how hard the turn is, each in a
// call of its own, at once.
// askWaited is ask with each question a wait ctx's watch knows of
// (noteWait): a routing group's decision model is asked before the
// request's route is begun.
func askWaited(ctx context.Context, ask classifier) classifier {
	return func(model string, intents []string, prev before, effort bool, text string) (verdict, error) {
		defer noteWait(ctx, "decision model")()
		return ask(model, intents, prev, effort, text)
	}
}

func (s *Server) askClassifier(model string, intents []string, prev before, effort bool, text string) (verdict, error) {
	if p, m, ok := provider.Resolve(model); ok && p.DecidesModel(m) {
		return s.askDecider(p, m, intents, prev, effort, text)
	}
	// a group of decision models: each in turn (decide_group.go)
	if v, ok, err := s.askDecisionGroup(model, intents, prev, effort, text); ok {
		return v, err
	}
	var v verdict
	var ierr, eerr error
	var wg sync.WaitGroup
	if len(intents) > 0 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			var a string
			if a, ierr = s.askChat(model, classifyBody(model, classifyEffort(model), intents, prev, text)); ierr == nil {
				v.Intent, ierr = readIntent(a, intents)
			}
		}()
	}
	if effort {
		wg.Add(1)
		go func() {
			defer wg.Done()
			var a string
			if a, eerr = s.askChat(model, effortBody(model, classifyEffort(model), prev, text)); eerr == nil {
				v.Effort, v.Score, eerr = readEffort(a)
			}
		}()
	}
	wg.Wait()
	if ierr != nil {
		return verdict{}, ierr
	}
	return v, eerr
}

const effortPrompt = "You judge how much reasoning a coding assistant needs to handle a user's message well. " +
	"Given numbered levels and the user's message, answer with the number of the level it needs. Answer with the number only."

// effortBody is the Chat request asking model how hard text is to think
// about, one of jevLevels by number. When the turn before was given a
// level (prev), a message that only carries on from it needs that one.
func effortBody(model, effort string, prev before, text string) []byte {
	var b strings.Builder
	b.WriteString("Levels:\n")
	was := 0
	for i, l := range jevLevels {
		fmt.Fprintf(&b, "%d. %s: %s\n", i+1, l.effort, l.what)
		if l.effort == prev.Effort {
			was = i + 1
		}
	}
	writePrior(&b, prev)
	if was > 0 {
		fmt.Fprintf(&b, "\nThe user's message before this one, in the same conversation, needed level %d. "+
			"A message that only carries on from it — go on, yes, do it — needs level %d too.\n", was, was)
	}
	b.WriteString("\nThe user's message:\n<message>\n")
	b.WriteString(text)
	b.WriteString("\n</message>\n\nThe number of the level it needs:")
	req := map[string]any{
		"model": model,
		"messages": []map[string]string{
			{"role": "system", "content": effortPrompt},
			{"role": "user", "content": b.String()},
		},
		"stream":      false,
		"temperature": 0,
		"max_tokens":  2048,
	}
	if effort != "" {
		req["reasoning_effort"] = effort
	}
	body, _ := json.Marshal(req)
	return body
}

// readEffort is the level a classifier's answer names by number, and its
// score as Jev gives one (0 for low to 3 for xhigh).
func readEffort(answer string) (string, float64, error) {
	n, err := strconv.Atoi(firstNumber.FindString(answer))
	if err != nil || n < 1 || n > len(jevLevels) {
		a := []rune(strings.TrimSpace(answer))
		if len(a) > 80 {
			a = append(a[:80], '…')
		}
		return "", 0, answerError{fmt.Errorf("it answered %q, not a level from 1 to %d", string(a), len(jevLevels))}
	}
	return jevLevels[n-1].effort, float64(n - 1), nil
}

// askChat sends a chat model body through the gateway and returns its
// answer's text.
func (s *Server) askChat(model string, body []byte) (string, error) {
	wait := classifyTimeout()
	ctx, cancel := context.WithTimeout(context.Background(), wait)
	defer cancel()
	r, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://magpie/v1/chat/completions", nil)
	if err != nil {
		return "", err
	}
	r.Header.Set("Content-Type", "application/json")
	r.Header.Set("User-Agent", RouterAgent)
	w := httptest.NewRecorder()
	s.serve(w, r, provider.Chat, body)
	if ctx.Err() != nil {
		return "", fmt.Errorf("%s gave no answer in %s", model, wait)
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
		return "", fmt.Errorf("%s: %d, not an answer", model, w.Code)
	}
	if w.Code >= 300 || out.Error != nil {
		msg := http.StatusText(w.Code)
		if out.Error != nil && out.Error.Message != "" {
			msg = out.Error.Message
		}
		return "", fmt.Errorf("%s: %s", model, msg)
	}
	if len(out.Choices) == 0 {
		return "", answerError{fmt.Errorf("%s gave no answer", model)}
	}
	return out.Choices[0].Message.Content, nil
}
