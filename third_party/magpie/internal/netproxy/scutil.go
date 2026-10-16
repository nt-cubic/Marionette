package netproxy

import (
	"net"
	"strings"
)

// parseScutil reads `scutil --proxy`. HTTPS wins over HTTP over SOCKS for
// URL; HTTP and SOCKS are each entry alone.
func parseScutil(out string) Proxy {
	kv := map[string]string{}
	var bypass []string
	inList := false
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if inList {
			if line == "}" {
				inList = false
			} else if _, v, ok := strings.Cut(line, " : "); ok {
				bypass = append(bypass, v)
			}
			continue
		}
		k, v, ok := strings.Cut(line, " : ")
		if !ok {
			continue
		}
		if k == "ExceptionsList" {
			inList = true
			continue
		}
		kv[k] = v
	}
	entry := func(key, scheme string) string {
		if kv[key+"Enable"] != "1" || kv[key+"Proxy"] == "" {
			return ""
		}
		host := kv[key+"Proxy"]
		if port := kv[key+"Port"]; port != "" {
			host = net.JoinHostPort(host, port)
		}
		return scheme + "://" + host
	}
	p := Proxy{HTTP: entry("HTTP", "http"), SOCKS: entry("SOCKS", "socks5")}
	for _, u := range []string{entry("HTTPS", "http"), p.HTTP, p.SOCKS} {
		if u != "" {
			p.URL, p.Bypass = u, bypass
			return p
		}
	}
	return Proxy{}
}
