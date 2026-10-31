package redact

import (
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"
)

// standInRe is an address's stand-in: public-ip- and eight lowercase
// letters, no address of any kind.
var standInRe = regexp.MustCompile(`^public-ip-[a-z]{8}$`)

func isIPStandIn(f string) bool { return standInRe.MatchString(f) }

// A server's address the user gives goes as a public-ip- name, neither
// {{IP_…}} nor another address: told "my server is {{IP_…}}", models asked
// the user for the real one or searched ~/.ssh for it (0x0_air on X,
// 10-28); told a documentation address, they said it was an example and
// not the user's server, and a 192.0.2. one read as a LAN address
// (akic404 on Discord, 10-31). The model's ssh with the stand-in comes
// back with the real address, in a whole reply and a stream.
func TestIPGoesAsAPublicIPName(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	const ip = "47.96.123.45"
	o := Options{Personal: true, Kinds: map[string]bool{"ip": true}}
	msg := "我的服务器是 " + ip + "，用户 root，已经配好密钥登录。先 ssh 上去看看 uname -a 和磁盘空间。"
	out, n := Mask(msg, o)
	f := strings.TrimSuffix(strings.Fields(strings.TrimPrefix(out, "我的服务器是 "))[0], "，用户")
	if n != 1 || strings.Contains(out, ip) || strings.Contains(out, "{{") || !isIPStandIn(f) {
		t.Fatalf("sent %q (address %q), want the address as a public-ip- name", out, f)
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
		{"http://" + f + ":8080/health", "http://" + ip + ":8080/health"},
		{"`" + f + "`", "`" + ip + "`"},
		{"**" + f + "**", "**" + ip + "**"},
		{f + "，用户 root", ip + "，用户 root"},
		{"Host " + f + "\n", "Host " + ip + "\n"},
		{f + "0", f + "0"},
		{f + "a", f + "a"},
		{f + "-2", f + "-2"},
		{f + "_x", f + "_x"},
		{"my-" + f, "my-" + f},
		{"x" + f, "x" + f},
		{"db." + f, "db." + f},
		{f + ".lan", f + ".lan"},
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

// An IPv6 address goes as a public-ip- name too, and comes back, in a
// URL's brackets as well.
func TestIPv6GoesAsAPublicIPName(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	const ip = "2400:cb00:2048:1::c629:d7a2"
	o := Options{Personal: true, Kinds: map[string]bool{"ip": true}}
	out, n := Mask("ssh root@"+ip, o)
	f := strings.TrimPrefix(out, "ssh root@")
	if n != 1 || !isIPStandIn(f) {
		t.Fatalf("sent %q", out)
	}
	if back := Restore(`{"command":"ping6 `+f+`"}`, true); back != `{"command":"ping6 `+ip+`"}` {
		t.Errorf("put back as %s", back)
	}
	if back := Restore("curl http://["+f+"]:8080/", false); back != "curl http://["+ip+"]:8080/" {
		t.Errorf("put back as %s", back)
	}
}

// Two addresses never share a stand-in, every one comes back as itself,
// and a documentation address the user writes goes and comes back as
// written: it is no stand-in, so nothing turns it into another address.
func TestIPStandInsNeverShared(t *testing.T) {
	Forget()
	t.Cleanup(Forget)
	o := Options{Personal: true, Kinds: map[string]bool{"ip": true}}
	seen := map[string]string{}
	for i := 0; i < 1000; i++ {
		ip := fmt.Sprintf("47.96.%d.%d", i/200, 1+i%200)
		out, n := Mask("host "+ip, o)
		f := strings.TrimPrefix(out, "host ")
		if n != 1 || !isIPStandIn(f) {
			t.Fatalf("%s (#%d) went as %q", ip, i, out)
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
	const doc = "203.0.113.147"
	if out, n := Mask("例子里写的是 "+doc, o); n != 0 || out != "例子里写的是 "+doc {
		t.Errorf("a documentation address went as %q", out)
	}
	if back := Restore("ssh root@"+doc, false); back != "ssh root@"+doc {
		t.Errorf("a documentation address came back as %q", back)
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
