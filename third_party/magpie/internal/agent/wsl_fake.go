package agent

import (
	"errors"
	"strings"
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
	fakeStop = func(name string) {
		wsl.Lock()
		stopped[name] = true
		wsl.runAt = time.Time{}
		wsl.Unlock()
	}
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

var fakeStop func(string)

// StopFakeWSL, for other packages' tests, has FakeWSL's distro name stop,
// as the user stopping it (wsl --shutdown) does: wsl.exe lists it as not
// running from now on.
func StopFakeWSL(name string) {
	if fakeStop != nil {
		fakeStop(name)
	}
}
