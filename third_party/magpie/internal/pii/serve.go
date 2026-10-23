package pii

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/proc"
	"github.com/yetone/magpie/internal/redact"
)

// hello is the line pf-serve writes once the model is loaded.
type hello struct {
	Device   string `json:"device"`
	GPU      bool   `json:"gpu"`
	MS       int    `json:"ms"`
	GPUError string `json:"gpuError"`
	Error    string `json:"error"`
}

// helper is one pf-serve running.
type helper struct {
	cmd    *exec.Cmd
	in     io.WriteCloser
	out    *bufio.Reader
	stderr *tail
	hello  hello
	exited chan struct{}
}

// srv is the pf-serve magpie runs, one at a time: its requests take
// turns.
var srv struct {
	mu   sync.Mutex
	h    *helper
	idle *time.Timer
	// cpu is set once pf-serve died on the GPU: from then on it runs on
	// the CPU; gpuLost says why
	cpu     bool
	gpuLost string
	// failed is why the last request got no answer, until one does
	failed string
}

// loadTimeout is how long pf-serve has to load the model: 1.6 GB read
// from a slow disk, and the GPU's kernels built.
var loadTimeout = 3 * time.Minute

// classifyTimeout is how long one request may take before pf-serve is
// taken for hung and ended: a second for each 4 KB on the slowest CPU,
// and 30 more.
var classifyTimeout = func(bytes int) time.Duration {
	return 30*time.Second + time.Duration(bytes/4096)*time.Second
}

// candidates are the programs to start, in turn: pf-serve-vulkan, where
// there is one, needs the Vulkan loader a GPU's driver installs and
// can't start without it; pf-serve runs anywhere.
func candidates(cpu bool) []string {
	var out []string
	if !cpu {
		if _, err := os.Stat(exe("pf-serve-vulkan")); err == nil {
			out = append(out, exe("pf-serve-vulkan"))
		}
	}
	return append(out, exe("pf-serve"))
}

// start runs pf-serve, the first of candidates that loads the model.
func start(cpu bool) (*helper, error) {
	if !Installed() {
		return nil, ErrNotInstalled
	}
	var errs []error
	for _, path := range candidates(cpu) {
		h, err := startOne(path, cpu)
		if err == nil {
			return h, nil
		}
		errs = append(errs, err)
	}
	return nil, fmt.Errorf("the local masking model didn't start: %w", errors.Join(errs...))
}

func startOne(path string, cpu bool) (*helper, error) {
	cmd := proc.Command(path, modelPath())
	cmd.Env = os.Environ()
	if cpu {
		cmd.Env = append(cmd.Env, "PF_DEVICE=cpu")
	}
	in, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	h := &helper{cmd: cmd, in: in, out: bufio.NewReaderSize(out, 64<<10), stderr: &tail{}, exited: make(chan struct{})}
	cmd.Stderr = h.stderr
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	go func() {
		cmd.Wait()
		close(h.exited)
	}()
	line := make(chan string, 1)
	go func() {
		s, _ := h.out.ReadString('\n')
		line <- s
	}()
	var s string
	select {
	case s = <-line:
	case <-time.After(loadTimeout):
		h.kill()
		return nil, fmt.Errorf("%s: the model didn't load in %s", baseName(path), loadTimeout)
	}
	if err := json.Unmarshal([]byte(s), &h.hello); err != nil || h.hello.Error != "" || h.hello.Device == "" {
		h.kill()
		why := h.hello.Error
		if why == "" {
			why = h.died()
		}
		return nil, fmt.Errorf("%s: %s", baseName(path), why)
	}
	return h, nil
}

func baseName(path string) string {
	path = strings.ReplaceAll(path, "\\", "/")
	return path[strings.LastIndexByte(path, '/')+1:]
}

// died says why pf-serve ended: what it wrote to stderr last, or how it
// exited.
func (h *helper) died() string {
	select {
	case <-h.exited:
	case <-time.After(2 * time.Second):
	}
	if t := strings.TrimSpace(h.stderr.String()); t != "" {
		return t
	}
	if st := h.cmd.ProcessState; st != nil {
		return st.String()
	}
	return "it ended"
}

func (h *helper) kill() {
	h.in.Close()
	if h.cmd.Process != nil {
		h.cmd.Process.Kill()
	}
	<-h.exited
}

// Warm starts pf-serve in the background, where the model is on disk, so
// that the first request doesn't wait for it to load, and Settings shows
// where it runs.
func Warm() {
	if Installed() {
		go classify([]string{"warm"})
	}
}

// Stop ends pf-serve, if it runs.
func Stop() {
	srv.mu.Lock()
	defer srv.mu.Unlock()
	stopLocked()
}

