package claudetui

// Command is the hidden subcommand that runs it.
const Command = "claude-tui-helper"

// TrustEnv, "1", has the shim tell Claude Code yes when it asks whether
// to trust the folder it runs in: the bridge sets it for magpie's own
// folder, and for a caller's in an account folder of magpie's, whose
// Claude Code no one else runs. Otherwise the question is the user's to
// answer in their own Claude Code, and the run is -p.
const TrustEnv = "MAGPIE_CLAUDE_TUI_TRUST"

// WSLEnv holds, as JSON (WSL), how to run a Claude Code that is in a WSL
// distro: the helper, on Windows, runs wsl.exe in its pseudo console.
const WSLEnv = "MAGPIE_CLAUDE_TUI_WSL"

// WSL is a Claude Code in a WSL distro, as the helper runs it.
type WSL struct {
	// Args are wsl.exe's arguments up to Claude Code's own, its path last
	Args []string `json:"args"`
	// Mount is where the distro mounts Windows' drives ("/mnt/"): an
	// image's path is pasted as Claude Code there reads it
	Mount string `json:"mount"`
	// Projects is Claude Code's projects folder, as Windows reads it
	Projects string `json:"projects"`
}
