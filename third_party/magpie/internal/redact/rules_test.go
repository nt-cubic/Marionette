package redact

import (
	"strings"
	"testing"
)

// a gateway's key, synthetic, as #195 has it
const ocKey = "oc_sk_AAAABBBBccccDDDD1234eeeeFFFF5678"

// every row of #195's table, and what must stay alone as key is let in
func TestMaskGatewayKey(t *testing.T) {
	cases := []struct {
		in   string
		gone bool
		kind string
	}{
		{in: `"key": "` + ocKey + `"`, gone: true, kind: "API_KEY"},
		{in: `\"key\": \"` + ocKey + `\"`, gone: true, kind: "API_KEY"},
		{in: `"apiKey": "` + ocKey + `"`, gone: true, kind: "API_KEY"},
		{in: `"token": "` + ocKey + `"`, gone: true, kind: "API_KEY"},
		{in: "bare " + ocKey + " value", gone: true, kind: "API_KEY"},
		{in: "or_sk_0123456789abcdefghijKLMNOP", gone: true, kind: "API_KEY"},
		// key alone, a value that is no API key
		{in: `{"id": "relay", "key": "Zq81nVx0pL2mWe7RtY4u"}`, gone: true, kind: "SECRET"},
		{in: `{\"key\": \"Zq81nVx0pL2mWe7RtY4u\"}`, gone: true, kind: "SECRET"},
		{in: "key: Zq81nVx0pL2mWe7RtY4u", gone: true, kind: "SECRET"},
		{in: `"api-key": "Zq81nVx0pL2mWe7RtY4u"`, gone: true, kind: "SECRET"},
		{in: `"apikey": "Zq81nVx0pL2mWe7RtY4u"`, gone: true, kind: "SECRET"},
		{in: `\"apiKey\": \"Zq81nVx0pL2mWe7RtY4u\"`, gone: true, kind: "SECRET"},
		// and key as code says it
		{in: `{"key": "value", "sort_key": "xyz"}`},
		{in: `key = "someIdentifier"`},
		{in: `key = "someVeryLongIdentifierName"`},
		{in: `sort_key: created_at_2024_rows`},
		{in: `monkey: banana1234567890abc`},
		{in: `obj.key = "abc123"`},
		{in: `"key": "row-12"`},
		{in: "the task_sk_something_long_name_here module"}, // no digits
		{in: "abcde_sk_0123456789abcdefghijKL"},             // five letters is a word
		{in: `"key": "${OPENCODE_API_KEY_0123456789}"`},
	}
	for _, c := range cases {
		out, n := Mask(c.in, Options{Secrets: true})
		if !c.gone {
			if n != 0 || out != c.in {
				t.Errorf("%q masked: %q", c.in, out)
			}
			continue
		}
		if n != 1 || !strings.Contains(out, "{{"+c.kind+"_") {
			t.Errorf("%q: %d masked, want one %s: %q", c.in, n, c.kind, out)
		}
		if back := Restore(out, false); back != c.in {
			t.Errorf("round trip: %q → %q → %q", c.in, out, back)
		}
	}
	// the same key, the same placeholder, whatever field it is in
	a, _ := Mask(`"key": "`+ocKey+`"`, Options{Secrets: true})
	b, _ := Mask("OC_API_KEY="+ocKey, Options{Secrets: true})
	c, _ := Mask(ocKey, Options{Secrets: true})
	if pa, pb, pc := placeholderRe.FindString(a), placeholderRe.FindString(b), placeholderRe.FindString(c); pa == "" || pa != pb || pa != pc {
		t.Fatalf("placeholders differ: %q %q %q", a, b, c)
	}
}

