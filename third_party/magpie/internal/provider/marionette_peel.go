package provider

import "strings"

// peelAgentModelMark takes Claude Code's [1m] window mark and Codex ACP's
// [effort] suffix (gpt-5.6-luna[xhigh]) off a model id so GroupFor still
// matches a group saved under the bare name. Stock magpie only stripped
// [1m]; Codex 2.x encodes reasoning effort in brackets, which otherwise
// 404s a Marionette share alias.
//
// Copied into internal/provider by tools/import-magpie.ps1 after every
// upstream import (the vendor tree is wiped).
func peelAgentModelMark(id string) string {
	id = strings.TrimSpace(id)
	id = strings.TrimSuffix(id, "[1m]")
	if i := strings.LastIndexByte(id, '['); i > 0 && strings.HasSuffix(id, "]") {
		inner := strings.ToLower(id[i+1 : len(id)-1])
		for _, e := range MemberEfforts {
			if e == inner {
				return id[:i]
			}
		}
	}
	return id
}
