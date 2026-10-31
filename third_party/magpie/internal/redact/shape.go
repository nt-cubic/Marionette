package redact

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/binary"
	"net/netip"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

// A key can go to the vendor as a made-up key of its own form instead of
// a placeholder (Options.Shape): sk_w3axCByTQgK1Px… as sk_hYQWRBFmdHNI…,
// with the same prefix, length and separators and the same kind of
// character in each place. An agent's or a tool's check of a key's form
// passes, and the model has no placeholder to remark on (Charles Cao on X).
// It is made from the placeholder key as a placeholder is, so one key goes
// as the same made-up one every turn and a vendor's cache still matches,
// and it comes back as it was wherever the vendor writes it.
//
// Only a secret's kind, of minShape characters or more with minRandom of
// them made up (not its prefix or separators), and only letters, digits,
// '-', '_' and '.': a made-up key like that is in no other text by chance,
// and it needs no escaping in JSON. Anything else goes as a placeholder
// still. 16 left a short key such as sk_abcdefg a placeholder, which read
// as the setting not working.

const (
	minShape  = 8
	minRandom = 6
)

var (
	// shapes are the made-up keys in values, with their kinds; guarded by
	// mu, as values is
	shapes = map[string]string{}
)

// keyPrefixes name a key's kind across more than one part, and are kept as
// they are; keyHead keeps one part of a few letters.
var keyPrefixes = []string{"sk-ant-api03-", "sk-ant-oat01-", "sk-ant-admin01-", "sk-proj-", "sk-svcacct-", "sk-or-v1-", "github_pat_"}

// keyHead is the start of v that names its kind and stays as it is: one of
// keyPrefixes, or up to four letters and the '-' or '_' after them (sk-,
// ghp_, xoxb-), or AWS's AKIA and ASIA. A head without its separator could
// be the key's own random letters, which stay with the user.
func keyHead(v string) string {
	for _, p := range keyPrefixes {
		if strings.HasPrefix(v, p) {
			return p
		}
	}
	if strings.HasPrefix(v, "AKIA") || strings.HasPrefix(v, "ASIA") {
		return v[:4]
	}
	for i := 0; i < len(v) && i <= 4; i++ {
		c := v[i]
		if (c == '-' || c == '_') && i > 0 {
			return v[:i+1]
		}
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z') {
			break
		}
	}
	return ""
}

