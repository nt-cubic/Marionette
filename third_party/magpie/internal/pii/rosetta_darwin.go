package pii

import (
	"sync"

	"golang.org/x/sys/unix"
)

// translated says magpie runs under Rosetta: an Intel build on Apple
// silicon.
var translated = sync.OnceValue(func() bool {
	v, err := unix.SysctlUint32("sysctl.proc_translated")
	return err == nil && v == 1
})