// Volcano Ark and the relays in front of it issue keys with an ark- prefix
// (#2946); no value rule knew one. A name that has api_key in it (ARK_API_KEY)
// masks its value by the name already; a key on its own, under another name
// or in a request body, went as it was. A word that only has ark- in it is
// no key. Each row has a value of its own, so no row's masking is the work
// of a value another row made known.
func TestMaskArkKey(t *testing.T) {
	const (
		inEnvName = "ark-c8b7e5a9-4f3d-4e2a-9b1c-7d6e5f4a3b2c"
		underArk  = "ark-6f2a9d47-1c3b-8e5f-a7d9-0b4c2e6f8a1d"
		inProse   = "ark-93be1f6c-7a2d-4b8e-9c0f-5d3a7e1b9c2a"
		inBody    = "ark-2e7c4a19-8d3f-6b5a-1e9c-7f4b2d8a3c6e"
		noKey     = "the dark-4a1c8f2e-9b3d-7e6a-0c5f-1d8b4a2e9f7b code"
	)
	for _, c := range []struct {
		in   string
		json bool
		gone bool
	}{
		{in: "ARK_API_KEY=" + inEnvName, gone: true},
		{in: "ARK_KEY=" + underArk, gone: true},
		{in: "the ark key is " + inProse, gone: true},
		{in: `{"notes":"relay key ` + inBody + ` for the plan"}`, json: true, gone: true},
		{in: noKey},
		{in: "ark-key"},
	} {
		var out string
		var n int
		if c.json {
			var b []byte
			b, n = MaskJSON([]byte(c.in), Options{Secrets: true})
			out = string(b)
		} else {
			out, n = Mask(c.in, Options{Secrets: true})
		}
		if !c.gone {
			if n != 0 || out != c.in {
				t.Errorf("%q masked: %q", c.in, out)
			}
			continue
		}
		if n != 1 || strings.Contains(out, "ark-") {
			t.Errorf("%q: %d masked: %q", c.in, n, out)
			continue
		}
		var back string
		if c.json {
			back = string(RestoreJSON([]byte(out)))
		} else {
			back = Restore(out, false)
		}
		if back != c.in {
			t.Errorf("round trip: %q → %q → %q", c.in, out, back)
		}
	}
}

// An env var's name says what it holds in the upper case: a SERVICE_X_KEY,
// a DB_PASS (#2946). In the lower case a name that only ends in key holds
// no secret: a sort key, a cache key, a public key.
func TestMaskUpperEnvNames(t *testing.T) {
	for _, c := range []struct {
		in   string
		gone bool
	}{
		{in: "SERVICE_X_KEY=Vt57wQx2mLp9kNr4Zc8b", gone: true},
		{in: "FOO_KEY=Jw83hT6yQv2nRs9Xd5ke", gone: true},
		{in: `DB_PASS="Mr4tQw9xZb7n"`, gone: true},
		{in: "DB_PASS=Kp8vHs3wTn6j", gone: true},
		{in: "sort_key: created_at_2024_rows"},
		{in: "cache_key: lru_items_01"},
		{in: "public_key = 04a1b2c3d4e5f6a7b8"},
		{in: "db_pass=zz52Qx9vwR3n"},
		{in: "SORT_KEY=created_at_2024"},
		{in: "PUBLIC_KEY=04a1b2c3d4e5f6a7b8"},
		{in: "PROMPT_CACHE_KEY=conv_12345678"},
		{in: "SERVICE_X_KEY: Uint8Array;"},
		{in: "DEBUG_PASS: options.pass8,"},
	} {
		out, n := Mask(c.in, Options{Secrets: true})
		if !c.gone {
			if n != 0 || out != c.in {
				t.Errorf("%q masked: %q", c.in, out)
			}
			continue
		}
		if n != 1 || strings.Contains(out, "Vt57wQx2") || strings.Contains(out, "Jw83hT6y") ||
			strings.Contains(out, "Mr4tQw9x") || strings.Contains(out, "Kp8vHs3w") {
			t.Errorf("%q: %d masked: %q", c.in, n, out)
			continue
		}
		if back := Restore(out, false); back != c.in {
			t.Errorf("round trip: %q → %q → %q", c.in, out, back)
		}
	}
}

