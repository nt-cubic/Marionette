// Package redact keeps secrets on the machine. What the gateway sends a
// vendor has its secrets (API keys, private keys, tokens, passwords in
// connection strings and assignments), and when asked personal data and the
// user's own words, swapped for placeholders like {{API_KEY_k3v9x2mq}}; what
// the vendor says back has them swapped back, streams included, so the agent
// and the user see the real thing while the vendor never does.
//
// A placeholder is the same for the same value every time (a keyed hash of
// it), so a conversation's history reads the same turn after turn — what a
// vendor cached of it stays good, and a thinking block's signature still
// matches the text it was made for — and one placeholder is never two
// values. The values are held in memory only, by placeholder, as requests
// bring them; a restart loses nothing, since the next request brings them
// again. A value once masked is masked wherever a request has it, whatever
// words are around it (known.go): the vendor writes about a placeholder in
// words no rule knows, and the agent sends that text back with the value.
package redact

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base32"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strings"
	"sync"
)

// Options says what to mask.
type Options struct {
	Secrets  bool     // API keys, private keys, tokens, passwords
	Personal bool     // emails, phone numbers, ID and bank card numbers
	Words    []string // the user's own: names, codenames, hosts
	Rules    []Rule   // the user's own rules, masked with the secrets
	// Kinds turns each of the Categories of personal data on or off by its
	// id, where the user chose; one not in it is as its Category.On says.
	// None of them is masked while Personal is off.
	Kinds map[string]bool
}

// covers says o masks what a rule of category cat finds: "" are the
// secrets, catPersonal the personal data always masked with it, any other
// one of the Categories.
func (o Options) covers(cat string) bool {
	switch cat {
	case "":
		return o.Secrets
	case catPersonal:
		return o.Personal
	}
	if !o.Personal {
		return false
	}
	if on, ok := o.Kinds[cat]; ok {
		return on
	}
	return categoryOn[cat]
}

// rule finds one kind of value. The match is group 1 when the pattern has
// one, else all of it; bound are the characters that may not touch it on
// either side (a key inside a longer word is no key); ok checks it further.
type rule struct {
	kind    string
	re      *regexp.Regexp
	markers []string // one of these is in any text it can match
	// fold are markers too, looked for in the text in lower case, for a
	// pattern that matches its words in any case (Passport, PASSPORT)
	fold  []string
	bound string
	ok    func(string) bool
	// at checks the value at s[a:b] by what is around it: a dotted quad
	// right after "version" is no address
	at func(s string, a, b int) bool
	// fix, where set, checks the value in place of ok and may shorten it:
	// an IBAN printed in groups of four, followed by a word in capitals
	fix func(s string, a, b int) (int, int, bool)
	// back and ahead, where ahead is set, are how far before and after
	// one of fold a match can be: the pattern is only run there, as one
	// that takes its words in any case is slow over all of a long text
	back, ahead int
	// cat is what turns the rule on (Options.covers): "" the secrets
	cat string
	// anyGroup takes the first group that matched, for a pattern with a
	// group in each of its alternatives
	anyGroup bool
	// groups, where set, are the kinds of a match's groups, each masked as
	// a value of its own (the user and the host of user@host), and groupOK
	// checks each
	groups  []string
	groupOK []func(string) bool
}

const (
	alnum   = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
	tokenCh = alnum + "_-"
	digits  = "0123456789"
)

