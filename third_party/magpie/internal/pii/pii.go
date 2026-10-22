// Package pii runs Settings → Privacy's local model, which finds what the
// masking rules can't (internal/redact's Model): a name or an address in
// free text, a password with nothing around it to say so.
//
// The model is OpenAI's Privacy Filter (Apache-2.0), 1.6 GB as the q8 GGUF
// LocalAI made of it, run by pf-serve (native/privacy-filter), a program
// built from privacy-filter.cpp on ggml. Neither ships with magpie: both
// are downloaded when the user turns the model on, each checked against
// the sha256 pinned here, and the setting (settings.RedactModel) is saved
// only once both are. pf-serve is started on the first request that needs
// it, on the GPU where there is one (Metal, Vulkan) and else the CPU, and
// is ended after a while with none, which lets its memory go.
//
// A process of its own, not linked in: 1.7 GB the gateway would otherwise
// hold for good, and a GPU driver's crash that would end the gateway with
// it. One that dies is started again; one that dies on the GPU is started
// on the CPU for the rest of magpie's run.
package pii

import (
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"time"

	"github.com/yetone/magpie/internal/appdir"
)

// The model, at a revision of LocalAI's repo, so the file is always the
// one hashed here.
var (
	modelURL  = "https://huggingface.co/LocalAI-io/privacy-filter-GGUF/resolve/935d86882f4b5dc6e0eef05aae48ae5ba0a82a7a/privacy-filter-q8.gguf"
	modelSize = int64(1637572192)
	modelSHA  = "80efc1803eda7c095a79741d2008c07e2e0a57b01bac8825fbeb448fd097998c"
)

// pf-serve, as the privacy-filter workflow built it at serveTag: a zip for
// each system, by its sha256. A system with none can't run the model.
var (
	serveTag  = "pf-v1"
	serveBase = "https://github.com/yetone/magpie/releases/download/"
	serveSHA  = map[string]string{
		"darwin-amd64":  "7d5512ed24ff865bb0e84009b0adecd3addf0444891990cbc78634d78390c607",
		"darwin-arm64":  "07eb79dd1d74a6dc8e2e59eefed1094a4c1dafa27051343fac65d6c2172617f0",
		"linux-amd64":   "b172f39b2b5446cb92ef14f996b65157f5e1c0e4e673ee9c61a0214cc67c78af",
		"linux-arm64":   "7a344515e35676d0da21b448c6c4bc49de7cf34f8f28604c6889c092ae963961",
		"windows-amd64": "dd06aa4e656cbebff348ad3fdfd66eca5f899b244dff65235d47a2c878b49c6c",
	}
)

// target is this system's name in serveSHA.
func target() string { return runtime.GOOS + "-" + runtime.GOARCH }

// Supported says pf-serve is built for this system.
func Supported() bool { return serveSHA[target()] != "" }

// Size is how much turning the model on downloads.
func Size() int64 { return modelSize }

// dir is where the model and pf-serve are kept: magpie's cache, since
// either can be downloaded again, and never the settings' folder, which a
// sync carries to the user's other computers.
var dir = func() string { return filepath.Join(appdir.Cache(), "privacy-filter") }

func modelPath() string { return filepath.Join(dir(), "privacy-filter-q8-"+modelSHA[:12]+".gguf") }

func serveDir() string { return filepath.Join(dir(), serveTag+"-"+target()) }

func exe(name string) string {
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	return filepath.Join(serveDir(), name)
}

// Installed says the model and pf-serve are on disk, as downloaded and
// checked: the model the size it was hashed at.
func Installed() bool {
	fi, err := os.Stat(modelPath())
	if err != nil || fi.Size() != modelSize {
		return false
	}
	_, err = os.Stat(exe("pf-serve"))
	return err == nil
}

// ErrNotInstalled is what a request gets when the model is on in the
// settings and isn't on disk (magpie's cache was cleared).
var ErrNotInstalled = errors.New("the local masking model isn't downloaded; download it again in Settings → Privacy, or turn it off")

// Status is what Settings → Privacy shows of the model.
type Status struct {
	Supported bool  `json:"supported"`
	Installed bool  `json:"installed"`
	Size      int64 `json:"size"`
	// a download under way, or the last one's error
	Downloading bool   `json:"downloading,omitempty"`
	Done        int64  `json:"done,omitempty"`
	Total       int64  `json:"total,omitempty"`
	Error       string `json:"error,omitempty"`
	// pf-serve, while it runs: where the model is, and how long it took
	// to start
	Running  bool   `json:"running,omitempty"`
	Device   string `json:"device,omitempty"`
	GPU      bool   `json:"gpu,omitempty"`
	LoadMS   int    `json:"loadMs,omitempty"`
	GPUError string `json:"gpuError,omitempty"`
	// why the last request got no answer
	Failed string `json:"failed,omitempty"`
}

// Now is the model's status.
func Now() Status {
	s := Status{Supported: Supported(), Installed: Installed(), Size: modelSize}
	dl.mu.Lock()
	s.Downloading, s.Error = dl.cancel != nil, dl.err
	s.Done, s.Total = dl.done.Load(), dl.total
	dl.mu.Unlock()
	srv.mu.Lock()
	if srv.h != nil {
		s.Running, s.Device, s.GPU, s.LoadMS, s.GPUError = true, srv.h.hello.Device, srv.h.hello.GPU, srv.h.hello.MS, srv.h.hello.GPUError
	}
	s.Failed = srv.failed
	if s.GPUError == "" {
		s.GPUError = srv.gpuLost
	}
	srv.mu.Unlock()
	return s
}

// Remove ends pf-serve and deletes the model and pf-serve.
func Remove() error {
	Cancel()
	Stop()
	dl.mu.Lock()
	dl.err = ""
	dl.mu.Unlock()
	return os.RemoveAll(dir())
}

// idleAfter is how long pf-serve runs with no request before it is ended.
var idleAfter = 10 * time.Minute
