package redact

import (
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"net/netip"
	"strings"
	"testing"
)

// docAddr says f is an IPv4 stand-in: in RFC 5737's ranges, .101 to .254.
func docAddr(f string) bool {
	a, err := netip.ParseAddr(f)
	if err != nil || !a.Is4() {
		return false
	}
	b := a.As4()
	inNet := b[0] == 192 && b[1] == 0 && b[2] == 2 || b[0] == 198 && b[1] == 51 && b[2] == 100 || b[0] == 203 && b[1] == 0 && b[2] == 113
	return inNet && b[3] >= 101 && b[3] <= 254
}

// A server's address the user gives goes as an address from the
// documentation ranges, not {{IP_…}}: told "my server is {{IP_…}}", models
// asked the user for the real one or searched ~/.ssh for it (0x0_air on X,
// 10-28: "你消息里的 IP 被脱敏成 …，我无法连接"). The model's ssh with the
// stand-in comes back with the real address, in a whole reply and a stream.
func TestIPGoesAsADocumentationAddress(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	const ip = "47.96.123.45"
	o := Options{Personal: true, Kinds: map[string]bool{"ip": true}}
	msg := "我的服务器是 " + ip + "，用户 root，已经配好密钥登录。先 ssh 上去看看 uname -a 和磁盘空间。"
	out, n := Mask(msg, o)
	f := strings.TrimSuffix(strings.Fields(strings.TrimPrefix(out, "我的服务器是 "))[0], "，用户")
	if n != 1 || strings.Contains(out, ip) || strings.Contains(out, "{{") || !docAddr(f) {
		t.Fatalf("sent %q (address %q), want the address as a documentation one", out, f)
	}
	if again, _ := Mask(msg, o); again != out {
		t.Errorf("not the same stand-in twice: %q, %q", out, again)
	}
	if p := Preview(msg, o); p != out {
		t.Errorf("preview %q, sent %q", p, out)
	}
	if back := Restore(out, false); back != msg {
		t.Errorf("put back as %q", back)
	}
	// the model's tool call, as the probe's models wrote it
	call := `{"command":"ssh -o BatchMode=yes -o ConnectTimeout=10 root@` + f + ` 'uname -a; echo ---; df -h'"}`
	if back := Restore(call, true); back != strings.Replace(call, f, ip, 1) {
		t.Errorf("tool call put back as %s", back)
	}
	// only a whole address is put back
	for _, c := range []struct{ s, want string }{
		{f + ":22", ip + ":22"},
		{"ssh to " + f + ".", "ssh to " + ip + "."},
		{"[" + f + "]", "[" + ip + "]"},
		{"ip:" + f, "ip:" + ip},
		{f + "0", f + "0"},
		{"1" + f, "1" + f},
		{"10." + f, "10." + f},
		{f + ".5", f + ".5"},
		{f + "a", f + "a"},
	} {
		if got := Restore(c.s, false); got != c.want {
			t.Errorf("Restore(%q) = %q, want %q", c.s, got, c.want)
		}
	}

	// streamed: the tool call's arguments cut at every place in the address
	whole := `{"command":"ssh root@` + f + ` 'uname -a'"}`
	for cut := strings.Index(whole, f); cut <= strings.Index(whole, f)+len(f); cut++ {
		ev := func(args string) string {
			b, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{"index": 0, "delta": map[string]any{
				"tool_calls": []any{map[string]any{"index": 0, "function": map[string]any{"arguments": args}}}}}}})
			return "data: " + string(b) + "\n\n"
		}
		rec := httptest.NewRecorder()
		rec.Header().Set("Content-Type", "text/event-stream")
		w := NewWriter(rec)
		w.WriteHeader(200)
		for _, e := range []string{ev(whole[:cut]), ev(whole[cut:]), "data: [DONE]\n\n"} {
			w.Write([]byte(e))
		}
		w.Finish()
		var args strings.Builder
		for _, e := range strings.Split(rec.Body.String(), "\n\n") {
			d, ok := strings.CutPrefix(e, "data: ")
			if !ok || d == "[DONE]" {
				continue
			}
			var x struct {
				Choices []struct {
					Delta struct {
						ToolCalls []struct{ Function struct{ Arguments string } } `json:"tool_calls"`
					}
				}
			}
			if err := json.Unmarshal([]byte(d), &x); err != nil {
				t.Fatalf("event %q: %v", d, err)
			}
			for _, c := range x.Choices[0].Delta.ToolCalls {
				args.WriteString(c.Function.Arguments)
			}
		}
		if want := strings.Replace(whole, f, ip, 1); args.String() != want {
			t.Errorf("cut at %d: arguments %s, want %s", cut, args.String(), want)
		}
	}
}

