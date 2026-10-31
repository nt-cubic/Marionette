package cli

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"golang.org/x/term"

	"github.com/yetone/magpie/internal/daemon"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/uninstall"
)

const uninstallUsage = "usage: magpie uninstall [--yes]"

// what uninstallCmd asks of the system; the tests put fakes in
var (
	servedBy      = gateway.ServedBy
	serviceStatus = daemon.Get
	runUninstall  = uninstall.Run
)

// magpie uninstall [--yes]: every agent disconnected and put back as it
// was, magpie's service, login item and shortcuts removed, then its own
// files (uninstall.Run). Deleting magpie's folders by hand left Codex
// pointed at a gateway that was gone (Jevik_AI on X). It asks first on a
// terminal; with no terminal to ask on, only --yes does it.
func uninstallCmd(args []string) error {
	yes := false
	for _, a := range args {
		switch a {
		case "--yes", "-y":
			yes = true
		default:
			return fmt.Errorf("unknown %q: nothing was changed\n%s", a, uninstallUsage)
		}
	}
	// a magpie still running would serve, and keep writing its files,
	// after its folders are gone: the service is stopped here, anything
	// else is quit first
	if s := servedBy(); s.Running && s.Pid != os.Getpid() {
		if st, err := serviceStatus(); err != nil || !st.Installed || st.PID != 0 && st.PID != s.Pid {
			how := "magpie"
			switch s.Serves {
			case "app", "tray":
				how = "magpie's app"
			case "serve", "tui", "web":
				how = "magpie " + s.Serves
			}
			pid := ""
			if s.Pid != 0 {
				pid = " (pid " + strconv.Itoa(s.Pid) + ")"
			}
			return fmt.Errorf("%s is running%s: quit it first, or uninstall from the app's Settings › Uninstall magpie. Nothing was changed", how, pid)
		}
	}
	connected := uninstall.Connected()
	remove, keep := uninstall.Folders()
	if !yes {
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			return fmt.Errorf("this disconnects every agent and deletes magpie's settings, providers and usage; run it with --yes to do so without being asked")
		}
		fmt.Println("This takes magpie off this computer:")
		if len(connected) > 0 {
			var names []string
			for _, a := range connected {
				names = append(names, a.Name)
			}
			fmt.Println("  · disconnects", strings.Join(names, ", "), "and puts back what each had (what you changed by hand since stays)")
		}
		fmt.Println("  · removes magpie's background service, opening at login and shortcuts")
		for _, p := range remove {
			fmt.Println("  · deletes", p)
		}
		if keep != "" {
			fmt.Println("  · keeps", keep, "(skills and servers your agents use)")
		}
		fmt.Print("Uninstall magpie? Your providers, keys and usage in magpie can't be brought back. [y/N] ")
		line, _ := stdin().ReadString('\n')
		if a := strings.ToLower(strings.TrimSpace(line)); a != "y" && a != "yes" {
			return fmt.Errorf("nothing was changed")
		}
	}
	r := runUninstall(context.Background())
	for _, l := range r.Says() {
		switch {
		case strings.HasPrefix(l, "✓"):
			fmt.Println(green.Render("✓") + l[len("✓"):])
		case strings.HasPrefix(l, "!"):
			fmt.Println(amber.Render("!") + l[1:])
		default:
			fmt.Println(muted.Render(l))
		}
	}
	if r.CodexApp {
		fmt.Println(amber.Render("!"), "the Codex app is still on what it read before: quit it and open it again")
	}
	if r.Exe != "" {
		fmt.Println(muted.Render("  magpie itself is left at " + r.Exe + ": delete it to finish"))
	}
	if r.Failed() {
		return fmt.Errorf("some of magpie couldn't be taken off; what is marked ! above is left")
	}
	fmt.Println(green.Render("✓"), "magpie is uninstalled")
	return nil
}

// uninstallData is `magpie uninstall --data-after <pid>`, which the app
// starts as it quits from Settings › Uninstall magpie: once the app has
// ended, its files are deleted (WebView2 holds its folder open while the
// app runs).
func uninstallData(args []string) error {
	if len(args) != 1 {
		return fmt.Errorf("usage: magpie uninstall --data-after <pid>")
	}
	// 0 or less is no process: kill(0) asks this process group, which
	// is alive as long as this runs, and the files went only at the wait's end
	pid, err := strconv.Atoi(args[0])
	if err != nil || pid <= 0 || pid == os.Getpid() {
		return fmt.Errorf("--data-after takes the app's process id, not %q: nothing was changed", args[0])
	}
	r := uninstall.AfterExit(pid, 2*time.Minute)
	for _, l := range r.Says() {
		fmt.Println(l)
	}
	return nil
}