var rules = append([]rule{
	{kind: "PRIVATE_KEY", re: regexp.MustCompile(`-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----[\s\S]+?-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----`), markers: []string{"PRIVATE KEY"}},
	{kind: "API_KEY", re: regexp.MustCompile(`sk-(?:ant-|proj-|or-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}`), markers: []string{"sk-"}, bound: tokenCh},
	// a gateway's or a relay's own: oc_sk_…, or_sk_… (#195)
	{kind: "API_KEY", re: regexp.MustCompile(`[a-z]{2,4}_sk_[A-Za-z0-9_-]{20,}`), markers: []string{"_sk_"}, bound: tokenCh, ok: secretValue},
	{kind: "API_KEY", re: regexp.MustCompile(`(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|glpat-[A-Za-z0-9_-]{20,})`), markers: []string{"gh", "github_pat_", "glpat-"}, bound: tokenCh},
	{kind: "API_KEY", re: regexp.MustCompile(`AIza[0-9A-Za-z_-]{35}`), markers: []string{"AIza"}, bound: tokenCh},
	{kind: "API_KEY", re: regexp.MustCompile(`xox[abposr]-[0-9A-Za-z-]{10,}`), markers: []string{"xox"}, bound: tokenCh},
	{kind: "API_KEY", re: regexp.MustCompile(`(?:sk|rk)_live_[0-9A-Za-z]{20,}`), markers: []string{"_live_"}, bound: tokenCh},
	{kind: "API_KEY", re: regexp.MustCompile(`(?:AKIA|ASIA)[0-9A-Z]{16}`), markers: []string{"AKIA", "ASIA"}, bound: alnum},
	{kind: "API_KEY", re: regexp.MustCompile(`(?:hf_[A-Za-z0-9]{30,}|gsk_[A-Za-z0-9]{40,}|xai-[A-Za-z0-9]{40,}|npm_[A-Za-z0-9]{36}|pypi-[A-Za-z0-9_-]{50,}|dop_v1_[a-f0-9]{64}|SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43})`),
		markers: []string{"hf_", "gsk_", "xai-", "npm_", "pypi-", "dop_v1_", "SG."}, bound: tokenCh},
	{kind: "TOKEN", re: regexp.MustCompile(`eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}`), markers: []string{"eyJ"}, bound: tokenCh},
	// the password of user:password@host
	{kind: "PASSWORD", re: regexp.MustCompile(`[A-Za-z][A-Za-z0-9+.-]*://[^\s:@/?#"'<>]+:([^\s@/?#"'<>]{3,})@`), markers: []string{"://"}, ok: notAVariable},
	// password = …, API_KEY: "…", as .env files and configs have them
	{kind: "SECRET", re: regexp.MustCompile(`(?i)[A-Za-z0-9_.-]*(?:` + secretNames.pattern() + `)[A-Za-z0-9_.-]*` + quote + `[ \t]*[:=][ \t]*` + quote + `(` + secretCh + `{8,})`),
		markers: secretNames.markers(), ok: secretValue},
	// a field called key and nothing more, as providers.json has one: "key":
	// "oc_sk_…" (#195). Only a long value of letters and digits both, since
	// code calls anything a key: a map's "key": "value", sort_key, keyboard.
	{kind: "SECRET", re: regexp.MustCompile(`(?i)(?:^|[^A-Za-z0-9_.-])` + quote + `key` + quote + `[ \t]*[:=][ \t]*` + quote + `(` + secretCh + `{16,})`),
		markers: fieldNames{{"key", "key"}}.markers(), ok: secretValue},

	{kind: "EMAIL", re: regexp.MustCompile(`[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}`), markers: []string{"@"}, bound: alnum + "._%+-", ok: realEmail, cat: catPersonal},
	{kind: "ID_CARD", re: regexp.MustCompile(`[1-9]\d{5}(?:18|19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]`), bound: alnum, ok: chineseID, cat: catPersonal},
	{kind: "PHONE", re: regexp.MustCompile(`(?:\+?86[- ]?)?1[3-9]\d{9}`), bound: digits, cat: catPersonal},
	{kind: "BANK_CARD", re: regexp.MustCompile(`[3-6]\d{3}(?:[ -]?\d{4}){2,3}(?:[ -]?\d{1,3})?`), bound: digits, ok: luhn, cat: catPersonal},
}, moreRules...)

// fieldNames are what a field that holds a secret is called: a pattern for
// each name, and text that any match of it has in it, so a rule's markers
// and the names its pattern takes come from one list and can't drift apart.
type fieldNames []struct{ re, marker string }

// secretNames are what a field holding a secret has in its name:
// DB_PASSWORD, apiKey, x-api-key, client_secret.
var secretNames = fieldNames{
	{"password", "pass"}, {"passwd", "pass"},
	{"secret", "secret"},
	{"token", "token"},
	{`api[_-]?key`, "key"}, {`access[_-]?key`, "key"}, {`private[_-]?key`, "key"},
	{"credential", "credential"},
}

func (f fieldNames) pattern() string {
	res := make([]string, len(f))
	for i, n := range f {
		res[i] = n.re
	}
	return strings.Join(res, "|")
}

// markers are each name's marker as a name is written in lower case, in
// upper case and capitalised: password, PASSWORD, Password, apiKey.
func (f fieldNames) markers() []string {
	var out []string
	for _, n := range f {
		for _, m := range []string{n.marker, strings.ToUpper(n.marker), strings.ToUpper(n.marker[:1]) + n.marker[1:]} {
			if !slices.Contains(out, m) {
				out = append(out, m)
			}
		}
	}
	return out
}

