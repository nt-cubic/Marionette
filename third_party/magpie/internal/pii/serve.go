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
	"sync/atomic"
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

// srv is the pf-serve magpie runs, one at a time. Its requests take
// turns on its one pipe: turn is held for each, from start to answer, and
// a request's caller can stop waiting for it (FindContext). mu guards
// only the fields below, and is never held while pf-serve loads or
// answers: the status (Now), Stop and the idle timer took it, and on a
// cold start every request and the Settings page waited minutes behind
// one load (#3682).
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
	// loading is set while pf-serve loads the model
	loading bool
	// stop is closed by Stop: a load under way gives up, and the request
	// it was for fails rather than start pf-serve again
	stop chan struct{}
}

// turn is pf-serve's pipe, held by the one request on it.
var turn = make(chan struct{}, 1)

// waiting counts the requests waiting for their turn.
var waiting atomic.Int32

// stopCh is srv.stop, made the first time; mu is held.
func stopCh() chan struct{} {
	if srv.stop == nil {
		srv.stop = make(chan struct{})
	}
	return srv.stop
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
// stop ends a load under way.
func start(cpu bool, stop <-chan struct{}) (*helper, error) {
	if !Installed() {
		return nil, ErrNotInstalled
	}
	upgrades.Add(1)
	go func() {
		defer upgrades.Done()
		upgrade()
	}()
	var errs []error
	for _, path := range candidates(cpu) {
		h, err := startOne(path, cpu, stop)
		if err == nil {
			return h, nil
		}
		if errors.Is(err, errStopped) {
			return nil, err
		}
		errs = append(errs, err)
	}
	return nil, fmt.Errorf("the local masking model didn't start: %w", errors.Join(errs...))
}

// upgrades are the upgrade checks start set going, for a test to wait for.
var upgrades sync.WaitGroup

// errStopped is a request's when Stop ended pf-serve under it.
var errStopped = errors.New("the local masking model was stopped")

func startOne(path string, cpu bool, stop <-chan struct{}) (*helper, error) {
	cmd := proc.Command(path, modelPath())
	// Without GGML_METAL_NO_RESIDENCY, ggml's Metal residency set keeps a
	// heartbeat that woke an idle pf-serve ~146 times a second for the 10
	// minutes it stays up after a request, and macOS's Energy Impact
	// counted magpie among the top for it (#3185). With it set: 0.3 a
	// second, and the first request after an idle spell takes 0.1-0.45s
	// more (51-78ms hot either way).
	cmd.Env = append(os.Environ(), "GGML_METAL_NO_RESIDENCY=1")
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
	case <-stop:
		h.kill()
		return nil, errStopped
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
		go func() {
			turn <- struct{}{}
			defer func() { <-turn }()
			classify([]string{"warm"})
		}()
	}
}

// Stop ends pf-serve, if it runs, and a load under way, at once: it
// waits for no request's turn. A request on it fails, and pf-serve is
// killed rather than given the 5s an idle one has to end.
func Stop() {
	free := false
	select {
	case turn <- struct{}{}:
		free = true
		defer func() { <-turn }()
	default:
	}
	srv.mu.Lock()
	h := detachLocked()
	if srv.stop != nil {
		close(srv.stop)
		srv.stop = nil
	}
	srv.mu.Unlock()
	if h != nil && !free {
		h.kill()
		return
	}
	end(h)
}

// stopWhenFree ends pf-serve once no request is on it, so that the next
// one starts the pf-serve on disk now (upgrade).
func stopWhenFree() {
	turn <- struct{}{}
	defer func() { <-turn }()
	srv.mu.Lock()
	h := detachLocked()
	srv.mu.Unlock()
	end(h)
}

// detachLocked takes pf-serve out of srv, for end; mu is held.
func detachLocked() *helper {
	if srv.idle != nil {
		srv.idle.Stop()
		srv.idle = nil
	}
	h := srv.h
	srv.h = nil
	return h
}

// end ends h, with no lock held: stdin ending ends it, and one that
// doesn't is killed.
func end(h *helper) {
	if h == nil {
		return
	}
	h.in.Close()
	select {
	case <-h.exited:
	case <-time.After(5 * time.Second):
		h.kill()
	}
}

// classify asks pf-serve, started if it isn't, what is in texts. One that
// fails is ended and started again; one that failed on the GPU is started
// on the CPU, for good. The caller holds turn.
func classify(texts []string) ([][]redact.Found, error) {
	out, err := classifyTurn(texts)
	srv.mu.Lock()
	srv.failed = ""
	if err != nil {
		srv.failed = err.Error()
	}
	srv.mu.Unlock()
	return out, err
}

func classifyTurn(texts []string) ([][]redact.Found, error) {
	var last error
	for try := 0; try < 2; try++ {
		srv.mu.Lock()
		h, cpu, stop := srv.h, srv.cpu, stopCh()
		if h == nil {
			srv.loading = true
		}
		srv.mu.Unlock()
		if h == nil {
			var err error
			h, err = start(cpu, stop)
			srv.mu.Lock()
			srv.loading = false
			if err == nil && stopped(stop) {
				err = errStopped
			}
			if err != nil {
				srv.mu.Unlock()
				if h != nil {
					h.kill()
				}
				return nil, err
			}
			srv.h = h
			srv.mu.Unlock()
		}
		out, err := h.classify(texts)
		srv.mu.Lock()
		if err == nil {
			if srv.h == h {
				idleLocked(h)
			}
			srv.mu.Unlock()
			return out, nil
		}
		last = err
		if srv.h == h {
			srv.h = nil
		}
		if stopped(stop) {
			srv.mu.Unlock()
			h.kill()
			return nil, errStopped
		}
		if h.hello.GPU {
			srv.cpu, srv.gpuLost = true, err.Error()
		}
		srv.mu.Unlock()
		h.kill()
	}
	return nil, fmt.Errorf("the local masking model failed: %w", last)
}

func stopped(stop <-chan struct{}) bool {
	select {
	case <-stop:
		return true
	default:
		return false
	}
}

// idleLocked ends h after idleAfter with no request; mu is held. The timer
// takes the turn only when it is free: a request on it sets the timer
// again when it is answered.
func idleLocked(h *helper) {
	if srv.idle != nil {
		srv.idle.Stop()
	}
	srv.idle = time.AfterFunc(idleAfter, func() {
		select {
		case turn <- struct{}{}:
		default:
			return
		}
		defer func() { <-turn }()
		srv.mu.Lock()
		var gone *helper
		if srv.h == h {
			gone = detachLocked()
		}
		srv.mu.Unlock()
		end(gone)
	})
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
