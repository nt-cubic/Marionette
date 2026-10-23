package settings

import "testing"

// How many Claude Code processes may run is sized to this machine's memory
// (#2956): a sync or backup from a big desktop never brings its number to a
// 512 MiB server, nor clears the server's own.
func TestClaudeRunsMostIsMachineLocal(t *testing.T) {
	from := Settings{ClaudeRunsMost: 12}
	from.KeepOwn(Settings{ClaudeRunsMost: 2})
	if from.ClaudeRunsMost != 2 {
		t.Fatalf("a sync brought %d Claude Code runs, this machine's own is 2", from.ClaudeRunsMost)
	}
	from = Settings{}
	from.KeepOwn(Settings{ClaudeRunsMost: 3})
	if from.ClaudeRunsMost != 3 {
		t.Fatalf("a sync cleared this machine's 3 Claude Code runs (%d)", from.ClaudeRunsMost)
	}
}