func stopLocked() {
	if srv.idle != nil {
		srv.idle.Stop()
		srv.idle = nil
	}
	if h := srv.h; h != nil {
		srv.h = nil
		// stdin ending ends it; one that doesn't is killed
		h.in.Close()
		select {
		case <-h.exited:
		case <-time.After(5 * time.Second):
			h.kill()
		}
	}
}

// classify asks pf-serve, started if it isn't, what is in texts. One that
// fails is ended and started again; one that failed on the GPU is started
// on the CPU, for good.
func classify(texts []string) ([][]redact.Found, error) {
	srv.mu.Lock()
	defer srv.mu.Unlock()
	out, err := classifyLocked(texts)
	srv.failed = ""
	if err != nil {
		srv.failed = err.Error()
	}
	return out, err
}

func classifyLocked(texts []string) ([][]redact.Found, error) {
	var last error
	for try := 0; try < 2; try++ {
		if srv.h == nil {
			h, err := start(srv.cpu)
			if err != nil {
				return nil, err
			}
			srv.h = h
		}
		out, err := srv.h.classify(texts)
		if err == nil {
			if srv.idle != nil {
				srv.idle.Stop()
			}
			h := srv.h
			srv.idle = time.AfterFunc(idleAfter, func() {
				srv.mu.Lock()
				defer srv.mu.Unlock()
				if srv.h == h {
					stopLocked()
				}
			})
			return out, nil
		}
		last = err
		gpu := srv.h.hello.GPU
		srv.h.kill()
		srv.h = nil
		if gpu {
			srv.cpu, srv.gpuLost = true, err.Error()
		}
	}
	return nil, fmt.Errorf("the local masking model failed: %w", last)
}

// classify is one request: n, then each text's length and bytes; the
// reply is a status, then each text's spans (pf-serve.cpp says how).
func (h *helper) classify(texts []string) ([][]redact.Found, error) {
	var b bytes.Buffer
	size := 0
	put := func(v uint32) { binary.Write(&b, binary.LittleEndian, v) }
	put(uint32(len(texts)))
	for _, t := range texts {
		put(uint32(len(t)))
		b.WriteString(t)
		size += len(t)
	}
	done, timeout := make(chan struct{}), classifyTimeout(size)
	defer close(done)
	go func() {
		select {
		case <-done:
		case <-time.After(timeout):
			// a hung pf-serve: killed, its pipe ends the read below
			h.cmd.Process.Kill()
		}
	}()
	if _, err := h.in.Write(b.Bytes()); err != nil {
		return nil, fmt.Errorf("%v (%s)", err, h.died())
	}
	r := &reader{r: h.out}
	status := r.u32()
	if status == 1 {
		msg := r.bytes(int(r.u32()))
		if r.err != nil {
			return nil, r.fail(h)
		}
		return nil, errors.New(string(msg))
	}
	out := make([][]redact.Found, len(texts))
	for i, t := range texts {
		n := int(r.u32())
		if r.err == nil && n > len(t)+1 {
			r.err = fmt.Errorf("%d spans in %d bytes", n, len(t))
		}
		for j := 0; j < n && r.err == nil; j++ {
			start, end := int32(r.u32()), int32(r.u32())
			r.u32() // the score: pf-serve sends only those over its threshold
			label := string(r.bytes(int(r.u8())))
			out[i] = append(out[i], redact.Found{Start: int(start), End: int(end), Label: label})
		}
	}
	if status != 0 && r.err == nil {
		r.err = fmt.Errorf("status %d", status)
	}
	if r.err != nil {
		return nil, r.fail(h)
	}
	return out, nil
}

type reader struct {
	r   *bufio.Reader
	err error
	b   [4]byte
}

func (r *reader) u32() uint32 {
	if r.err == nil {
		_, r.err = io.ReadFull(r.r, r.b[:4])
	}
	if r.err != nil {
		return 0
	}
	return binary.LittleEndian.Uint32(r.b[:4])
}

func (r *reader) u8() byte {
	if r.err == nil {
		var c byte
		c, r.err = r.r.ReadByte()
		return c
	}
	return 0
}

func (r *reader) bytes(n int) []byte {
	if r.err != nil {
		return nil
	}
	b := make([]byte, n)
	_, r.err = io.ReadFull(r.r, b)
	return b
}

func (r *reader) fail(h *helper) error {
	if errors.Is(r.err, io.EOF) || errors.Is(r.err, io.ErrUnexpectedEOF) {
		return fmt.Errorf("pf-serve ended: %s", h.died())
	}
	return r.err
}

// tail keeps the last 4 KB written to it.
type tail struct {
	mu sync.Mutex
	b  []byte
}

func (t *tail) Write(p []byte) (int, error) {
	t.mu.Lock()
	t.b = append(t.b, p...)
	if len(t.b) > 4096 {
		t.b = t.b[len(t.b)-4096:]
	}
	t.mu.Unlock()
	return len(p), nil
}

func (t *tail) String() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return string(t.b)
}