const (
	// quote is a field's or a value's quote, if it has one: " or ', or \"
	// in JSON written inside a string
	quote = `(?:\\?["'])?`
	// secretCh are the characters of a secret's value
	secretCh = `[A-Za-z0-9_\-./+=~!@#%^&*]`
)

// placeholderRe is a placeholder as Mask writes it.
var placeholderRe = regexp.MustCompile(`\{\{[A-Z][A-Z0-9_]*_[a-z2-7]{8}\}\}`)

// notAVariable turns away what stands in for a value: ${PASS}, <password>, ****.
func notAVariable(v string) bool {
	if strings.ContainsAny(v[:1], "$%{<[*") {
		return false
	}
	return strings.Trim(v, "*xX.") != ""
}

// secretValue is a value that looks like a secret, not code: letters and
// digits both, and not a call, a reference or a placeholder of its own.
func secretValue(v string) bool {
	if !notAVariable(v) || strings.HasPrefix(v, "process.env") || strings.HasPrefix(v, "os.") {
		return false
	}
	return strings.ContainsAny(v, digits) && strings.IndexFunc(v, func(r rune) bool { return r >= 'A' && r <= 'Z' || r >= 'a' && r <= 'z' }) >= 0
}

func realEmail(v string) bool {
	host := strings.ToLower(v[strings.LastIndexByte(v, '@')+1:])
	for _, fake := range []string{"example.com", "example.org", "example.net", "localhost", ".test", ".invalid", ".example"} {
		if host == strings.TrimPrefix(fake, ".") || strings.HasSuffix(host, fake) {
			return false
		}
	}
	return !strings.HasPrefix(host, "noreply") && !strings.Contains(v, "noreply@")
}

func chineseID(v string) bool {
	w := []int{7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2}
	sum := 0
	for i := 0; i < 17; i++ {
		sum += int(v[i]-'0') * w[i]
	}
	return strings.ToUpper(v[17:]) == string("10X98765432"[sum%11])
}

func luhn(v string) bool {
	var ds []int
	for _, r := range v {
		if r >= '0' && r <= '9' {
			ds = append(ds, int(r-'0'))
		}
	}
	if len(ds) < 15 || len(ds) > 19 {
		return false
	}
	sum := 0
	for i := range ds {
		d := ds[len(ds)-1-i]
		if i%2 == 1 {
			if d *= 2; d > 9 {
				d -= 9
			}
		}
		sum += d
	}
	return sum%10 == 0
}

// ---- placeholders ---------------------------------------------------------------

var (
	mu      sync.RWMutex
	values  = map[string]string{} // placeholder → value
	kinds   = map[string]bool{}   // the kinds of values, for wordTail
	keyOf   []byte
	keyPath string // guarded by mu, as keyOf is
)

// maxValues bounds what is held; past it the oldest half is let go, and a
// request that still has them brings them back.
const maxValues = 200_000

func key() []byte {
	mu.RLock()
	k := keyOf
	mu.RUnlock()
	if k != nil {
		return k
	}
	mu.Lock()
	defer mu.Unlock()
	if keyOf != nil {
		return keyOf
	}
	keyOf = loadKey()
	return keyOf
}

// SetKeyPath chooses where the placeholder key is kept across restarts;
// "" keeps it in memory only. The gateway's owner sets it before requests
// arrive. Once the key is loaded, both it and its path stay fixed.
func SetKeyPath(path string) {
	mu.Lock()
	defer mu.Unlock()
	if keyOf == nil {
		keyPath = path
	}
}

// loadKey is called with mu held.
func loadKey() []byte {
	if keyPath != "" {
		if b, err := os.ReadFile(keyPath); err == nil && len(b) >= 32 {
			return b[:32]
		}
	}
	b := make([]byte, 32)
	_, _ = rand.Read(b)
	if keyPath != "" {
		_ = os.MkdirAll(filepath.Dir(keyPath), 0o700)
		_ = os.WriteFile(keyPath, b, 0o600)
	}
	return b
}

var b32 = base32.StdEncoding.WithPadding(base32.NoPadding)

