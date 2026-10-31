package agent

import (
	"errors"
	"strings"
	"sync"
	"time"
)

// FakeWSL, for other packages' tests, stands in for wsl.exe on any
// system: each distro named in probes is installed and running, answers
// the probe with what probes has for it (wslProbeScript's output: "home:/
// home/me\ndir:.codex\n…") and is opened at roots' folder for it. What
// magpie remembers of WSL is forgotten, before and after; the returned
// func puts everything back.
func FakeWSL(probes, roots map[string]string) func() {
	on, run, root := wslOn, wslRun, wslRoot
	reset := func() {
		wsl.Lock()
		wsl.seen, wsl.at, wsl.runAt, wsl.names, wsl.running, wsl.probed, wsl.failed, wsl.dirty = nil, time.Time{}, time.Time{}, nil, nil, nil, nil, false
		wsl.Unlock()
	}
	reset()
	var names []string
	for n := range probes {
		names = append(names, n)
	}
	list := []byte(strings.Join(names, "\n"))
	stopped := map[string]bool{}
	fakeStop = func(name string, stop bool) {
		wsl.Lock()
		stopped[name] = stop
		wsl.runAt = time.Time{}
		wsl.Unlock()
	}
	fakeCalls.Lock()
	fakeCalls.in = map[string]int{}
	fakeCalls.Unlock()
	wslOn = true
	wslRun = func(_ time.Duration, args ...string) ([]byte, error) {
		switch strings.Join(args, " ") {
		case "-l -q":
			return list, nil
		case "-l --running -q":
			var run []string
			for _, n := range names {
				if !stopped[n] {
					run = append(run, n)
				}
			}
			return []byte(strings.Join(run, "\n")), nil
		}
		if len(args) > 2 && args[0] == "-d" {
			fakeCalls.Lock()
			fakeCalls.in[args[1]]++
			fakeCalls.Unlock()
			if p, ok := probes[args[1]]; ok {
				return []byte(p), nil
			}
		}
		return nil, errors.New("unexpected wsl.exe " + strings.Join(args, " "))
	}
	wslRoot = func(name string) string { return roots[name] }
	return func() {
		wslOn, wslRun, wslRoot = on, run, root
		fakeStop = nil
		reset()
	}
}

var fakeStop func(string, bool)

// fakeCalls counts, for each of FakeWSL's distros, the commands run in it
// (wsl.exe -d name …): each one would start a stopped distro.
var fakeCalls struct {
	sync.Mutex
	in map[string]int
}

// StopFakeWSL, for other packages' tests, has FakeWSL's distro name stop,
// as the user stopping it (wsl --shutdown) does: wsl.exe lists it as not
// running from now on.
func StopFakeWSL(name string) {
	if fakeStop != nil {
		fakeStop(name, true)
	}
}

// StartFakeWSL has a distro StopFakeWSL stopped run again, as the user
// opening a WSL shell does.
func StartFakeWSL(name string) {
	if fakeStop != nil {
		fakeStop(name, false)
	}
}

// FakeWSLRuns is how many commands magpie ran in FakeWSL's distro name
// (wsl.exe -d name …), each of which starts a stopped distro.
func FakeWSLRuns(name string) int {
	fakeCalls.Lock()
	defer fakeCalls.Unlock()
	return fakeCalls.in[name]
}