// An Authorization or Cookie header's value is a secret whatever it looks
// like: an opaque bearer token, a session value (#2946). Prose about them,
// and a header of another name, are not.
func TestMaskAuthHeaders(t *testing.T) {
	const jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJlc2lnbmF0dXJl"
	for _, c := range []struct {
		in string
		n  int
	}{
		{"Authorization: Bearer mfa_9x2kQ7wLr4Tz", 1},
		{"authorization: Basic Zm9vYmFyYmF6", 1},
		{"Cookie: sid=8f3kz9a1x2c4", 1},
		{"cookie: session=deadbeef42", 1},
		{"POST /v1 HTTP/1.1\nAuthorization: Bearer tt5Za8wQ3nR7kLmx\nCookie: sid=pq93Xm2vB8n4Rk\n", 2},
		{"Authorization: Bearer " + jwt, 1},
		{"the authorization is required", 0},
		{"cookies: 3", 0},
		{"Authorization: the user must sign in first", 0},
		{"Authorization: required", 0},
		{"Cookie: see the docs for how they are kept", 0},
		{"Authorization: Bearer ${API_TOKEN}", 0},
		{"Cookie: sid=Rk82mQ4xVb; theme=dark", 1},
	} {
		out, n := Mask(c.in, Options{Secrets: true})
		if n != c.n {
			t.Errorf("%q: %d masked, want %d: %q", c.in, n, c.n, out)
			continue
		}
		if strings.Contains(out, "mfa_9x2kQ7wLr4Tz") || strings.Contains(out, "8f3kz9a1x2c4") ||
			strings.Contains(out, "Zm9vYmFyYmF6") || strings.Contains(out, "deadbeef42") ||
			strings.Contains(out, "tt5Za8wQ3nR7kLmx") || strings.Contains(out, "pq93Xm2vB8n4Rk") {
			t.Errorf("%q: a value went out: %q", c.in, out)
			continue
		}
		if back := Restore(out, false); back != c.in {
			t.Errorf("round trip: %q → %q → %q", c.in, out, back)
		}
	}
}

// the markers of a rule's names, made from its list, are in any name it takes
func TestSecretMarkers(t *testing.T) {
	for _, name := range []string{"DB_PASSWORD", "passwd", "client_secret", "Token", "apiKey", "API-KEY", "AccessKey", "private_key", "CREDENTIAL", "Credentials"} {
		if !containsAny(name, secretNames.markers()) {
			t.Errorf("%s: no marker", name)
		}
	}
}

