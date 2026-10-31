//go:build !windows

package masking

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/redact"
)

// TestLiveHistoryCost measures what the default's local model (AstrLink
// Guard) costs on a long Codex history: CPU seconds and wall time per
// request, background work included. It needs the model's files, in the
// folder MAGPIE_GUARD_FILES names (model_int8.onnx, model_int8.onnx_data,
// tokenizer.json), and is skipped without; nothing is downloaded.
func TestLiveHistoryCost(t *testing.T) {
	src := os.Getenv("MAGPIE_GUARD_FILES")
	if src == "" {
		t.Skip("MAGPIE_GUARD_FILES names no folder with AstrLink Guard's files")
	}
	// testenv's sandbox: the model's folder there, never the user's
	if os.Getenv("XDG_CACHE_HOME") == "" {
		t.Fatal("no sandbox XDG_CACHE_HOME")
	}
	dst := filepath.Join(os.Getenv("XDG_CACHE_HOME"), "magpie", "astrlink-guard", "7216b6a3b897")
	if err := os.MkdirAll(dst, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{"model_int8.onnx", "model_int8.onnx_data", "tokenizer.json"} {
		if err := os.Symlink(filepath.Join(src, f), filepath.Join(dst, f)); err != nil {
			t.Fatal(err)
		}
	}
	m := pii.Of(pii.AstrLinkGuard)
	if !m.Installed() {
		t.Fatal("the model's files aren't where pii looks")
	}
	// load it (and compile CoreML on a Mac) before measuring
	if _, err := m.Find(context.Background(), []string{"warm the model up"}); err != nil {
		t.Fatal(err)
	}
	turns := liveTurns(t, 230)
	mask := func(n int) (wall, cpu time.Duration) {
		body := codexBody(turns[:n])
		o := redact.Options{Secrets: true, Personal: true, MinScore: m.MinScore()}
		o.Model = model(context.Background(), m, o.MinScore, false)
		c0, w0 := cpuNow(), time.Now()
		redact.MaskJSON(body, o)
		return time.Since(w0), cpuNow() - c0
	}
	// the rules alone, for the same bodies
	rules := func(n int) time.Duration {
		body := codexBody(turns[:n])
		c0 := cpuNow()
		redact.MaskJSON(body, redact.Options{Secrets: true, Personal: true})
		return cpuNow() - c0
	}
	fmt.Printf("history of 200 turns: %d bytes; rules alone %v CPU\n", len(codexBody(turns[:200])), rules(200).Round(time.Millisecond))
	// A Codex session resumed on a 200-turn history the model hasn't
	// read: 30 requests, a turn more each, 3s apart (the vendor's
	// answer), then what runs on after the last.
	var wallAll []time.Duration
	c0, w0 := cpuNow(), time.Now()
	for i := 0; i < 30; i++ {
		start := time.Now()
		wall, cpu := mask(200 + i)
		wallAll = append(wallAll, wall)
		fmt.Printf("request %2d: wall %6v  cpu %6v\n", i+1, wall.Round(time.Millisecond), cpu.Round(time.Millisecond))
		time.Sleep(3*time.Second - time.Since(start)%(3*time.Second))
	}
	session := cpuNow() - c0
	// what runs on once no request comes
	c1 := cpuNow()
	time.Sleep(20 * time.Second)
	after := cpuNow() - c1
	sort.Slice(wallAll, func(i, j int) bool { return wallAll[i] < wallAll[j] })
	fmt.Printf("session: %v wall, %v CPU (%.0f%% of a core); 20s after it: %v CPU; request wall median %v max %v\n",
		time.Since(w0).Round(time.Second)-20*time.Second, session.Round(time.Millisecond),
		100*session.Seconds()/(time.Since(w0)-20*time.Second).Seconds(), after.Round(time.Millisecond),
		wallAll[len(wallAll)/2].Round(time.Millisecond), wallAll[len(wallAll)-1].Round(time.Millisecond))
	// warm: the whole history read (as far as it will be), a turn more
	full := redact.Options{Secrets: true, Personal: true, MinScore: m.MinScore()}
	full.Model = model(context.Background(), m, full.MinScore, true)
	redact.MaskJSON(codexBody(turns[:229]), full)
	wall, cpu := mask(230)
	fmt.Printf("warm, one turn more: wall %v cpu %v\n", wall.Round(time.Millisecond), cpu.Round(time.Millisecond))
}

func cpuNow() time.Duration {
	var ru syscall.Rusage
	syscall.Getrusage(syscall.RUSAGE_SELF, &ru)
	return time.Duration(ru.Utime.Nano() + ru.Stime.Nano())
}

type liveTurn struct {
	user, cmd, output, answer string
}

// liveTurns are n turns of an agent at work on this repo: its own files
// as tool output, its docs as the answers, a person or two named.
func liveTurns(t *testing.T, n int) []liveTurn {
	root := "../.."
	var files []string
	filepath.Walk(filepath.Join(root, "internal"), func(p string, fi os.FileInfo, err error) error {
		if err == nil && !fi.IsDir() && strings.HasSuffix(p, ".go") && fi.Size() > 2000 && fi.Size() < 40000 {
			files = append(files, p)
		}
		return nil
	})
	sort.Strings(files)
	doc, err := os.ReadFile(filepath.Join(root, "docs", "subsystems", "gateway-routing.md"))
	if err != nil {
		t.Fatal(err)
	}
	paras := strings.Split(string(doc), "\n\n")
	people := []string{"张伟", "Alice Martin", "陈建国", "Bob Chen"}
	var out []liveTurn
	for i := 0; i < n; i++ {
		f := files[(i*7)%len(files)]
		b, _ := os.ReadFile(f)
		if len(b) > 12000 {
			b = b[:12000]
		}
		rel, _ := filepath.Rel(root, f)
		tu := liveTurn{cmd: fmt.Sprintf(`{"command":["bash","-lc","sed -n 1,400p %s"],"workdir":"/home/dev/magpie"}`, rel),
			output: string(b), answer: paras[i%len(paras)]}
		if len(tu.answer) > 3000 {
			tu.answer = tu.answer[:3000]
		}
		if i%10 == 0 {
			tu.user = fmt.Sprintf("请看一下 %s，%s 说那里有个问题，邮件发给 %s 了。", rel, people[i%len(people)], "bob.chen@example.org")
		}
		out = append(out, tu)
	}
	return out
}

// codexBody is a Responses request with turns as its history.
func codexBody(turns []liveTurn) []byte {
	var in []any
	in = append(in, map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "<environment_context>\n  <cwd>/home/dev/magpie</cwd>\n  <shell>bash</shell>\n</environment_context>"}}})
	for i, tu := range turns {
		if tu.user != "" {
			in = append(in, map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": tu.user}}})
		}
		id := fmt.Sprintf("call_%024d", i)
		in = append(in,
			map[string]any{"type": "reasoning", "summary": []any{}, "encrypted_content": strings.Repeat("gAAAAB", 200)},
			map[string]any{"type": "function_call", "name": "shell", "arguments": tu.cmd, "call_id": id},
			map[string]any{"type": "function_call_output", "call_id": id, "output": tu.output},
			map[string]any{"type": "message", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": tu.answer}}})
	}
	b, _ := json.Marshal(map[string]any{"model": "gpt-5-codex", "instructions": strings.Repeat("You are Codex, a coding agent. Follow the user's instructions. ", 200),
		"input": in, "tools": []any{map[string]any{"type": "function", "name": "shell", "description": "Runs a shell command", "parameters": map[string]any{"type": "object"}}},
		"stream": true})
	return b
}
