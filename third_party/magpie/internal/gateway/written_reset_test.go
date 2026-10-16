package gateway

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

// WorkBuddy's two refusals, as Natsumeretsu quoted them (#1669).
const (
	wbZh = "WorkBuddy：您的使用量已超出频率限制，将在 2026-10-11 20:16:02 UTC+8重置，您也可以切换其他模型继续使用。"
	wbEn = "WorkBuddy AI: usage exceeds frequency limit, but don't worry, your usage will reset at 2026-10-11 12:37:53 UTC+8, alternatively, you can switch to the other models to continue using it."
)

// escaped is msg as a vendor's JSON error whose encoder escapes what
// isn't ASCII as \uXXXX, as Python's json.dumps does by default.
func escaped(msg, code string) string {
	return `{"error":{"message":` + strconv.QuoteToASCII(msg) + `,"code":"` + code + `"}}`
}

// wbAt is WorkBuddy's refusal in lang, saying it resets at t.
func wbAt(lang string, t time.Time) string {
	at := t.In(time.FixedZone("", 8*3600)).Format("2006-01-02 15:04:05")
	if lang == "zh" {
		return strings.Replace(wbZh, "2026-10-11 20:16:02", at, 1)
	}
	return strings.Replace(wbEn, "2026-10-11 12:37:53", at, 1)
}

// A reset time written in words is read in the offset it is written with,
// in either language; one cut short, garbled, with no offset, past, too far
// off, or a date another vendor mentions in passing is not read at all.
func TestResetsWritten(t *testing.T) {
	cst := time.FixedZone("", 8*3600)
	at := func(s string) time.Time {
		v, err := time.ParseInLocation("2006-01-02 15:04:05", s, cst)
		if err != nil {
			t.Fatal(err)
		}
		return v
	}
	for _, c := range []struct {
		name, body string
		now        time.Time
		want       time.Duration
	}{
		{"zh", wbZh, at("2026-10-11 17:00:00"), 3*time.Hour + 16*time.Minute + 2*time.Second},
		{"en", wbEn, at("2026-10-11 10:00:00"), 2*time.Hour + 37*time.Minute + 53*time.Second},
		// the same instant, written in another offset
		{"en UTC", strings.Replace(wbEn, "12:37:53 UTC+8", "04:37:53 UTC+0", 1), at("2026-10-11 10:00:00"), 2*time.Hour + 37*time.Minute + 53*time.Second},
		{"en +08:00", strings.Replace(wbEn, "UTC+8", "UTC+08:00", 1), at("2026-10-11 10:00:00"), 2*time.Hour + 37*time.Minute + 53*time.Second},
		{"en -5", strings.Replace(wbEn, "2026-10-11 12:37:53 UTC+8", "2026-10-10 23:37:53 UTC-5", 1), at("2026-10-11 10:00:00"), 2*time.Hour + 37*time.Minute + 53*time.Second},
		{"zh JSON", `{"error":{"message":"` + wbZh + `","code":"6004"}}`, at("2026-10-11 17:00:00"), 3*time.Hour + 16*time.Minute + 2*time.Second},
		{"zh escaped", escaped(wbZh, "6004"), at("2026-10-11 17:00:00"), 3*time.Hour + 16*time.Minute + 2*time.Second},
		{"zh escaped, cut short", escaped(wbZh[:strings.Index(wbZh, "+8重置")], "6004"), at("2026-10-11 17:00:00"), 0},
		// cut short: never read as UTC, or as a time without its seconds
		{"zh cut after UTC", wbZh[:strings.Index(wbZh, "+8重置")], at("2026-10-11 17:00:00"), 0},
		{"zh cut after +", wbZh[:strings.Index(wbZh, "8重置")], at("2026-10-11 17:00:00"), 0},
		{"zh cut after +8", wbZh[:strings.Index(wbZh, "重置")], at("2026-10-11 17:00:00"), 0},
		{"en cut after UTC", wbEn[:strings.Index(wbEn, "+8,")], at("2026-10-11 10:00:00"), 0},
		{"en cut after +8", wbEn[:strings.Index(wbEn, ", alternatively")], at("2026-10-11 10:00:00"), 0},
		{"en cut in the time", wbEn[:strings.Index(wbEn, ":53")], at("2026-10-11 10:00:00"), 0},
		{"zh cut in a rune", wbZh[:strings.Index(wbZh, "重置")+1], at("2026-10-11 17:00:00"), 0},
		// garbled
		{"month 13", strings.Replace(wbZh, "2026-10-11", "2026-13-11", 1), at("2026-10-11 17:00:00"), 0},
		{"day 45", strings.Replace(wbEn, "2026-10-11", "2026-10-45", 1), at("2026-10-11 10:00:00"), 0},
		{"hour 25", strings.Replace(wbZh, "20:16:02", "25:16:02", 1), at("2026-10-11 17:00:00"), 0},
		{"offset +99", strings.Replace(wbEn, "UTC+8", "UTC+99", 1), at("2026-10-11 10:00:00"), 0},
		{"no offset", strings.Replace(wbZh, " UTC+8", "", 1), at("2026-10-11 17:00:00"), 0},
		// past, or further off than a week
		{"past", wbZh, at("2026-10-11 20:30:00"), 0},
		{"8 days", wbEn, at("2026-10-03 12:37:52"), 0},
		{"7 days", wbEn, at("2026-10-04 12:37:53"), 7 * 24 * time.Hour},
		// another vendor's date in passing is no reset
		{"deprecated", "This model will be deprecated on 2026-10-12 00:00:00 UTC+8, please switch to the other models.", at("2026-10-11 10:00:00"), 0},
		{"retry after", "Too many requests, retry after 2026-10-11 12:37:53 UTC+8.", at("2026-10-11 10:00:00"), 0},
	} {
		if got := resetsWritten([]byte(c.body), c.now); got != c.want {
			t.Errorf("%s: %v, want %v (%q)", c.name, got, c.want, c.body)
		}
		if got := resetsAt([]byte(c.body), c.now); got != c.want {
			t.Errorf("%s: resetsAt %v, want %v", c.name, got, c.want)
		}
	}
}