func TestRuleKind(t *testing.T) {
	for in, want := range map[string]string{
		"":                                   "CUSTOM",
		"api key":                            "API_KEY",
		"oc-sk":                              "OC_SK",
		"{{x}}":                              "X",
		"9lives":                             "CUSTOM_9LIVES",
		"数据库":                                "CUSTOM",
		"a_very_long_kind_name_that_goes_on": "A_VERY_LONG_KIND_NAME_TH",
	} {
		if got := RuleKind(in); got != want {
			t.Errorf("RuleKind(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestCheckRules(t *testing.T) {
	got, err := CheckRules([]Rule{{Kind: " gw key ", Prefix: " acme_ "}, {Regex: "  "}, {Kind: "GW_KEY", Prefix: "acme_"}, {Kind: "ns", Regex: `ns-([0-9a-f]{12})`}})
	if err != nil || len(got) != 2 || got[0] != (Rule{Kind: "GW_KEY", Prefix: "acme_"}) || got[1].Kind != "NS" {
		t.Fatalf("%+v %v", got, err)
	}
	for _, bad := range [][]Rule{
		{{Regex: `ns-([0-9a-f`}},                   // doesn't compile
		{{Regex: `(?P<x>a)(?<!b)`}},                // RE2 has no lookbehind
		{{Regex: `x*`}},                            // matches nothing at all
		{{Prefix: "ab"}},                           // too short a prefix
		{{Prefix: "acme_", Regex: "acme_[a-z]+"}},  // both
		{{Regex: strings.Repeat("a", maxRegex+1)}}, // too long
	} {
		if _, err := CheckRules(bad); err == nil {
			t.Errorf("%+v accepted", bad)
		}
	}
	many := make([]Rule, MaxRules+1)
	for i := range many {
		many[i] = Rule{Prefix: "p" + strings.Repeat("x", i+2)}
	}
	if _, err := CheckRules(many); err == nil {
		t.Error("too many rules accepted")
	}
}

func TestCustomRules(t *testing.T) {
	rules := []Rule{
		{Kind: "GW_KEY", Prefix: "acme-"},
		{Kind: "NS_TOKEN", Regex: `nstok:([0-9a-f]{12})`},
		{Kind: "BROKEN", Regex: `x(`}, // a settings file written by hand: left out
	}
	o := Options{Secrets: true, Rules: rules}
	in := "use acme-Zx9_ab-12cd then nstok:0123456789ab and " + openaiKey
	out, n := Mask(in, o)
	if n != 3 || strings.Contains(out, "acme-Zx9") || strings.Contains(out, "0123456789ab") ||
		!strings.Contains(out, "{{GW_KEY_") || !strings.Contains(out, "nstok:{{NS_TOKEN_") || !strings.Contains(out, "{{API_KEY_") {
		t.Fatalf("%d: %q", n, out)
	}
	if back := Restore(out, false); back != in {
		t.Fatalf("restored %q", back)
	}
	// the same placeholder turn after turn
	again, _ := Mask("again: acme-Zx9_ab-12cd", o)
	if p1, p2 := placeholderRe.FindString(out), placeholderRe.FindString(again); p1 == "" || p1 != p2 {
		t.Fatalf("placeholders differ: %q %q", out, again)
	}
	// a prefix wants 8 more characters after it
	if _, n := Mask("acme-short", o); n != 0 {
		t.Fatal("short value masked")
	}
	// only with the secrets
	if _, n := Mask(in, Options{Rules: rules}); n != 0 {
		t.Fatal("masked with secrets off")
	}
	// where the user's and magpie's find the same value, the user's name
	// wins; where they overlap, the first to start and the longest
	o = Options{Secrets: true, Rules: []Rule{{Kind: "OC", Prefix: "oc_sk_"}}}
	if out, _ := Mask(`"key": "`+ocKey+`"`, o); !strings.Contains(out, "{{OC_") || strings.Contains(out, "API_KEY") {
		t.Fatalf("user's kind lost: %q", out)
	}
	o = Options{Secrets: true, Rules: []Rule{{Kind: "TAIL", Regex: `BBBBcccc[A-Za-z0-9]+`}}}
	if out, n := Mask(ocKey, o); n != 1 || !strings.HasPrefix(out, "{{API_KEY_") {
		t.Fatalf("overlap: %q", out)
	}
	// a kind with digits restores, in a stream too
	o = Options{Secrets: true, Rules: []Rule{{Kind: "K8S", Prefix: "k8s-tok-"}}}
	out, _ = Mask("k8s-tok-abcdef123456", o)
	if !strings.HasPrefix(out, "{{K8S_") || Restore(out, false) != "k8s-tok-abcdef123456" || partialTail(out[:len(out)-3]) == 0 {
		t.Fatalf("digits in kind: %q", out)
	}
	// a kind as long as a rule may have is held in a stream up to its last
	// character, as a short one is
	o = Options{Secrets: true, Rules: []Rule{{Kind: "internal gateway token x", Prefix: "igt-"}}}
	out, _ = Mask("igt-abcdef123456", o)
	if !strings.HasPrefix(out, "{{INTERNAL_GATEWAY_TOKEN_X_") || len(out) != len("{{")+maxKind+len("_")+8+len("}}") {
		t.Fatalf("long kind: %q", out)
	}
	for i := 1; i < len(out); i++ {
		if n := partialTail("text " + out[:i]); n != i {
			t.Fatalf("long kind cut at %d: held %d of %q", i, n, out[:i])
		}
	}
}
