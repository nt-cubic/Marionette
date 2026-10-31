package gui

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/maskrec"
	"github.com/yetone/magpie/internal/redact"
)

// fakeMaskAlerts is maskAlerts on a clock the test moves, its flush run
// when the test says, what it said kept.
type fakeMaskAlerts struct {
	*maskAlerts
	clock  time.Time
	waits  []time.Duration
	flushF func()
	said   []maskNote
	on     bool
}

func newFakeMaskAlerts(t *testing.T) *fakeMaskAlerts {
	t.Helper()
	f := &fakeMaskAlerts{clock: time.Date(2026, 10, 29, 10, 0, 0, 0, time.Local), on: true}
	f.maskAlerts = newMaskAlerts(func() bool { return f.on }, func(n maskNote) { f.said = append(f.said, n) })
	f.now = func() time.Time { return f.clock }
	f.after = func(d time.Duration, fn func()) { f.waits = append(f.waits, d); f.flushF = fn }
	return f
}

// fire runs the flush set, as its wait is over.
func (f *fakeMaskAlerts) fire(t *testing.T) {
	t.Helper()
	if f.flushF == nil {
		t.Fatal("no notification was set to be said")
	}
	fn := f.flushF
	f.flushF = nil
	f.clock = f.clock.Add(f.waits[len(f.waits)-1])
	fn()
}

// mask masks text as the gateway does, and records it as noteMasked does,
// for agent: the record OnMasked is told.
func mask(t *testing.T, text, agent string) maskrec.Record {
	t.Helper()
	var vals []maskrec.Value
	o := redact.Options{Secrets: true, Personal: true, Seen: func(h redact.Hit) {
		vals = append(vals, maskrec.Value{Placeholder: h.Placeholder, Kind: h.Kind, By: h.By, Head: h.Head, Len: h.Len, Action: h.Action})
	}}
	out, n := redact.Mask(text, o)
	if n == 0 {
		t.Fatalf("nothing masked in %q", text)
	}
	if out == text {
		t.Fatalf("text went as written: %q", out)
	}
	return maskrec.Add(maskrec.Record{What: "chat", Agent: agent, N: n, Values: vals})
}

// A conversation sends its history again every turn, so a value masked in
// its first turn is masked in each turn after. Only the first is said;
// a value new in a later turn is said on its own.
func TestMaskAlertFirstTimeOnly(t *testing.T) {
	f := newFakeMaskAlerts(t)
	key := "sk-ant-api03-" + strings.Repeat("Q7vR2mXk", 5)
	turn1 := "deploy with ANTHROPIC_API_KEY=" + key
	f.add(mask(t, turn1, "claude"))
	f.fire(t)
	if len(f.said) != 1 || f.said[0].Kinds["API_KEY"] != 1 || f.said[0].total() != 1 {
		t.Fatalf("turn 1 said %+v, want one API key", f.said)
	}
	// turns 2 and 3: the history again, the key with it, nothing new
	f.clock = f.clock.Add(5 * time.Minute)
	f.add(mask(t, turn1+"\nok, done. what next?", "claude"))
	f.add(mask(t, turn1+"\nok, done. what next?\nrun the tests", "claude"))
	f.fire(t)
	if len(f.said) != 1 {
		t.Fatalf("the history masked again was said again: %+v", f.said[1:])
	}
	// turn 4 brings an email the conversation never had
	f.clock = f.clock.Add(5 * time.Minute)
	f.add(mask(t, turn1+"\nmail the report to wang.lei@example-corp.cn", "claude"))
	f.fire(t)
	if len(f.said) != 2 || f.said[1].Kinds["EMAIL"] != 1 || f.said[1].total() != 1 {
		t.Fatalf("turn 4 said %+v, want the email alone", f.said[1:])
	}
}