// WorkBuddy's 429 that names when it is back rests the model until then,
// in Chinese as in English: both read as out of quota, by "resets", past
// the hour a Retry-After is held to, while the account's other models are
// still asked. Another vendor's 429s rest as before (#1669).
func TestWrittenResetRestsUntilThen(t *testing.T) {
	old := allowances
	defer func() { allowances = old }()
	allowances = func(string) map[string]provider.Allowance { return nil }
	s := &Server{}
	acct := func(id, user, model string) candidate {
		return candidate{p: provider.Provider{ID: id, Account: &provider.Account{Agent: id, User: user}}, model: model, rest: id + "#" + user}
	}
	near := func(d, want time.Duration) bool { return d > want-time.Minute && d <= want }
	back := 5*time.Hour + 30*time.Minute

	for _, lang := range []string{"zh", "en", "zh-escaped"} {
		user := lang + "@x.com"
		c := acct("workbuddy", user, "glm-5")
		body := []byte(`{"error":{"message":"` + wbAt(lang, time.Now().Add(back)) + `","code":"6004"}}`)
		if lang == "zh-escaped" {
			body = []byte(escaped(wbAt("zh", time.Now().Add(back)), "6004"))
		}
		if f := failure(429, body); f != failQuota {
			t.Fatalf("%s: %s", lang, f)
		}
		r := s.restAfter(c, 429, http.Header{}, body)
		if r.Why != failQuota || r.By != "resets" || !near(time.Until(r.Until), back) {
			t.Fatalf("%s: rests %v by %s (%s), want %v", lang, time.Until(r.Until), r.By, r.Why, back)
		}
		if r.Key != c.restID() {
			t.Fatalf("%s: rests %s, not the model %s", lang, r.Key, c.restID())
		}
		if !resting(c) || resting(acct("workbuddy", user, "deepseek-v3.2")) {
			t.Fatalf("%s: the model rests %v, another of the account %v", lang, resting(c), resting(acct("workbuddy", user, "deepseek-v3.2")))
		}

		// as the agent's error, with the time keepRetry noted beside it
		user = lang + "-noted@x.com"
		c = acct("workbuddy", user, "glm-5")
		h := http.Header{}
		keepRetry(h, http.Header{}, body)
		r = s.restAfter(c, 429, h, []byte("WorkBuddy: "+provider.APIError(body, "")))
		if r.Why != failQuota || r.By != "resets" || !near(time.Until(r.Until), back) || r.Key != c.restID() {
			t.Fatalf("%s noted: rests %v by %s (%s) on %s", lang, time.Until(r.Until), r.By, r.Why, r.Key)
		}

		// cut short before its offset: the fallback, never a wrong time
		c = acct("workbuddy", lang+"-cut@x.com", "glm-5")
		cut := wbAt(strings.TrimSuffix(lang, "-escaped"), time.Now().Add(back))
		cut = cut[:strings.Index(cut, "UTC")+3]
		if r := s.restAfter(c, 429, http.Header{}, []byte(cut)); time.Until(r.Until) > quotaRest {
			t.Fatalf("%s cut short: rests %v by %s", lang, time.Until(r.Until), r.By)
		}
	}

	// another vendor's 429s rest as they did
	for _, c := range []struct {
		body string
		why  string
		d    time.Duration
	}{
		{`{"error":{"message":"Rate limit reached for requests","type":"rate_limit_error"}}`, failRate, fallbackCooldown},
		{`{"error":{"message":"Too many requests, retry after 2026-10-11 12:37:53 UTC+8."}}`, failRate, fallbackCooldown},
		{`{"error":{"message":"You exceeded your current quota. glm-4 is deprecated on ` + time.Now().Add(72*time.Hour).Format("2006-01-02 15:04:05") + ` UTC+8."}}`, failQuota, quotaRest},
	} {
		k := acct("other", "o@x.com", "m")
		clearRest(k.restKey())
		r := s.restAfter(k, 429, http.Header{}, []byte(c.body))
		if r.Why != c.why || !near(time.Until(r.Until), c.d) || r.Key != k.restKey() {
			t.Fatalf("%s: rests %v by %s (%s) on %s, want %v (%s)", c.body, time.Until(r.Until), r.By, r.Why, r.Key, c.d, c.why)
		}
		clearRest(k.restKey())
	}
}

