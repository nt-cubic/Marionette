package settings

import (
	"fmt"
	"strings"

	"github.com/yetone/magpie/internal/redact"
)

// How many values the masking allowlist may hold, and how long each.
const (
	maxAllow    = 200
	maxAllowLen = 256
)

// redactActions is m with only the actions redact knows, each for a kind
// named as a placeholder names one (API_KEY, a rule's own); masking, the
// default, is left out.
func redactActions(m map[string]string) (map[string]string, error) {
	out := map[string]string{}
	for k, a := range m {
		k, a = strings.TrimSpace(k), strings.ToLower(strings.TrimSpace(a))
		switch a {
		case "", "mask":
			continue
		case redact.Warn, redact.Block:
		default:
			return nil, fmt.Errorf("what is done with a kind of value found must be mask, %s or %s, not %q", redact.Warn, redact.Block, a)
		}
		if !redact.KindName(k) {
			return nil, fmt.Errorf("%q isn't a kind of value masking finds", k)
		}
		out[k] = a
	}
	if len(out) == 0 {
		return nil, nil
	}
	return out, nil
}

// redactAllow is the allowlist trimmed, each value once whatever its case.
func redactAllow(vs []string) ([]string, error) {
	var out []string
	seen := map[string]bool{}
	for _, v := range vs {
		v = strings.TrimSpace(v)
		if v == "" || seen[strings.ToLower(v)] {
			continue
		}
		if len(v) > maxAllowLen {
			return nil, fmt.Errorf("a value the masking lets go is at most %d characters", maxAllowLen)
		}
		seen[strings.ToLower(v)] = true
		out = append(out, v)
	}
	if len(out) > maxAllow {
		return nil, fmt.Errorf("the masking lets at most %d values go", maxAllow)
	}
	return out, nil
}