// namedValue is a name before = or : and the value after it.
var namedValue = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_.-]*[ \t]*[=:][ \t]*()`)

// splitName parts a value found with its name, as 密钥
// OPENAI_API_KEY=sk_… is, into the name, which stays as written, and the
// key after it, which alone is made up and put back.
func splitName(v string) (name, key string) {
	if m := namedValue.FindStringSubmatchIndex(v); m != nil {
		return v[:m[2]], v[m[2]:]
	}
	return "", v
}

func shapeByte(c byte) bool {
	return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_' || c == '.'
}

// shapeOf is the made-up key v of kind goes as, if it may go as one.
func shapeOf(kind, v string) (string, bool) {
	if _, personal := kindCats[kind]; personal || kind == "TERM" || len(v) < minShape {
		return "", false
	}
	for i := 0; i < len(v); i++ {
		if !shapeByte(v[i]) {
			return "", false
		}
	}
	head := keyHead(v)
	rest := v[len(head):]
	// a hex key's characters are any hex digit, of the case it has
	hexLower, hexUpper := true, true
	for i := 0; i < len(rest); i++ {
		c := rest[i]
		switch {
		case c >= '0' && c <= '9', c == '-' || c == '_' || c == '.':
		case c >= 'a' && c <= 'f':
			hexUpper = false
		case c >= 'A' && c <= 'F':
			hexLower = false
		default:
			hexLower, hexUpper = false, false
		}
	}
	h := hmac.New(sha256.New, key())
	h.Write([]byte("shape\x00" + kind + "\x00" + v))
	seed := h.Sum(nil)
	var stream []byte
	n := 0
	next := func(k int) byte {
		if len(stream) == 0 {
			h := hmac.New(sha256.New, seed)
			var c [8]byte
			binary.BigEndian.PutUint64(c[:], uint64(n))
			n++
			h.Write(c[:])
			stream = h.Sum(nil)
		}
		b := stream[0]
		stream = stream[1:]
		return b % byte(k)
	}
	const (
		upper = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
		lower = "abcdefghijklmnopqrstuvwxyz"
		digit = "0123456789"
	)
	out, made := []byte(head), 0
	for i := 0; i < len(rest); i++ {
		c := rest[i]
		set := ""
		switch {
		case c == '-' || c == '_' || c == '.':
		case hexLower:
			set = digit + "abcdef"
		case hexUpper:
			set = digit + "ABCDEF"
		case c >= 'A' && c <= 'Z':
			set = upper
		case c >= 'a' && c <= 'z':
			set = lower
		default:
			set = digit
		}
		if set == "" {
			out = append(out, c)
			continue
		}
		made++
		out = append(out, set[next(len(set))])
	}
	if made < minRandom {
		return "", false
	}
	f := string(out)
	return f, f != v
}

// shaped is put for Options.Shape: v as a made-up key of its form where it
// may go as one, else as its placeholder.
func shaped(kind, v string) string {
	name, k := splitName(v)
	f, ok := shapeOf(kind, k)
	if !ok {
		return placeholder(kind, v)
	}
	mu.Lock()
	have, taken := values[f]
	if !taken {
		makeRoom()
		values[f] = k
		kinds[kind] = true
		shapes[f] = kind
		addKnown(f, k)
	}
	mu.Unlock()
	if taken && have != k {
		// another value's made-up key, which nothing ever is by chance
		return placeholder(kind, v)
	}
	return name + f
}

// restoreShapes puts back each made-up key in s, counting it in put
// where put isn't nil; called with mu held for reading. A made-up key
// needs no escaping, nor does the value it stands for. A stand-in address
// is put back only where it is a whole address: 203.0.113.147 is no part
// of 203.0.113.1470 or 10.203.0.113.147.
func restoreShapes(s string, put map[string]int) string {
	for f, kind := range shapes {
		if kind == "IP" {
			s = restoreAddr(s, f, values[f], put)
			continue
		}
		if n := strings.Count(s, f); n > 0 {
			s = strings.ReplaceAll(s, f, values[f])
			if put != nil {
				put[f] += n
			}
		}
	}
	return s
}

// restoreAddr is s with each whole stand-in address f in it put back as
// v, counted in put where put isn't nil.
func restoreAddr(s, f, v string, put map[string]int) string {
	if !strings.Contains(s, f) {
		return s
	}
	v6 := strings.Contains(f, ":")
	var b strings.Builder
	last := 0
	for i := 0; ; {
		j := strings.Index(s[i:], f)
		if j < 0 {
			break
		}
		a, z := i+j, i+j+len(f)
		i = a + 1
		if a > 0 && addrByte(s[a-1], v6) || z < len(s) && addrByte(s[z], v6) && s[z] != '.' ||
			z+1 < len(s) && s[z] == '.' && s[z+1] >= '0' && s[z+1] <= '9' {
			continue
		}
		b.WriteString(s[last:a])
		b.WriteString(v)
		last, i = z, z
		if put != nil {
			put[f]++
		}
	}
	if last == 0 {
		return s
	}
	b.WriteString(s[last:])
	return b.String()
}

// addrByte says c may be part of an address, so an address beside it is
// a piece of a longer one: a letter or digit, a dot, and beside an IPv6
// one a colon (an IPv4 one has its port after one, or a field's name
// before: ip:203.0.113.147:22).
func addrByte(c byte, v6 bool) bool {
	return c >= '0' && c <= '9' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c == '.' || v6 && c == ':'
}

// shapeTail is how much of the end of s may be the start of a made-up
// key, which the next piece of a stream goes on with: the longest end of s
// that one of them starts with, after the last whole one in s. A key that
// ends as one starts (…sqZs, sk_…) has come whole, and is put back whole.
func shapeTail(s string) int {
	mu.RLock()
	defer mu.RUnlock()
	whole := 0
	for f := range shapes {
		if i := strings.LastIndex(s, f); i >= 0 {
			whole = max(whole, i+len(f))
		}
	}
	best := 0
	for f := range shapes {
		for k := min(len(f)-1, len(s)-whole); k > best; k-- {
			if strings.HasSuffix(s, f[:k]) {
				best = k
				break
			}
		}
	}
	return best
}

// hasShapes says some made-up key is in b. With none made, as while
// Options.Shape is off, a whole reply isn't copied to find out.
func hasShapes(b []byte) bool {
	mu.RLock()
	defer mu.RUnlock()
	if len(shapes) == 0 {
		return false
	}
	s := string(b)
	for f := range shapes {
		if strings.Contains(s, f) {
			return true
		}
	}
	return false
}

// outsideShapes is found without what lies in a made-up key already in s,
// as a placeholder's inside is left alone: a key that came back unrestored
// is not masked again as a key of its own.
func outsideShapes(s string, found []span) []span {
	mu.RLock()
	defer mu.RUnlock()
	for f := range shapes {
		for i := 0; ; {
			j := strings.Index(s[i:], f)
			if j < 0 {
				break
			}
			a, z := i+j, i+j+len(f)
			found = slices.DeleteFunc(found, func(x span) bool { return x.start < z && x.end > a })
			i = z
		}
	}
	return found
}

// A user name goes to the vendor as a made-up name, not a placeholder:
// C:\Users\eerttreeeert as C:\Users\qmzvkrta, never C:\Users\{{USERNAME_…}}.
// A path with braces in it reads as a template's or a temporary folder's,
// and an agent on mimo-v2.6-flash took the user's home for a temporary
// folder and set out to delete it (eerttreeeert on X, 10-28). A made-up
// name keeps the path a home folder's, which is what it is, and still
// tells the vendor nothing of the user. It is made from the name as a
// made-up key is (shapeOf), so it is the same every turn and a vendor's
// cache still matches, and it is put back wherever the vendor writes it
// (restoreShapes), in a tool call's path too. Only USERNAME's: what the
// user chose to mask (their words) goes as they chose.

// standInLen is a made-up name's length: eight lowercase letters, which
// read as a login name and are in no other text by chance.
const standInLen = 8

// standInOf is the made-up name the user name v goes as.
func standInOf(v string) string {
	h := hmac.New(sha256.New, key())
	h.Write([]byte("user\x00" + v))
	sum := h.Sum(nil)
	out := make([]byte, standInLen)
	for i := range out {
		out[i] = 'a' + sum[i]%26
	}
	return string(out)
}

// A public IP address goes to the vendor as an address from the ranges
// set aside for documentation (RFC 5737's 192.0.2.0/24, 198.51.100.0/24
// and 203.0.113.0/24; RFC 3849's 2001:db8::/32), not a placeholder. Told
// "my server is {{IP_…}}, ssh in", models asked the user for the real
// address or searched ~/.ssh and the environment for it (0x0_air on X,
// 10-28: "你消息里的 IP 被脱敏成 …，我无法连接"); told 203.0.113.147, they
// ran ssh root@203.0.113.147, which comes back as the real address in the
// tool call, so the agent connects where the user asked. An address in
// those ranges is no one's, so a stand-in the vendor tries itself reaches
// nobody, and the IP rule never masks one (notIPs). The same address gets
// the same stand-in every turn while magpie runs; two addresses never
// share one. IPv4 stand-ins end in 101 to 254, so one is never the start
// of another, and come back only as whole addresses (restoreAddr).

// ipNets are the IPv4 documentation ranges stand-ins are made in, and
// ipHosts how many of each range's addresses are used: .101 to .254.
var ipNets = [...]string{"192.0.2.", "198.51.100.", "203.0.113."}

const ipHosts = 154

// ipStandIns are the stand-ins the address v may go as, in the order
// they are tried: n of them, the i-th made by at(i) (every IPv4 one from
// where v's hash starts, or a few IPv6 ones), with nothing locked, so it
// may be called with mu held. False where v is no address.
func ipStandIns(v string) (at func(i int) string, n int, ok bool) {
	a, err := netip.ParseAddr(v)
	if err != nil {
		return nil, 0, false
	}
	k := key()
	sum := func(i int) []byte {
		h := hmac.New(sha256.New, k)
		h.Write([]byte("ip\x00" + a.String()))
		if i >= 0 {
			var c [8]byte
			binary.BigEndian.PutUint64(c[:], uint64(i))
			h.Write(c[:])
		}
		return h.Sum(nil)
	}
	if a.Unmap().Is4() {
		n := len(ipNets) * ipHosts
		start := int(binary.BigEndian.Uint32(sum(-1)[:4]) % uint32(n))
		return func(i int) string {
			slot := (start + i) % n
			return ipNets[slot/ipHosts] + strconv.Itoa(101+slot%ipHosts)
		}, n, true
	}
	return func(i int) string {
		s := sum(i)
		g := []string{"2001", "db8"}
		for k := 0; k < 6; k++ {
			// each group of four digits with no 0 to start, so all are
			// as long and none is the start of another
			g = append(g, strconv.FormatUint(uint64(0x1000+binary.BigEndian.Uint16(s[2*k:])%0xf000), 16))
		}
		return strings.Join(g, ":")
	}, 8, true
}

// ipStandIn is the stand-in the address v goes as, kept to be put back as
// a made-up key is: the one it went as before, or the first that is no
// other value's. False where v is no address or every one is taken.
func ipStandIn(v string) (string, bool) {
	at, n, ok := ipStandIns(v)
	if !ok {
		return "", false
	}
	mu.Lock()
	defer mu.Unlock()
	free := ""
	for i := 0; i < n; i++ {
		f := at(i)
		have, taken := values[f]
		if taken && have == v {
			return f, true
		}
		if !taken && free == "" {
			free = f
		}
	}
	if free == "" {
		return "", false
	}
	makeRoom()
	values[free] = v
	kinds["IP"] = true
	shapes[free] = "IP"
	addKnown(free, v)
	return free, true
}

// standIn is put with a user name sent as its made-up name and an IP
// address as a documentation one, each kept to be put back as a made-up
// key is; put's own where that name is another value's already.
func standIn(put func(kind, v string) string) func(kind, v string) string {
	return func(kind, v string) string {
		if kind == "IP" {
			if f, ok := ipStandIn(v); ok {
				return f
			}
			return put(kind, v)
		}
		if kind != "USERNAME" {
			return put(kind, v)
		}
		f := standInOf(v)
		if f == v {
			return put(kind, v)
		}
		mu.Lock()
		have, taken := values[f]
		if !taken {
			makeRoom()
			values[f] = v
			kinds[kind] = true
			shapes[f] = kind
			addKnown(f, v)
		}
		mu.Unlock()
		if taken && have != v {
			return put(kind, v)
		}
		return f
	}
}