// A rate limit whose refusal names when it's back rests until then, as far
// as out of quota is believed; a Retry-After alone is still held to the
// hour (#1669: the same words rested 8 days read as out of quota, an hour
// read as a rate limit).
func TestRateLimitBelievesItsReset(t *testing.T) {
	old := allowances
	defer func() { allowances = old }()
	allowances = func(string) map[string]provider.Allowance { return nil }
	s := &Server{}
	c := candidate{p: provider.Provider{ID: "codex", Account: &provider.Account{Agent: "codex", User: "r@x.com"}}, model: "gpt-5.6-sol", rest: "codex#r"}
	near := func(d, want time.Duration) bool { return d > want-time.Minute && d <= want }

	body := []byte(`{"error":{"resets_in_seconds":` + strconv.Itoa(3*3600) + `,"type":"rate_limit_exceeded","message":"Rate limit exceeded, slow down"}}`)
	if f := failure(429, body); f != failRate {
		t.Fatalf("read as %s", f)
	}
	r := s.restAfter(c, 429, http.Header{}, body)
	if r.Why != failRate || r.By != "resets" || !near(time.Until(r.Until), 3*time.Hour) {
		t.Fatalf("rate limited, back in 3h: %v by %s", time.Until(r.Until), r.By)
	}
	clearRest(c.restKey())
	h := http.Header{}
	h.Set(resetsHeader, strconv.FormatInt(time.Now().Add(2*time.Hour).Unix(), 10))
	if r := s.restAfter(c, 429, h, []byte("Rate limit exceeded")); r.By != "resets" || !near(time.Until(r.Until), 2*time.Hour) {
		t.Fatalf("rate limited, noted back in 2h: %v by %s", time.Until(r.Until), r.By)
	}
	clearRest(c.restKey())
	h = http.Header{}
	h.Set("Retry-After", strconv.Itoa(3*3600))
	if r := s.restAfter(c, 429, h, []byte("Rate limit exceeded")); r.By != "retry-after" || !near(time.Until(r.Until), longestWait) {
		t.Fatalf("Retry-After 3h: %v by %s", time.Until(r.Until), r.By)
	}
	clearRest(c.restKey())
}