func placeholder(kind, v string) string {
	h := hmac.New(sha256.New, key())
	h.Write([]byte(kind + "\x00" + v))
	p := "{{" + kind + "_" + strings.ToLower(b32.EncodeToString(h.Sum(nil))[:8]) + "}}"
	mu.Lock()
	if _, ok := values[p]; !ok {
		if len(values) >= maxValues {
			n := 0
			for k := range values {
				delete(values, k)
				if n++; n >= maxValues/2 {
					break
				}
			}
			reindexKnown()
		}
		values[p] = v
		kinds[kind] = true
		addKnown(p, v)
	}
	mu.Unlock()
	return p
}

// ---- masking --------------------------------------------------------------------

type span struct {
	start, end int
	kind       string
}

// Mask swaps what o covers in s for placeholders, and says how many.
func Mask(s string, o Options) (string, int) { return mask(s, o, placeholder) }

// mask swaps what o covers in s for what put makes of each value.
func mask(s string, o Options, put func(kind, v string) string) (string, int) {
	if len(s) < 3 {
		return s, 0
	}
	var found []span
	for _, w := range o.Words {
		if w = strings.TrimSpace(w); len(w) < 2 {
			continue
		}
		for i := 0; ; {
			j := strings.Index(s[i:], w)
			if j < 0 {
				break
			}
			found = append(found, span{i + j, i + j + len(w), "TERM"})
			i += j + len(w)
		}
	}
	all := rules
	if o.Secrets && len(o.Rules) > 0 {
		// the user's first: where one of theirs and one of magpie's find
		// the same value, it goes by the name they gave it
		all = append(customRules(o.Rules), rules...)
	}
	lower := ""
	// split are matches a rule took apart into values of their own
	// (rkato@box17 as a user and a host): what another rule found from
	// the same place in them, an email, goes
	var split []span
	for _, r := range all {
		if !o.covers(r.cat) {
			continue
		}
		if len(r.markers) > 0 && !containsAny(s, r.markers) {
			continue
		}
		if len(r.fold) > 0 {
			if lower == "" {
				lower = lowerASCII(s)
			}
			if !containsAny(lower, r.fold) {
				continue
			}
		}
		for _, m := range r.matches(s, lower) {
			if len(r.groups) > 0 {
				took := false
				for g, kind := range r.groups {
					if a, b := m[2*g+2], m[2*g+3]; a >= 0 && r.takes(s, a, b, r.groupOK[g]) {
						found = append(found, span{a, b, kind})
						took = true
					}
				}
				if took {
					split = append(split, span{m[2], m[1], ""})
				}
				continue
			}
			a, b := m[0], m[1]
			for g := 1; 2*g+1 < len(m); g++ {
				if m[2*g] >= 0 {
					a, b = m[2*g], m[2*g+1]
					break
				}
				if !r.anyGroup {
					break
				}
			}
			ok := r.ok
			if r.fix != nil {
				var good bool
				if a, b, good = r.fix(s, a, b); !good {
					continue
				}
				ok = nil
			}
			if r.takes(s, a, b, ok) {
				found = append(found, span{a, b, r.kind})
			}
		}
	}
	for _, sp := range split {
		found = slices.DeleteFunc(found, func(f span) bool {
			return f.kind == "EMAIL" && f.start == sp.start && f.end <= sp.end
		})
	}
	// after the rules', so where a rule finds the same value it keeps the
	// kind it has always had
	found = append(found, knownSpans(s, o)...)
	return apply(s, found, put)
}

// matches are r's matches in s, lower being lowerASCII(s), as
// FindAllStringSubmatchIndex has them: in all of s, or, for a rule with
// ahead set, only around its fold markers.
func (r rule) matches(s, lower string) [][]int {
	if r.ahead == 0 {
		return r.re.FindAllStringSubmatchIndex(s, -1)
	}
	var wins [][2]int
	for _, f := range r.fold {
		for i := 0; ; {
			j := strings.Index(lower[i:], f)
			if j < 0 {
				break
			}
			at := i + j
			i = at + len(f)
			// from the line's start, or from where a word starts no further
			// back than back: what the pattern takes for the text's start is
			// where a line or a word starts in s too
			start := strings.LastIndexByte(s[:at], '\n') + 1
			if start < at-r.back {
				start = at - r.back
				for start < at && (strings.IndexByte(alnum+"_.-", s[start-1]) >= 0 || s[start]&0xc0 == 0x80) {
					start++
				}
				if start == at && strings.IndexByte(alnum+"_", s[at-1]) >= 0 {
					continue // inside a longer word
				}
			}
			wins = append(wins, [2]int{start, min(len(s), i+r.ahead)})
		}
	}
	sort.Slice(wins, func(i, j int) bool { return wins[i][0] < wins[j][0] })
	var out [][]int
	for k := 0; k < len(wins); k++ {
		w := wins[k]
		for k+1 < len(wins) && wins[k+1][0] <= w[1] {
			k++
			w[1] = max(w[1], wins[k][1])
		}
		for _, m := range r.re.FindAllStringSubmatchIndex(s[w[0]:w[1]], -1) {
			for x := range m {
				if m[x] >= 0 {
					m[x] += w[0]
				}
			}
			out = append(out, m)
		}
	}
	return out
}

