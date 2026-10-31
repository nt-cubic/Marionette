package gui

import (
	"fmt"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/maskrec"
)

// The notification that masking swapped a value (Must2333 on X: "我还希望
// 能即时的知道发生了替换，但不是通过随时监控网关的方式"). It says what kind of
// value and how many, and which agent sent it; never the value, nor any
// part of it (a record's Head is not read here).
//
// It stays quiet: only a value masked for the first time is news
// (maskrec.Fresh), since a conversation sends its whole history again each
// turn; the records of a burst (an agent's tool calls, one after another)
// are gathered for maskGather and said as one; and one is said at most
// every maskGap, what comes in between said after it.

// maskGather is how long after a record its notification waits for more;
// maskGap is the least time from one notification to the next.
var (
	maskGather = 3 * time.Second
	maskGap    = time.Minute
)

// maskPendingMax bounds the records waiting; past it a record is dropped,
// its values still masked and in the 脱敏 page, only not told.
const maskPendingMax = 5000

// maskNote is one notification: the new values by kind, the agents that
// sent them in the order they came, and the latest record with one, which
// a click opens the 脱敏 page on.
type maskNote struct {
	Kinds  map[string]int
	Agents []string
	Key    int64
}

func (n maskNote) total() int {
	t := 0
	for _, c := range n.Kinds {
		t += c
	}
	return t
}

type maskAlerts struct {
	mu      sync.Mutex
	pending []maskrec.Record
	waiting bool      // a flush is set to run
	last    time.Time // the last notification said

	on    func() bool // the setting, read as a record comes and as it is said
	fresh func(maskrec.Record) []maskrec.Value
	send  func(maskNote)
	now   func() time.Time
	after func(time.Duration, func())
}

func newMaskAlerts(on func() bool, send func(maskNote)) *maskAlerts {
	return &maskAlerts{on: on, send: send, fresh: maskrec.Fresh, now: time.Now,
		after: func(d time.Duration, f func()) { time.AfterFunc(d, f) }}
}