// What is said is the kind, how many and the agent: never the value, no
// piece of it, nor the hint the 脱敏 page keeps of it (a key's prefix).
func TestMaskAlertNeverSaysTheValue(t *testing.T) {
	f := newFakeMaskAlerts(t)
	key := "sk-proj-" + strings.Repeat("Zx9Lm3Pq", 5)
	mail, phone := "lfqzw.xu@xqjvb-yd.cn", "13987654321"
	r := mask(t, "OPENAI_API_KEY="+key+"\ncontact "+mail+" or "+phone, "codex")
	for _, v := range r.Values {
		if v.Head == "" {
			t.Fatalf("the record has no hint to leak, the test proves nothing: %+v", r.Values)
		}
	}
	f.add(r)
	f.fire(t)
	if len(f.said) != 1 || f.said[0].total() != 3 {
		t.Fatalf("said %+v, want the key, the email and the phone", f.said)
	}
	for _, lang := range []string{"en", "zh", "zh-TW", "ja", "de"} {
		title, body := maskAlertText(lang, f.said[0], agentName)
		text := title + "\n" + body
		for _, v := range []string{key, mail, phone} {
			for i := 0; i+4 <= len(v); i++ {
				if strings.Contains(text, v[i:i+4]) {
					t.Fatalf("%s says %q of %q: %s", lang, v[i:i+4], v, text)
				}
			}
		}
		for _, v := range r.Values {
			if len(v.Head) > 1 && strings.Contains(text, v.Head) {
				t.Fatalf("%s says the hint %q: %s", lang, v.Head, text)
			}
		}
		if !strings.Contains(body, "Codex") || !strings.Contains(title, "3") {
			t.Fatalf("%s doesn't say how many or who: %s", lang, text)
		}
	}
}

// An agent's tool calls come one after another, each a request with a new
// value: they are said once, together. What comes while the last one is
// fresh waits a minute from it, and is said then, together too.
func TestMaskAlertCoalesces(t *testing.T) {
	f := newFakeMaskAlerts(t)
	texts := []string{
		"GITHUB_TOKEN=ghp_" + strings.Repeat("aB3dE5fG7h", 4),
		"db: postgres://app:Hunter2Secret9@db.internal:5432/app",
		"write to zhao.min@mail-host.cn",
		"call 13711112222 tomorrow",
		"AWS key AKIAQWERTYUIOPASDFGH",
	}
	for i, s := range texts {
		f.add(mask(t, s, []string{"claude", "codex", "opencode"}[i%3]))
	}
	if len(f.waits) != 1 || f.waits[0] != maskGather {
		t.Fatalf("waits %v, want one of %v for the burst", f.waits, maskGather)
	}
	f.fire(t)
	if len(f.said) != 1 || f.said[0].total() != len(texts) {
		t.Fatalf("said %d notifications %+v, want one of %d values", len(f.said), f.said, len(texts))
	}
	if got := f.said[0].Agents; len(got) != 3 {
		t.Fatalf("agents %v, want the three that sent them", got)
	}
	if _, body := maskAlertText("en", f.said[0], agentName); !strings.Contains(body, "+1") {
		t.Fatalf("a third agent isn't counted: %s", body)
	}
	// ten seconds on, two more: said a minute after the last, as one
	f.clock = f.clock.Add(10 * time.Second)
	f.add(mask(t, "the admin is sun.li@corp-mail.cn", "claude"))
	f.add(mask(t, "her phone is 15822223333", "claude"))
	if w := f.waits[len(f.waits)-1]; w != maskGap-10*time.Second {
		t.Fatalf("waits %v after one said 10s ago, want %v", w, maskGap-10*time.Second)
	}
	if len(f.waits) != 2 {
		t.Fatalf("waits %v, want one more for both", f.waits)
	}
	f.fire(t)
	if len(f.said) != 2 || f.said[1].total() != 2 {
		t.Fatalf("said %+v, want the two together", f.said[1:])
	}
	// turned off, nothing is kept or said
	f.on = false
	f.clock = f.clock.Add(time.Hour)
	f.add(mask(t, "and wu.qiang@corp-mail.cn", "claude"))
	if f.flushF != nil || len(f.pending) != 0 {
		t.Fatal("a record was kept with the alert off")
	}
}
