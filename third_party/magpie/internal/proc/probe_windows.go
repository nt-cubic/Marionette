package proc

import (
	"os/exec"
	"time"
)

// probeGroup kills the probe alone, as CommandContext does: a job, as
// StartTree gives a command, is joined only once it has started, and Output
// starts it out of reach. EndProbes ends it the same way when magpie exits,
// so what a shim started (npm's .cmd running node) is still left running.
func probeGroup(cmd *exec.Cmd, p *probe) {
	cmd.Cancel = func() error {
		p.kill()
		defer p.end()
		return cmd.Process.Kill()
	}
}

// killWait is how long EndProbes waits, past a kill's start, for it to be
// over: the probe is killed alone, and nothing is signalled again.
func killWait() time.Duration { return time.Second }