// add takes a request's record; it is on the request's way, so it only
// keeps it.
func (m *maskAlerts) add(r maskrec.Record) {
	if !m.on() {
		return
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if len(m.pending) >= maskPendingMax {
		return
	}
	m.pending = append(m.pending, r)
	if m.waiting {
		return
	}
	m.waiting = true
	wait := maskGather
	if !m.last.IsZero() {
		wait = max(wait, m.last.Add(maskGap).Sub(m.now()))
	}
	m.after(wait, m.flush)
}

// flush says what the records waiting masked for the first time, if any.
func (m *maskAlerts) flush() {
	m.mu.Lock()
	rs := m.pending
	m.pending, m.waiting = nil, false
	m.mu.Unlock()
	if !m.on() {
		return
	}
	n := maskNote{Kinds: map[string]int{}}
	seen := map[string]bool{}
	for _, r := range rs {
		got := false
		for _, v := range m.fresh(r) {
			if seen[v.Placeholder] {
				continue
			}
			seen[v.Placeholder] = true
			n.Kinds[v.Kind]++
			got = true
		}
		if !got {
			continue
		}
		n.Key = max(n.Key, r.Key)
		if r.Agent != "" && !slices.Contains(n.Agents, r.Agent) {
			n.Agents = append(n.Agents, r.Agent)
		}
	}
	if len(n.Kinds) == 0 {
		return
	}
	m.mu.Lock()
	m.last = m.now()
	m.mu.Unlock()
	m.send(n)
}

// maskKinds are the kinds' names in a notification, in en, zh, zh-TW, ja
// and de: the 脱敏 page's own words for them (MASK_KINDS in app.js).
var maskKinds = map[string][5]string{
	"API_KEY":     {"API keys", "API 密钥", "API 金鑰", "API キー", "API-Schlüssel"},
	"PRIVATE_KEY": {"Private keys", "私钥", "私鑰", "秘密鍵", "Private Schlüssel"},
	"TOKEN":       {"Tokens", "Token", "Token", "トークン", "Token"},
	"PASSWORD":    {"Passwords", "密码", "密碼", "パスワード", "Passwörter"},
	"SECRET":      {"Secrets", "机密", "機密", "シークレット", "Geheimnisse"},
	"EMAIL":       {"Emails", "邮箱", "電子郵件", "メール", "E-Mails"},
	"PHONE":       {"Phone numbers", "手机号", "電話號碼", "電話番号", "Telefonnummern"},
	"ID_CARD":     {"ID numbers", "身份证号", "身分證號", "身分証番号", "Ausweisnummern"},
	"BANK_CARD":   {"Bank cards", "银行卡号", "銀行卡號", "銀行カード", "Bankkarten"},
	"PERSON":      {"Names", "姓名", "姓名", "氏名", "Namen"},
	"ADDRESS":     {"Addresses", "地址", "地址", "住所", "Adressen"},
	"ACCOUNT":     {"Account numbers", "账号", "帳號", "口座番号", "Kontonummern"},
	"SSN":         {"SSNs", "社会安全号", "社會安全號碼", "社会保障番号", "SSNs"},
	"PASSPORT":    {"Passports", "护照号", "護照號碼", "パスポート番号", "Reisepässe"},
	"IBAN":        {"IBANs", "IBAN", "IBAN", "IBAN", "IBANs"},
	"BIRTHDAY":    {"Birthdays", "生日", "生日", "誕生日", "Geburtstage"},
	"MAC":         {"MAC addresses", "MAC 地址", "MAC 位址", "MAC アドレス", "MAC-Adressen"},
	"SERIAL":      {"Serial numbers", "序列号", "序號", "シリアル番号", "Seriennummern"},
	"USERNAME":    {"User names", "用户名", "使用者名稱", "ユーザー名", "Benutzernamen"},
	"HOSTNAME":    {"Computer names", "电脑名", "電腦名稱", "コンピュータ名", "Computernamen"},
	"IP":          {"IP addresses", "IP 地址", "IP 位址", "IP アドレス", "IP-Adressen"},
	"BUCKET":      {"Buckets", "存储桶", "儲存桶", "バケット", "Buckets"},
	"TERM":        {"Your words", "你的词", "你的詞", "あなたの語", "Deine Wörter"},
}

func langIndex(lang string) int {
	switch lang {
	case "zh":
		return 1
	case "zh-TW":
		return 2
	case "ja":
		return 3
	case "de":
		return 4
	}
	return 0
}

// maskKindName is a kind as the notification names it; one of the user's
// own rules is named by the kind they gave it.
func maskKindName(lang, kind string) string {
	if names, ok := maskKinds[kind]; ok {
		return names[langIndex(lang)]
	}
	return kind
}

// maskAlertText is the notification in lang: how many values were masked
// for the first time, then each kind with its count, the most first, and
// the agents that sent them.
func maskAlertText(lang string, n maskNote, agentName func(string) string) (title, body string) {
	kinds := make([]string, 0, len(n.Kinds))
	for k := range n.Kinds {
		kinds = append(kinds, k)
	}
	sort.Slice(kinds, func(i, j int) bool {
		if n.Kinds[kinds[i]] != n.Kinds[kinds[j]] {
			return n.Kinds[kinds[i]] > n.Kinds[kinds[j]]
		}
		return kinds[i] < kinds[j]
	})
	colon, comma := ": ", ", "
	if lang == "zh" || lang == "zh-TW" || lang == "ja" {
		colon, comma = "：", "、"
	}
	var parts []string
	for _, k := range kinds {
		parts = append(parts, fmt.Sprintf("%s%s%d", maskKindName(lang, k), colon, n.Kinds[k]))
	}
	body = strings.Join(parts, comma)
	if len(n.Agents) > 0 {
		names := make([]string, 0, 2)
		for _, a := range n.Agents[:min(2, len(n.Agents))] {
			names = append(names, agentName(a))
		}
		who := strings.Join(names, comma)
		if more := len(n.Agents) - 2; more > 0 {
			who += fmt.Sprintf(" +%d", more)
		}
		body += " · " + who
	}
	t := n.total()
	switch lang {
	case "zh":
		title = fmt.Sprintf("已脱敏 %d 个新值", t)
	case "zh-TW":
		title = fmt.Sprintf("已脫敏 %d 個新值", t)
	case "ja":
		title = fmt.Sprintf("新しい値を %d 件マスクしました", t)
	case "de":
		if t == 1 {
			title = "1 neuer Wert maskiert"
		} else {
			title = fmt.Sprintf("%d neue Werte maskiert", t)
		}
	default:
		if t == 1 {
			title = "Masked 1 new value"
		} else {
			title = fmt.Sprintf("Masked %d new values", t)
		}
	}
	return title, body
}
