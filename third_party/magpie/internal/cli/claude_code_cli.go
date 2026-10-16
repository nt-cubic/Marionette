package cli

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/claudecode"
	"github.com/yetone/magpie/internal/proc"
)

const claudeCodeUsage = "usage: magpie claude-code                  which Claude Code the Claude subscription runs\n" +
	"       magpie claude-code install [--yes]  download Anthropic's own build of Claude Code for magpie (a server or container without one)\n" +
	"       magpie claude-code remove           delete the Claude Code magpie downloaded"

// claudeCodeCmd: magpie claude-code [install [--yes]|remove]. A Claude
// subscription is answered by Claude Code, and signed in to through it; a
// server or a container has none (Jorben on Discord), so magpie downloads
// Anthropic's build when asked, as Anthropic's installer does.
func claudeCodeCmd(args []string) error {
	if len(args) == 0 {
		return claudeCodeStatus()
	}
	switch args[0] {
	case "install":
		yes := false
		for _, a := range args[1:] {
			switch a {
			case "--yes", "-y":
				yes = true
			default:
				return fmt.Errorf("magpie claude-code install: unknown %q\n%s", a, claudeCodeUsage)
			}
		}
		return installClaudeCode(yes)
	case "remove", "rm":
		path, v := claudeCode.downloaded()
		if path == "" {
			fmt.Println(muted.Render("magpie hasn't downloaded Claude Code"))
			return nil
		}
		if err := claudecode.Remove(); err != nil {
			return err
		}
		fmt.Println(green.Render("✓"), "removed Claude Code", v, muted.Render("("+claudecode.Root()+")"))
		return nil
	case "-h", "--help", "help":
		fmt.Println(claudeCodeUsage)
		return nil
	}
	return fmt.Errorf("magpie claude-code: unknown %q\n%s", args[0], claudeCodeUsage)
}

// claudeCode is what the command reads of the machine; tests stand in for it.
var claudeCode = struct {
	own        func() string
	downloaded func() (string, string)
	latest     func(context.Context) (claudecode.Release, error)
	install    func(context.Context, claudecode.Release, func(int64)) (string, error)
}{
	own:        func() string { return proc.FindTool("claude") },
	downloaded: claudecode.Downloaded,
	latest:     claudecode.Latest,
	install:    claudecode.Install,
}

func claudeCodeStatus() error {
	own := claudeCode.own()
	path, v := claudeCode.downloaded()
	switch {
	case own != "":
		fmt.Println(green.Render("●"), "Claude Code", muted.Render(own), muted.Render("· this machine's own, which the Claude subscription runs"))
		if path != "" {
			fmt.Println(muted.Render("  magpie's download (" + v + ", " + path + ") isn't used while it is here · magpie claude-code remove deletes it"))
		}
	case path != "":
		fmt.Println(green.Render("●"), "Claude Code", v, muted.Render(path), muted.Render("· downloaded by magpie"))
		fmt.Println(muted.Render("  magpie claude-code install gets the newest · magpie claude-code remove deletes it"))
	default:
		fmt.Println(amber.Render("○"), "no Claude Code here: a Claude subscription can't be signed in to or answered")
		fmt.Println(muted.Render("  install it (https://code.claude.com/docs), or magpie claude-code install downloads Anthropic's own build for magpie"))
	}
	return nil
}

// installClaudeCode downloads the newest Claude Code after saying what,
// from where and to where, and asking (unless yes).
func installClaudeCode(yes bool) error {
	ctx, stop := interruptContext()
	defer stop()
	lctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	r, err := claudeCode.latest(lctx)
	cancel()
	if err != nil {
		return fmt.Errorf("asking for Claude Code's newest release: %w", err)
	}
	if path, v := claudeCode.downloaded(); v == r.Version {
		fmt.Println(green.Render("✓"), "Claude Code", v, "is downloaded already", muted.Render("("+path+")"))
		return nil
	}
	fmt.Println("Claude Code", bold.Render(r.Version), muted.Render("("+r.Platform+", "+mb(r.Size)+")"))
	fmt.Println(muted.Render("  from  " + r.URL))
	fmt.Println(muted.Render("  to    " + claudecode.Root()))
	fmt.Println(muted.Render("  Anthropic's own build, as its installer downloads it; its SHA-256 is checked against Anthropic's manifest"))
	fmt.Println(muted.Render("  it is Anthropic's software, under Anthropic's terms; magpie runs it for the Claude subscription only"))
	if own := claudeCode.own(); own != "" {
		fmt.Println(amber.Render("!"), "this machine has a Claude Code of its own,", own+": it is the one run while it is there")
	}
	if !yes {
		fmt.Print("Download it? [y/N] ")
		var answer string
		fmt.Scanln(&answer)
		if a := strings.ToLower(strings.TrimSpace(answer)); a != "y" && a != "yes" {
			return fmt.Errorf("download canceled")
		}
	}
	last := time.Time{}
	tty := isTerminal(os.Stdout)
	path, err := claudeCode.install(ctx, r, func(done int64) {
		if !tty || time.Since(last) < 200*time.Millisecond && done < r.Size {
			return
		}
		last = time.Now()
		fmt.Printf("\r  %s of %s", mb(done), mb(r.Size))
	})
	if tty {
		fmt.Println()
	}
	if err != nil {
		return fmt.Errorf("downloading Claude Code %s: %w", r.Version, err)
	}
	fmt.Println(green.Render("✓"), "Claude Code", r.Version, "downloaded and checked", muted.Render("("+path+")"))
	fmt.Println(muted.Render("  add a Claude account: magpie accounts add claude"))
	return nil
}

func mb(n int64) string { return fmt.Sprintf("%.0f MB", float64(n)/(1<<20)) }

func isTerminal(f *os.File) bool {
	st, err := f.Stat()
	return err == nil && st.Mode()&os.ModeCharDevice != 0
}