// Through a plugin — as a moved WorkBuddy's requests are — the refusal is
// read by the same rest: the plugin passes WorkBuddy's 429 on, and the
// gateway rests the model until the time it names.
func TestPluginWrittenResetRests(t *testing.T) {
	bun := testenv.Bun(t)
	fresh(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	old := allowances
	allowances = func(string) map[string]provider.Allowance { return nil }
	t.Cleanup(func() { allowances = old })

	back := 4 * time.Hour
	refusal := wbAt("zh", time.Now().Add(back))
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(429)
		fmt.Fprintf(w, `{"error":{"message":%q,"code":"6004"}}`, refusal)
	}))
	defer up.Close()
	t.Setenv("FAKE_BASE", up.URL+"/v1")

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	// two, as the last one left is told the error without resting
	for _, k := range []string{"k1", "k2"} {
		if _, err := plugin.APIKey(ctx, "fakeco", 0, nil, k, plugin.NewAccount); err != nil {
			t.Fatal(err)
		}
	}
	restingUntil.Lock()
	restingUntil.m, restingUntil.note = map[string]time.Time{}, map[string]Rest{}
	restingUntil.Unlock()

	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions",
		strings.NewReader(`{"model":"fakeco/fake-1","messages":[{"role":"user","content":"hello"}]}`)))
	if rec.Code != 429 {
		t.Fatalf("answered %d %s", rec.Code, rec.Body.String())
	}
	restingUntil.Lock()
	defer restingUntil.Unlock()
	var got []string
	for id, r := range restingUntil.note {
		d := time.Until(restingUntil.m[id])
		got = append(got, fmt.Sprintf("%s: %v by %s (%s)", id, d, r.By, r.Why))
		if r.Why == failQuota && r.By == "resets" && d > back-time.Minute && d <= back && strings.HasSuffix(id, "/fake-1") {
			return
		}
	}
	t.Fatalf("no rest of fake-1 until the stated time: %v", got)
}

// The word lists read a JSON error that escapes its Chinese as \uXXXX the
// same as one that doesn't (#1669): Zhipu's 1113 is out of credit, not a
// rate limit, and a sub2api key's "7天限额已用完" sent as
// rate_limit_exceeded is out of quota.
func TestFailureReadsEscapedWords(t *testing.T) {
	for _, c := range []struct {
		body, want string
	}{
		{escaped("余额不足或无可用资源包，请充值。", "1113"), failCredit},
		{`{"error":{"type":"rate_limit_exceeded","message":` + strconv.QuoteToASCII("api key 7天限额已用完") + `}}`, failQuota},
		{escaped(wbZh, "6004"), failQuota},
		{escaped("请求太频繁，请稍后再试", "1302"), failRate},
	} {
		if !strings.Contains(c.body, `\u`) {
			t.Fatalf("not escaped: %s", c.body)
		}
		if got := failure(429, []byte(c.body)); got != c.want {
			t.Errorf("%s: %s, want %s", c.body, got, c.want)
		}
	}
}
