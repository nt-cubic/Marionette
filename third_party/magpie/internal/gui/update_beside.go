package gui

import (
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/update"
)

// restartBeside answers the gateway's RestartPath: `magpie update`, run in
// a terminal (gaubeebangeel on X: an agent this gateway serves, at 4am),
// put version in where this magpie runs from. Before, the app ran on as
// the old version until someone quit and reopened it, which cut what the
// agents had in flight. It now restarts into it as Settings' AutoRestart
// does: at once when the restart cuts nothing short (on a Mac or Linux it
// hands the gateway over, so the agent's own requests go on), else once
// that is done (waitIdle). A magpie that wasn't the one replaced (another
// copy) stays as it is.
func (u *updater) restartBeside(version string) gateway.RestartReply {
	u.mu.Lock()
	restart := u.autoRestart
	in := u.replaced()
	if in {
		// what the terminal put in is the version restarted into, as if
		// this one had downloaded it: the feed isn't asked first
		if version != "" && (u.latest == nil || update.Newer(version, u.latest.Version)) {
			u.latest, u.notesIn = &update.Release{Version: version}, ""
		}
		if u.state != "checking" && u.state != "downloading" {
			u.state = "ready"
		}
	}
	u.mu.Unlock()
	switch {
	case !in:
		return gateway.RestartReply{State: "current"}
	case restart == nil:
		return gateway.RestartReply{State: "unsupported"}
	}
	reply := gateway.RestartReply{Handover: handsOver()}
	if waiting, _ := u.waitingFor(); !waiting && u.idle() {
		reply.State = "restarting"
		go restart() // after the answer: the restart lets this request finish
		return reply
	}
	u.waitIdle(restart, false)
	b := restartBusy()
	reply.State, reply.Busy = "waiting", &b
	return reply
}