// lowerASCII is s with its ASCII letters in lower case, as long as s, so a
// place in one is the same place in the other.
func lowerASCII(s string) string {
	b := []byte(s)
	for i, c := range b {
		if 'A' <= c && c <= 'Z' {
			b[i] = c + 'a' - 'A'
		}
	}
	return string(b)
}

// takes says s[a:b] is a value r masks: nothing it may not touch beside
// it, and its checks pass.
func (r rule) takes(s string, a, b int, ok func(string) bool) bool {
	if b <= a {
		return false
	}
	if r.bound != "" && (a > 0 && strings.IndexByte(r.bound, s[a-1]) >= 0 || b < len(s) && strings.IndexByte(r.bound, s[b]) >= 0) {
		return false
	}
	if ok != nil && !ok(s[a:b]) {
		return false
	}
	return r.at == nil || r.at(s, a, b)
}

// apply swaps each of found in s for what put makes of its value, the
// first and longest where they overlap, nothing inside a placeholder.
func apply(s string, found []span, put func(kind, v string) string) (string, int) {
	if len(found) == 0 {
		return s, 0
	}
	// nothing inside a placeholder already there
	if strings.Contains(s, "{{") {
		for _, p := range placeholderRe.FindAllStringIndex(s, -1) {
			found = slices.DeleteFunc(found, func(f span) bool { return f.start < p[1] && f.end > p[0] })
		}
		if len(found) == 0 {
			return s, 0
		}
	}
	// the first to start, and of those the longest, wins where they overlap;
	// of two the same, the rule found first, so a value's placeholder is the
	// same every time (an API key in API_KEY=… is an API_KEY, not a SECRET)
	sort.SliceStable(found, func(i, j int) bool {
		if found[i].start != found[j].start {
			return found[i].start < found[j].start
		}
		return found[i].end > found[j].end
	})
	var b strings.Builder
	last, n := 0, 0
	for _, f := range found {
		if f.start < last {
			continue
		}
		b.WriteString(s[last:f.start])
		b.WriteString(put(f.kind, s[f.start:f.end]))
		last, n = f.end, n+1
	}
	b.WriteString(s[last:])
	return b.String(), n
}

func containsAny(s string, subs []string) bool {
	for _, x := range subs {
		if strings.Contains(s, x) {
			return true
		}
	}
	return false
}

// ---- restoring ------------------------------------------------------------------

// placeholderBody is a placeholder without its braces, as a model may write
// one back: API_KEY_k3v9x2mq, phone_b5puzbfv, PHONE_B5PUZBFV.
var placeholderBody = regexp.MustCompile(`[A-Za-z][A-Za-z0-9_]*_[A-Za-z2-7]{8}`)

// Restore puts the values back in s. escaped is for text that is itself
// JSON (a tool call's arguments), where a value goes in escaped.
//
// A model doesn't always write a placeholder back as it was given: it
// writes {{ PHONE_b5puzbfv }} as a template has it, drops a brace, or the
// braces, or changes the case. Any of those is put back too when what is
// left names a placeholder this magpie made: its kind and the 8 of its
// hash, which no other text has by chance (shensmart on Discord).
func Restore(s string, escaped bool) string {
	if !strings.Contains(s, "_") {
		return s
	}
	mu.RLock()
	defer mu.RUnlock()
	if len(values) == 0 {
		return s
	}
	var b strings.Builder
	last := 0
	for _, m := range placeholderBody.FindAllStringIndex(s, -1) {
		a, z := m[0], m[1]
		if a < last || a > 0 && isWordByte(s[a-1]) || z < len(s) && isWordByte(s[z]) {
			continue
		}
		v, ok := values[canonical(s[a:z])]
		if !ok {
			continue
		}
		a, z = braces(s, a, z)
		if a < last {
			continue
		}
		b.WriteString(s[last:a])
		if escaped {
			v = jsonEscape(v)
		}
		b.WriteString(v)
		last = z
	}
	if last == 0 {
		return s
	}
	b.WriteString(s[last:])
	return b.String()
}

