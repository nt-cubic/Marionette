//go:build !windows

package proc

import "context"

// list asks ps, in its POSIX form, which macOS, Linux and the BSDs all
// take: every process, its id and its whole command line.
func list(ctx context.Context, _ string) ([]Process, error) {
	out, err := CommandContext(ctx, "ps", "-A", "-o", "pid=", "-o", "args=").Output()
	if err != nil {
		return nil, err
	}
	return parseList(string(out)), nil
}