// An IPv6 address goes as one in 2001:db8::/32, and comes back.
func TestIPv6GoesAsADocumentationAddress(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	const ip = "2400:cb00:2048:1::c629:d7a2"
	o := Options{Personal: true, Kinds: map[string]bool{"ip": true}}
	out, n := Mask("ssh root@"+ip, o)
	f := strings.TrimPrefix(out, "ssh root@")
	a, err := netip.ParseAddr(f)
	if n != 1 || err != nil || !netip.MustParsePrefix("2001:db8::/32").Contains(a) {
		t.Fatalf("sent %q", out)
	}
	if back := Restore(`{"command":"ping6 `+f+`"}`, true); back != `{"command":"ping6 `+ip+`"}` {
		t.Errorf("put back as %s", back)
	}
}

// Two addresses never share a stand-in: past the 462 IPv4 ones, an
// address goes as its placeholder, and every one comes back as itself.
func TestIPStandInsNeverShared(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	o := Options{Personal: true, Kinds: map[string]bool{"ip": true}}
	seen := map[string]string{}
	const total = len(ipNets)*ipHosts + 10
	for i := 0; i < total; i++ {
		ip := fmt.Sprintf("47.96.%d.%d", i/200, 1+i%200)
		out, n := Mask("host "+ip, o)
		f := strings.TrimPrefix(out, "host ")
		if n != 1 {
			t.Fatalf("%s: sent %q", ip, out)
		}
		if i < len(ipNets)*ipHosts && !docAddr(f) || i >= len(ipNets)*ipHosts && !strings.HasPrefix(f, "{{IP_") {
			t.Fatalf("%s (#%d) went as %q", ip, i, f)
		}
		if other, ok := seen[f]; ok {
			t.Fatalf("%s and %s both went as %s", other, ip, f)
		}
		seen[f] = ip
		if again, _ := Mask("host "+ip, o); again != out {
			t.Fatalf("%s: %q then %q", ip, out, again)
		}
	}
	for f, ip := range seen {
		if back := Restore("ssh "+f, false); back != "ssh "+ip {
			t.Fatalf("%s put back as %q, want %s", f, back, ip)
		}
	}
}

// An EVM wallet's or contract's address is public, and goes as written
// whatever the local model takes it for: masked, the vendor's model took
// the placeholder for a private key and told the user to rotate it (0x0_air
// on X, 10-28). A private key, 64 hex digits, is masked still, and so is
// 0x and 40 hex digits called a key.
func TestEVMAddressGoesAsWritten(t *testing.T) {
	const addr = "0x89dfBD9b4C2e7A1f3D5b6E8c0A9F2d4B7e1C3a5D"
	const pk = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
	all := map[string]bool{}
	for _, c := range Categories {
		all[c.ID] = true
	}
	for _, text := range []string{
		"挖矿收益打到这个合约地址 " + addr + " 就行，不需要签名",
		"wallet: " + addr,
		`{"to":"` + addr + `","value":"0x0"}`,
		"address is " + addr + ".",
	} {
		for _, label := range []string{"secret", "account_number"} {
			f := &fakeModel{values: map[string]string{addr: label}}
			if out, n := Mask(text, Options{Secrets: true, Personal: true, Kinds: all, Model: f.find}); n != 0 {
				t.Errorf("%q as %s: masked %q", text, label, out)
			}
		}
	}
	for _, c := range []struct{ text, value string }{
		{"私钥 " + pk + " 别外传", pk},
		{"the key is " + pk, pk},
		{"api key: " + addr, addr},
		{"token is " + addr, addr},
	} {
		f := &fakeModel{values: map[string]string{c.value: "secret"}}
		if out, _ := Mask(c.text, Options{Secrets: true, Personal: true, Model: f.find}); strings.Contains(out, c.value) {
			t.Errorf("%q: left in %q", c.value, out)
		}
	}
}