// canonical is the placeholder Mask wrote for body, whatever its case.
func canonical(body string) string {
	cut := len(body) - 8
	return "{{" + strings.ToUpper(body[:cut]) + strings.ToLower(body[cut:]) + "}}"
}

func isWordByte(c byte) bool {
	return c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '_'
}

// braces widens s[a:z], a placeholder's body, over the braces the model
// wrote around it, and the spaces inside them: both of {{ and }}, or one
// of them dropped ({{X} or {X}}), or just {{ or just }}. A single brace
// on one side alone is the text's own ({X as in code) and stays.
func braces(s string, a, z int) (int, int) {
	la, l := a, 0
	for la > 0 && (s[la-1] == ' ' || s[la-1] == '\t') {
		la--
	}
	for l < 2 && la > 0 && s[la-1] == '{' {
		la--
		l++
	}
	rz, r := z, 0
	for rz < len(s) && (s[rz] == ' ' || s[rz] == '\t') {
		rz++
	}
	for r < 2 && rz < len(s) && s[rz] == '}' {
		rz++
		r++
	}
	if l == 2 || l == 1 && r >= 1 {
		a = la
	}
	if r == 2 || r == 1 && l >= 1 {
		z = rz
	}
	return a, z
}

// maxPlaceholder is the longest placeholder Mask writes: {{, a kind (a
// user's rule may name one up to maxKind long), _, the 8 of the hash, }}.
const maxPlaceholder = len("{{") + maxKind + len("_") + 8 + len("}}")

// partialTail is how much of the end of s may be the start of a
// placeholder the next piece of a stream finishes, in any of the ways
// Restore takes one back: {{API_, {{ phone_b5, PHONE_b5puzbfv} — or, with
// no brace before it, a last word that starts as a placeholder this magpie
// made does (PHO, PHONE_b5pu), or is one whose }} may still come.
func partialTail(s string) int {
	if n := braceTail(s); n > 0 {
		return n
	}
	return wordTail(s)
}

// braceTail is a tail from {, or {{, on: then spaces, a placeholder's body
// or the start of one, then spaces and one } at most.
func braceTail(s string) int {
	i := strings.LastIndexByte(s, '{')
	if i < 0 {
		return 0
	}
	if i > 0 && s[i-1] == '{' {
		i--
	}
	t := s[i:]
	if len(t) >= maxPlaceholder+4 || strings.Contains(t, "}}") {
		return 0
	}
	body := strings.TrimLeft(t, "{")
	body = strings.TrimLeft(body, " \t")
	if body == "" {
		return len(t) // {, {{, {{␠
	}
	if c := body[0]; !(c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z') {
		return 0
	}
	j := 0
	for j < len(body) && isWordByte(body[j]) {
		j++
	}
	rest := strings.TrimLeft(body[j:], " \t")
	if rest != "" && rest != "}" {
		return 0
	}
	if j < len(body) && placeholderBody.FindString(body[:j]) != body[:j] {
		return 0 // a space or a brace after something that is no placeholder
	}
	return len(t)
}

// wordTail is the last word of s, where it may be a placeholder's body as
// a model writes one without braces: the start of one of the kinds this
// magpie has made placeholders of, up to the 8 of its hash. A word of
// text that only starts like a kind is held for an event and then goes
// on, unchanged.
func wordTail(s string) int {
	i := len(s)
	for i > 0 && isWordByte(s[i-1]) {
		i--
	}
	w := s[i:]
	if w == "" || len(w) > maxKind+9 || !(w[0] >= 'A' && w[0] <= 'Z') {
		return 0
	}
	u := strings.ToUpper(w)
	mu.RLock()
	defer mu.RUnlock()
	for k := range kinds {
		k += "_"
		if strings.HasPrefix(k, u) {
			return len(w)
		}
		if strings.HasPrefix(u, k) && len(u)-len(k) <= 8 && strings.Trim(strings.ToLower(u[len(k):]), "abcdefghijklmnopqrstuvwxyz234567") == "" {
			return len(w)
		}
	}
	return 0
}
