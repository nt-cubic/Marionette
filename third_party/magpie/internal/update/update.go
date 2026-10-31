// Package update keeps magpie current. Builds are published as GitHub
// releases of yetone/magpie-releases; usemagpie.ai/api/latest describes the
// newest one: its version, notes, and every file with its SHA-256.
//
// The macOS app replaces its own bundle: the new zip is downloaded, checked
// against its hash, unpacked, and accepted only if it is signed by the same
// team as the app already installed. A bare binary (the terminal build,
// and the desktop app on Windows and Linux) replaces itself the same way,
// minus the signature.
package update

import (
	"cmp"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"html"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/netproxy"
	"github.com/yetone/magpie/internal/proc"
	"github.com/yetone/magpie/internal/source"
)

// Site is magpie's home; its /api/latest is the update feed.
const Site = "https://usemagpie.ai"

// Feed is where the newest release is described. MAGPIE_UPDATE_FEED points
// it elsewhere, for testing an update against a local server.
func Feed() string {
	if f := os.Getenv("MAGPIE_UPDATE_FEED"); f != "" {
		return f
	}
	return Site + "/api/latest"
}

// Release is one published version.
type Release struct {
	Version string           `json:"version"` // "0.2.0", no v
	Notes   string           `json:"notes"`   // markdown
	URL     string           `json:"url"`     // the release page
	Assets  map[string]Asset `json:"assets"`  // by file name
}

// Asset is one downloadable file of a release.
type Asset struct {
	URL    string `json:"url"`
	Size   int64  `json:"size"`
	SHA256 string `json:"sha256"`
}

// client asks the feed and downloads a release through the proxy the rest
// of magpie's requests take — Settings' Proxy, else the environment's, else
// the system's (#294) — on a transport of its own, whatever the process has
// done to http.DefaultTransport.
var client = &http.Client{Timeout: 10 * time.Minute, Transport: proxied()}

func proxied() http.RoundTripper {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.Proxy = netproxy.Func
	return netproxy.Dispatch(t)
}

// Latest asks the feed for the newest release, its notes in English.
func Latest(ctx context.Context) (*Release, error) { return LatestIn(ctx, "") }

// LatestIn asks the feed for the newest release, its notes in lang (see
// InLang): the app's language, which What's new follows. On Settings'
// Stable channel it is Stable's release (see Stable).
func LatestIn(ctx context.Context, lang string) (*Release, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", withChannel(withLang(Feed(), lang)), nil)
	if err != nil {
		return nil, err
	}
	res, err := source.Do(client, req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return nil, fmt.Errorf("update feed: %s", res.Status)
	}
	var r Release
	if err := json.NewDecoder(res.Body).Decode(&r); err != nil {
		return nil, fmt.Errorf("update feed: %w", err)
	}
	if parse(r.Version) == nil {
		return nil, fmt.Errorf("update feed: no version")
	}
	r.Notes = InLang(r.Notes, lang)
	return &r, nil
}

// Released reports whether v is a release version rather than a build from
// source ("dev", "0bcb2cc-dirty", git describe's "v0.1.0-3-g0bcb2cc");
// only releases update themselves.
func Released(v string) bool {
	s := parse(v)
	return s != nil && !describe.MatchString(s.pre) && !strings.Contains(s.pre, "dirty")
}

// describe matches what git describe adds after a tag: commits since, hash.
var describe = regexp.MustCompile(`^\d+-g[0-9a-f]+`)

// Newer reports whether version a comes after b. A pre-release comes before
// the release it leads up to.
func Newer(a, b string) bool {
	x, y := parse(a), parse(b)
	if x == nil || y == nil {
		return false
	}
	for i := range 3 {
		if x.n[i] != y.n[i] {
			return x.n[i] > y.n[i]
		}
	}
	return comparePre(x.pre, y.pre) > 0
}

// comparePre orders two pre-releases the way semver does (§11): no
// pre-release comes after any; otherwise identifier by identifier, numbers
// as numbers ("beta.10" after "beta.9"), a number before a word, and a
// shorter list before a longer one it starts.
func comparePre(a, b string) int {
	switch {
	case a == b:
		return 0
	case a == "":
		return 1
	case b == "":
		return -1
	}
	x, y := strings.Split(a, "."), strings.Split(b, ".")
	for i := 0; i < len(x) && i < len(y); i++ {
		m, errM := strconv.ParseUint(x[i], 10, 64)
		n, errN := strconv.ParseUint(y[i], 10, 64)
		switch {
		case errM == nil && errN == nil:
			if m != n {
				return cmp.Compare(m, n)
			}
		case errM == nil:
			return -1
		case errN == nil:
			return 1
		default:
			if c := strings.Compare(x[i], y[i]); c != 0 {
				return c
			}
		}
	}
	return cmp.Compare(len(x), len(y))
}

type semver struct {
	n   [3]int
	pre string
}

func parse(v string) *semver {
	v = strings.TrimPrefix(strings.TrimSpace(v), "v")
	// build metadata ("+sha.5d0c1dc") plays no part in the order
	v, _, _ = strings.Cut(v, "+")
	v, pre, _ := strings.Cut(v, "-")
	parts := strings.Split(v, ".")
	if len(parts) != 3 {
		return nil
	}
	var s semver
	for i, p := range parts {
		n, err := strconv.Atoi(p)
		if err != nil || n < 0 {
			return nil
		}
		s.n[i] = n
	}
	s.pre = pre
	return &s
}

// GUI says this binary has the desktop app in it. Off the Mac that app is a
// single binary too, and it updates from its own build, not the terminal
// one.
var GUI bool

// AppAsset and BinaryAsset name the files this machine would install.
func AppAsset() string { return "magpie-darwin-" + runtime.GOARCH + ".zip" }
func BinaryAsset() string {
	name := "magpie-cli-" + runtime.GOOS + "-" + runtime.GOARCH
	if GUI && runtime.GOOS != "darwin" {
		name = "magpie-" + runtime.GOOS + "-" + runtime.GOARCH
	}
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	return name
}

// Bundle is the .app the running binary lives in, or "" when it is not in
// one.
func Bundle() string {
	if runtime.GOOS != "darwin" {
		return ""
	}
	exe, err := Executable()
	if err != nil {
		return ""
	}
	// …/magpie.app/Contents/MacOS/magpie
	app := filepath.Dir(filepath.Dir(filepath.Dir(exe)))
	if filepath.Ext(app) != ".app" || filepath.Base(filepath.Dir(exe)) != "MacOS" {
		return ""
	}
	return app
}

// Homebrew is whether exe was installed by Homebrew (brew install magpie),
// which keeps it in its Cellar and has to be the one to upgrade it: a
// binary replaced under it leaves brew thinking the old version is there.
func Homebrew(exe string) bool {
	return strings.Contains(filepath.ToSlash(exe), "/Cellar/magpie/")
}

// Executable is the running binary, symlinks resolved.
func Executable() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	return filepath.EvalSymlinks(exe)
}

// Writable reports whether magpie may replace what lives in dir.
func Writable(dir string) bool {
	f, err := os.CreateTemp(dir, ".magpie-update-*")
	if err != nil {
		return false
	}
	f.Close()
	os.Remove(f.Name())
	return true
}

// Stage downloads the app in rel and unpacks it next to the installed
// bundle, ready for Install. It returns the unpacked app, a bundle whose
// folder is named stagedName.
func Stage(ctx context.Context, rel *Release, bundle string) (string, error) {
	a, ok := rel.Assets[AppAsset()]
	if !ok {
		return "", fmt.Errorf("release %s has no %s", rel.Version, AppAsset())
	}
	dir := stageDir(filepath.Dir(bundle))
	os.RemoveAll(dir)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	zip := filepath.Join(dir, AppAsset())
	if err := download(ctx, a, zip); err != nil {
		os.RemoveAll(dir)
		return "", err
	}
	out := filepath.Join(dir, "unzip")
	if b, err := proc.CommandContext(ctx, "ditto", "-x", "-k", zip, out).CombinedOutput(); err != nil {
		os.RemoveAll(dir)
		return "", fmt.Errorf("unzip: %v: %s", err, b)
	}
	os.Remove(zip)
	// the zip holds magpie.app, which leaves that name at once: a folder
	// ending in .app beside the installed one is taken by LaunchServices
	// for a second magpie, and Spotlight lists it after it is gone (#3965)
	unzipped := filepath.Join(out, "magpie.app")
	app := filepath.Join(dir, stagedName)
	if err := os.Rename(unzipped, app); err != nil {
		os.RemoveAll(dir)
		return "", err
	}
	os.Remove(out)
	unregister(unzipped) // in case it looked in that moment
	if err := sameSigner(ctx, bundle, app); err != nil {
		os.RemoveAll(dir)
		return "", err
	}
	return app, nil
}

// sameSigner accepts app only if its signature is intact and made by the
// team that signed the bundle it replaces.
func sameSigner(ctx context.Context, bundle, app string) error {
	if b, err := proc.CommandContext(ctx, "codesign", "--verify", "--deep", "--strict", app).CombinedOutput(); err != nil {
		return fmt.Errorf("the downloaded app's signature is broken: %s", strings.TrimSpace(string(b)))
	}
	want, got := team(ctx, bundle), team(ctx, app)
	if want != got {
		return fmt.Errorf("the downloaded app is signed by %q, not %q", got, want)
	}
	return nil
}

func team(ctx context.Context, app string) string {
	b, _ := proc.CommandContext(ctx, "codesign", "-dv", app).CombinedOutput()
	for _, l := range strings.Split(string(b), "\n") {
		if v, ok := strings.CutPrefix(l, "TeamIdentifier="); ok && v != "not set" {
			return v
		}
	}
	return ""
}

// stagedName is what the new app is unpacked as in stageDir, and asideName
// what the installed one is moved to while they are swapped. Neither ends
// in .app, so LaunchServices never registers either as a second magpie
// (#3965); the renames don't need the name, and stay on one volume.
const (
	stagedName = "payload"
	asideName  = "old"
)

// stageDir is where an update is unpacked: beside what it replaces, so
// installing it is a rename on one volume, or, where magpie may not write,
// in its cache, to be moved in with the administrator's password.
func stageDir(dir string) string {
	if !Writable(dir) {
		if cache, err := appdir.SystemCache(); err == nil {
			return filepath.Join(cache, "update")
		}
	}
	return filepath.Join(dir, ".magpie-update")
}

// Install swaps the staged app in for bundle. The running copy keeps going
// until it quits; the next launch is the new one. Where the folder is not
// magpie's to change the error is a permission one (NeedsAdmin), and
// InstallAsAdmin can do it instead.
func Install(staged, bundle string) error {
	old := filepath.Join(filepath.Dir(staged), asideName)
	os.RemoveAll(old)
	if err := os.Rename(bundle, old); err != nil {
		return err
	}
	if err := os.Rename(staged, bundle); err != nil {
		os.Rename(old, bundle) // put it back
		return err
	}
	os.RemoveAll(filepath.Dir(staged)) // .magpie-update
	return nil
}

// InstallAsAdmin is Install with the administrator's password, asked for
// by the system.
func InstallAsAdmin(staged, bundle string) error {
	old := filepath.Join(filepath.Dir(staged), asideName)
	err := asAdmin(swapScript(staged, bundle, old))
	if err == nil {
		os.RemoveAll(filepath.Dir(staged))
	}
	return err
}

// Relaunch opens bundle again once this process (pid) has exited: with its
// window when window is set, else only its menu bar icon (`tray`, as a
// login launch), which a restart by itself uses while no window was up.
// Each try, and what it came to, is written to relaunch.log in magpie's
// own folder.
func Relaunch(bundle string, window bool) error {
	return runAfterExit(os.Getpid(), relaunchCommand(bundle, window, relaunchLog()))
}

// openWaits are the pauses, in seconds, between tries of the relaunch's
// open: about 13s in all. Just after magpie exits, LaunchServices can
// still hold it as running and send the open to the process that is gone,
// and open fails (-600, procNotFound; seen with the screen locked). Tried
// once, nothing started magpie again until the user did, nine hours later.
var openWaits = []string{"0.5", "1", "1", "2", "2", "3", "4"}

// relaunchLogName is the file in magpie's folder the relaunch writes each
// try of its open to, and the start it falls back to: the shell that runs
// them has no output anyone sees, and a magpie that never came back left
// only the system log to say why.
const relaunchLogName = "relaunch.log"

// relaunchLog is the relaunch's log file, made ready to append to; once it
// is past 64KB the last one is kept as relaunch.log.old. "" when magpie's
// folder can't be made: the tries then go unwritten.
func relaunchLog() string {
	h, err := appdir.Home()
	if err != nil || h == "" {
		return ""
	}
	dir := appdir.Config()
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return ""
	}
	p := filepath.Join(dir, relaunchLogName)
	if fi, err := os.Stat(p); err == nil && fi.Size() > 64<<10 {
		os.Rename(p, p+".old")
	}
	return p
}

// relaunchCommand is the shell command Relaunch runs once magpie has
// exited: open, tried again while it fails, and, when it never worked,
// the bundle's own executable run straight, as the login launch agent
// runs it. With the screen locked, magpie stayed closed after an update
// even with the 13s of tries (molvqingtai, after fb23bca5c): LaunchServices
// can go on refusing the open for as long as the session is locked, and a
// program run by its path needs nothing from it to start.
func relaunchCommand(bundle string, window bool, log string) string {
	return relaunchScript(bundle, window, log, openWaits)
}

// relaunchScript is relaunchCommand with the pauses between opens.
func relaunchScript(bundle string, window bool, log string, waits []string) string {
	if log == "" {
		log = os.DevNull
	}
	exe := filepath.Join(bundle, "Contents", "MacOS", bundleExecutable(bundle))
	run := shellQuote(exe)
	for _, a := range RelaunchArgs(window, "") {
		run += " " + shellQuote(a)
	}
	var b strings.Builder
	b.WriteString("log=" + shellQuote(log) + "; ")
	b.WriteString(`note() { echo "$(date '+%Y-%m-%d %H:%M:%S %z') $*" >> "$log" 2>/dev/null; }; `)
	// open's own exit status and what it said, on one line
	b.WriteString(`try() { n=$((n+1)); e=$(` + relaunchOpen(bundle, window) + ` 2>&1 >/dev/null); s=$?; note "open try $n: exit $s $(printf '%s' "$e" | tr '\n' ' ')"; return $s; }; `)
	b.WriteString("n=0; note " + shellQuote("magpie has exited; opening "+bundle) + "; ")
	b.WriteString(retried("try", waits))
	b.WriteString(" || { note \"open gave up after $n tries; starting \"" + shellQuote(run) + "; ")
	b.WriteString("[ -x " + shellQuote(exe) + " ] || { note \"no executable at \"" + shellQuote(exe) + "; exit 1; }; ")
	b.WriteString("exec " + run + " </dev/null >/dev/null 2>&1; }")
	return b.String()
}

// bundleExecutableKey finds the program an app's Info.plist names.
var bundleExecutableKey = regexp.MustCompile(`<key>CFBundleExecutable</key>\s*<string>([^<]+)</string>`)

// bundleExecutable is the name of the program in bundle's Contents/MacOS:
// the one its Info.plist names, else this one's own when it runs from
// that bundle, else magpie.
func bundleExecutable(bundle string) string {
	if b, err := os.ReadFile(filepath.Join(bundle, "Contents", "Info.plist")); err == nil {
		if m := bundleExecutableKey.FindSubmatch(b); m != nil {
			if name := strings.TrimSpace(html.UnescapeString(string(m[1]))); name != "" && !strings.ContainsRune(name, '/') {
				return name
			}
		}
	}
	if exe, err := Executable(); err == nil && filepath.Dir(exe) == filepath.Join(bundle, "Contents", "MacOS") {
		return filepath.Base(exe)
	}
	return "magpie"
}

// retried is a shell command that runs command until it succeeds, sleeping
// each of waits in turn after a failure; the last try's status is its own.
func retried(command string, waits []string) string {
	if len(waits) == 0 {
		return command
	}
	return fmt.Sprintf("for w in %s; do %s && exit 0; sleep $w; done; %s", strings.Join(waits, " "), command, command)
}

// runAfterExit starts a shell that runs command once pid has exited. The
// shell is in a session of its own: launchd ends the process group of a
// job whose process exits, so one left in magpie's group was killed as
// magpie quit and the new version never opened. That is the launch agent
// of Open at login without AbandonProcessGroup (ee16c657), and, on macOS
// 13 at least, the job LaunchServices runs an app in when it is opened from
// the Dock or Finder, which magpie can't set that key on (#2362).
func runAfterExit(pid int, command string) error {
	script := fmt.Sprintf(`while kill -0 %d 2>/dev/null; do sleep 0.2; done; %s`, pid, command)
	cmd := proc.Command("/bin/sh", "-c", script)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = nil, nil, nil
	detach(cmd)
	return cmd.Start()
}

// relaunchOpen is the open command Relaunch runs for bundle. -n asks
// LaunchServices for a new instance: without it, open looks the app up
// among those running first, and just after magpie exits LaunchServices
// can still have the gone process registered as magpie. open then sends
// it a reopen Apple event (aevt/rapp) rather than launching the bundle,
// and fails with -600, procNotFound ("Specified target process … does
// not exist", "LAUNCH: Launch failed in CSUI"; Linka's log on Discord,
// macOS 27.0.1, v0.1.1308), as often as it is tried. A magpie that is in
// fact up still has the one instance: the new process finds the instance
// lock held and hands over to it (runningAlready in internal/gui).
func relaunchOpen(bundle string, window bool) string {
	if window {
		return fmt.Sprintf("open -n %q", bundle)
	}
	return fmt.Sprintf("open -n %q --args tray", bundle)
}

// ReplaceBinary puts the release's build for this binary where the
// running one is.
func ReplaceBinary(ctx context.Context, rel *Release) error {
	exe, err := Executable()
	if err != nil {
		return err
	}
	staged, err := StageBinary(ctx, rel)
	if err != nil {
		return err
	}
	err = InstallBinary(staged, exe)
	if NeedsAdmin(err) && CanElevate() {
		err = InstallBinaryAsAdmin(staged, exe)
	}
	return err
}

// StageBinary downloads the release's build for this binary next to it (or
// to the cache, where magpie may not write there), ready for InstallBinary.
func StageBinary(ctx context.Context, rel *Release) (string, error) {
	a, ok := rel.Assets[BinaryAsset()]
	if !ok {
		return "", fmt.Errorf("release %s has no %s", rel.Version, BinaryAsset())
	}
	exe, err := Executable()
	if err != nil {
		return "", err
	}
	tmp := exe + ".new"
	if !Writable(filepath.Dir(exe)) {
		dir := stageDir(filepath.Dir(exe))
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return "", err
		}
		tmp = filepath.Join(dir, filepath.Base(exe)+".new")
	}
	if err := download(ctx, a, tmp); err != nil {
		return "", err
	}
	if err := os.Chmod(tmp, 0o755); err != nil {
		os.Remove(tmp)
		return "", err
	}
	return tmp, nil
}

// InstallBinary swaps a staged binary in for exe, the running one, which
// keeps going until it exits.
func InstallBinary(staged, exe string) error {
	_, err := InstallBinaryAside(staged, exe)
	return err
}

// InstallBinaryAside is InstallBinary, saying where the running exe was
// moved to on Windows ("" elsewhere), for RelaunchBinary to put it back
// should the new version not start. A download this computer won't start
// (Smart App Control, #894) isn't put in: a *BlockedError, and the
// download removed.
func InstallBinaryAside(staged, exe string) (string, error) {
	var old string
	if runtime.GOOS == "windows" {
		// tried once per download: a restart clicked again after a failed
		// one doesn't start it again, which would have an antivirus that
		// checks a program as it starts hold it again for the move below
		if !trialled.passed(staged) {
			if err := canStart(staged); err != nil {
				os.Remove(staged)
				return "", &BlockedError{Err: err}
			}
			trialled.note(staged)
		}
		// A running .exe cannot be overwritten, but it can be moved aside.
		// What the last update moved aside may still be running too (a
		// `magpie serve` started before it), and can't be removed or
		// replaced then: this one goes beside it, under a name of its own.
		// The download is kept, for another try.
		RemoveOld(exe)
		var err error
		if old, err = moveAside(exe); err != nil {
			return "", err
		}
	}
	if err := moveIn(staged, exe); err != nil {
		if heldOpen(err) {
			// the reason and what to do, shown as is in the version row
			// and the Update pill's tooltip, without the paths
			err = &saidError{fmt.Sprintf("couldn't put the new version in place of %s: another program still has the download open (often an antivirus checking it): let magpie through it and restart to update again, or download the new version and put it in place of this one", filepath.Base(exe)), err}
		}
		if old != "" {
			if restoreErr := moveIn(old, exe); restoreErr != nil {
				return "", errors.Join(err, fmt.Errorf("couldn't restore %s from %s: %w", filepath.Base(exe), old, restoreErr))
			}
		} else if !NeedsAdmin(err) { // kept for InstallBinaryAsAdmin
			os.Remove(staged)
		}
		return "", err
	}
	return old, nil
}

// saidError is err said in other words: what is shown, err what
// errors.Is and errors.As find.
type saidError struct {
	said string
	err  error
}

func (e *saidError) Error() string { return e.said }
func (e *saidError) Unwrap() error { return e.err }

// renameFile is os.Rename; tests make it fail.
var renameFile = os.Rename

// asideWaits are the pauses between tries at moving the running exe aside:
// an antivirus scanning it or a sync client (OneDrive) reading it holds it
// open, which Windows won't rename under, for a moment, and lets go.
var asideWaits = []time.Duration{100 * time.Millisecond, 200 * time.Millisecond, 400 * time.Millisecond, 800 * time.Millisecond, 1500 * time.Millisecond}

// heldFor is how long a file Windows says another program holds open is
// waited for, past asideWaits, the last pause repeated: an antivirus that
// checks a new program as it starts may hold a 50 MB download longer than
// asideWaits' three seconds, and a restart that gave up then would fail
// the same way on every click, each click's trial start having it checked
// again (白菜 on Discord). The user clicked restart and sees "Restarting…"
// meanwhile. A var for tests.
var heldFor = 30 * time.Second

// waitHeld is whether to try again after try i (from 0) failed with err,
// begun at start, sleeping first when so: through asideWaits for any error
// retry allows, then on while err is a file held open, up to heldFor.
func waitHeld(i int, err error, start time.Time, retry bool) bool {
	if !retry {
		return false
	}
	if i < len(asideWaits) {
		time.Sleep(asideWaits[i])
		return true
	}
	if !heldOpen(err) || len(asideWaits) == 0 {
		return false
	}
	pause := asideWaits[len(asideWaits)-1]
	if time.Since(start)+pause > heldFor {
		return false
	}
	time.Sleep(pause)
	return true
}

// trialled is the download canStart last let through, by path, size and
// time written, so a restart tried again on it doesn't start it again.
var trialled trialNote

type trialNote struct {
	mu   sync.Mutex
	path string
	size int64
	mod  time.Time
}

func (n *trialNote) passed(path string) bool {
	fi, err := os.Stat(path)
	if err != nil {
		return false
	}
	n.mu.Lock()
	defer n.mu.Unlock()
	return n.path == path && n.size == fi.Size() && n.mod.Equal(fi.ModTime())
}

func (n *trialNote) note(path string) {
	fi, err := os.Stat(path)
	n.mu.Lock()
	defer n.mu.Unlock()
	if err != nil {
		n.path = ""
		return
	}
	n.path, n.size, n.mod = path, fi.Size(), fi.ModTime()
}

// Windows' errors for a file another program holds open without letting
// it be renamed.
const (
	errSharingViolation syscall.Errno = 32
	errLockViolation    syscall.Errno = 33
)

// moveIn moves from to exe, trying again for a few seconds while
// something holds from open, as moveAside does for the running exe. On
// Windows that is the download just after canStart's trial: an antivirus
// that checks a program as its process starts can hold its file a moment
// after the process has ended, and a move made at once failed the first
// install of every update (#2272). Any other failure is returned at once.
func moveIn(from, exe string) error {
	start := time.Now()
	for i := 0; ; i++ {
		err := os.Rename(from, exe)
		if err == nil || !waitHeld(i, err, start, heldOpen(err)) {
			return err
		}
	}
}

// heldOpen is whether err is Windows refusing a file another program has
// open.
func heldOpen(err error) bool {
	return errors.Is(err, errSharingViolation) || errors.Is(err, errLockViolation)
}

// moveAside moves the running exe out of the way of the new version, under
// a name of its own, trying again for a few seconds while something has it
// open. What it says when it can't is the reason and what to do: the error
// is shown as is in the version row. On success it returns the actual name,
// so a failed installation can put the running exe back.
func moveAside(exe string) (string, error) {
	var err error
	start := time.Now()
	for i := 0; ; i++ {
		old := oldName(exe)
		if err = renameFile(exe, old); err == nil {
			return old, nil
		}
		if i == 0 {
			os.Chmod(exe, 0o755) // a read-only exe: the attribute off
		}
		if !waitHeld(i, err, start, !errors.Is(err, fs.ErrNotExist)) {
			break
		}
	}
	why := err.Error()
	var le *os.LinkError
	if errors.As(err, &le) {
		why = le.Err.Error() // the paths are known; the reason is what's new
	}
	var hint string
	switch {
	case heldOpen(err):
		hint = "another program has it open (often an antivirus or OneDrive): let magpie through it and restart to update again, or download the new version and put it in place of this one"
	case errors.Is(err, fs.ErrPermission):
		hint = "Windows doesn't let magpie change files in " + filepath.Dir(exe) + " (an antivirus' folder protection, or the folder's permissions): let magpie through, or download the new version and put it in place of this one"
	default:
		hint = "download the new version and put it in place of this one"
	}
	return "", fmt.Errorf("couldn't move %s aside to put the new version in: %s; %s", filepath.Base(exe), strings.TrimRight(why, ". 。"), hint)
}

// oldName is where a running exe is moved aside to: exe.old, or when
// that is still there (in use), exe.old-2, exe.old-3…
func oldName(exe string) string {
	name := exe + ".old"
	for n := 2; ; n++ {
		if _, err := os.Lstat(name); os.IsNotExist(err) {
			return name
		}
		name = fmt.Sprintf("%s.old-%d", exe, n)
	}
}

// RemoveOld removes what earlier updates moved aside of exe, those no
// longer running.
func RemoveOld(exe string) {
	os.Remove(exe + ".old")
	olds, _ := filepath.Glob(exe + ".old-*")
	for _, o := range olds {
		os.Remove(o)
	}
}

// Replaced reports whether exe is no longer the binary that was there when
// this process looked (started, from os.Stat): another magpie installed an
// update over it, and this one runs from where it was moved aside.
func Replaced(exe string, started os.FileInfo) bool {
	now, err := os.Stat(exe)
	return err == nil && started != nil && (now.Size() != started.Size() || !now.ModTime().Equal(started.ModTime()))
}

// RemoveStaleNew removes exe.new when it is exe over again: the update a
// magpie left running from before it downloaded once more.
func RemoveStaleNew(exe string) {
	staged := exe + ".new"
	a, err1 := os.Stat(exe)
	b, err2 := os.Stat(staged)
	if err1 != nil || err2 != nil || a.Size() != b.Size() {
		return
	}
	if ha, hb := fileHash(exe), fileHash(staged); ha != "" && ha == hb {
		os.Remove(staged)
	}
}

func fileHash(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return ""
	}
	return hex.EncodeToString(h.Sum(nil))
}

// InstallBinaryAsAdmin is InstallBinary with the administrator's password.
func InstallBinaryAsAdmin(staged, exe string) error {
	err := asAdmin("mv -f " + shellQuote(staged) + " " + shellQuote(exe))
	if err != nil && !errors.Is(err, ErrCanceled) {
		os.Remove(staged)
	}
	return err
}

// RelaunchBinary starts exe again: with its window on view when window is
// set (the window was open), else as the tray app alone, as autostart
// starts it. The new process waits for this one to exit before it takes
// the gateway's port; see AwaitPredecessor. With old, where this one was
// moved aside to (Windows), it waits for the new one to say it is up, and
// when it can't start puts old back and says so (a *BlockedError): this
// one then runs on rather than quitting.
func RelaunchBinary(exe, old string, window bool, view string) error {
	args := RelaunchArgs(window, view)
	env := append(os.Environ(), "MAGPIE_REPLACES="+strconv.Itoa(os.Getpid()))
	if old != "" {
		return startChecked(exe, old, args, env)
	}
	_, _, err := launch(exe, args, env)
	return err
}

// RelaunchArgs is what RelaunchBinary starts magpie with: `gui [view]` for
// the window, `tray` for the tray icon alone.
func RelaunchArgs(window bool, view string) []string {
	switch {
	case !window:
		return []string{"tray"}
	case view != "":
		return []string{"gui", view}
	}
	return []string{"gui"}
}

// AwaitPredecessor blocks, for a while at most, until the magpie that
// relaunched this one has exited.
func AwaitPredecessor() {
	reportStarted() // first: the one that started this waits for it
	pid, err := strconv.Atoi(os.Getenv("MAGPIE_REPLACES"))
	os.Unsetenv("MAGPIE_REPLACES")
	if err != nil || pid <= 0 {
		return
	}
	for deadline := time.Now().Add(20 * time.Second); time.Now().Before(deadline) && alive(pid); {
		time.Sleep(100 * time.Millisecond)
	}
}

type progressKey struct{}

// WithProgress has downloads made under ctx report how far along they are;
// total is 0 while the size is unknown.
func WithProgress(ctx context.Context, f func(done, total int64)) context.Context {
	return context.WithValue(ctx, progressKey{}, f)
}

// retryWaits are the pauses before each try after the first: a
// connection that drops at the start or part way (GitHub's asset host from
// some networks: AIMetaPhD on X, "EOF" at 0.1.1618) is tried again, from
// where it broke off. A var for tests.
var retryWaits = []time.Duration{2 * time.Second, 4 * time.Second, 8 * time.Second, 16 * time.Second, 30 * time.Second}

// download fetches a to path and checks its hash. A connection that drops
// is tried again after each of retryWaits, asking with Range for what is
// still missing; the hash is checked over the whole file, the bytes kept
// from the tries before included.
func download(ctx context.Context, a Asset, path string) error {
	if a.SHA256 == "" {
		return errors.New("the release lists no checksum for " + filepath.Base(path))
	}
	os.Remove(path) // a file left by an earlier run is no part of this one
	var err error
	for try := 0; try <= len(retryWaits); try++ {
		if try > 0 {
			select {
			case <-ctx.Done():
				return err
			case <-time.After(retryWaits[try-1]):
			}
		}
		if err = fetch(ctx, a, path); err == nil || ctx.Err() != nil || !Network(err) {
			break
		}
	}
	if err != nil {
		os.Remove(path)
	}
	return err
}

// NetError is a download the network broke off: the connection failed or
// dropped part way. It names the file and the cause, never the address:
// GitHub's redirect is a signed URL, hundreds of characters of signature
// and token that say nothing to the user.
type NetError struct {
	File string
	Err  error
}

func (e *NetError) Error() string {
	cause := e.Err
	var ue *url.Error
	if errors.As(cause, &ue) {
		cause = ue.Err
	}
	return fmt.Sprintf("the download of %s broke off (%v)", e.File, cause)
}

func (e *NetError) Unwrap() error { return e.Err }

// Network reports whether err is a download the network broke off, which
// trying again may get through.
func Network(err error) bool { return errors.As(err, new(*NetError)) }

// fetch downloads a to path, or the rest of it when path holds the start
// already (a try before broke off): what is there is asked for no more
// (Range), and a host that sends the whole file again has it written from
// the beginning. A try the network breaks off keeps what came, for the next
// one; any other failure removes it.
func fetch(ctx context.Context, a Asset, path string) error {
	// through a mirror when one is given (mirror.go): the hash checked
	// below is still the feed's
	u := mirrored(mirrorOf(ctx), a.URL)
	mirror := strings.TrimSuffix(u, a.URL)
	name := filepath.Base(path)
	var have int64
	if fi, err := os.Stat(path); err == nil && fi.Mode().IsRegular() {
		have = fi.Size()
	}
	req, err := http.NewRequestWithContext(ctx, "GET", u, nil)
	if err != nil {
		return err
	}
	if have > 0 {
		req.Header.Set("Range", fmt.Sprintf("bytes=%d-", have))
	}
	fail := func(err error) error {
		if !Network(err) {
			os.Remove(path)
		}
		if u != a.URL && !errors.As(err, new(*MirrorError)) {
			err = &MirrorError{Mirror: mirror, Err: err}
		}
		return err
	}
	res, err := source.Do(client, req)
	if err != nil {
		if ctx.Err() != nil {
			return fail(err)
		}
		return fail(&NetError{File: name, Err: err})
	}
	defer res.Body.Close()
	flag := os.O_WRONLY | os.O_CREATE | os.O_TRUNC
	switch {
	case res.StatusCode == http.StatusPartialContent && have > 0 && strings.HasPrefix(res.Header.Get("Content-Range"), fmt.Sprintf("bytes %d-", have)):
		flag = os.O_WRONLY | os.O_APPEND
	case res.StatusCode == http.StatusOK:
		have = 0 // the whole file again
	default:
		if u != a.URL {
			return fail(&MirrorError{Mirror: mirror, Err: fmt.Errorf("download %s through the mirror %s: %s", name, mirror, res.Status)})
		}
		return fail(fmt.Errorf("download %s: %s", name, res.Status))
	}
	f, err := os.OpenFile(path, flag, 0o644)
	if err != nil {
		return fail(err)
	}
	// a read that fails is the network's; a write that fails (a full
	// disk) is not, and isn't tried again
	in := &readErr{r: res.Body}
	var body io.Reader = in
	if report, ok := ctx.Value(progressKey{}).(func(done, total int64)); ok {
		total := res.ContentLength
		if total > 0 {
			total += have
		} else {
			total = a.Size
		}
		body = &counter{r: in, done: have, total: max(total, 0), report: report}
		report(have, max(total, 0))
	}
	_, err = io.Copy(f, body)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		if ctx.Err() == nil && in.err != nil {
			err = &NetError{File: name, Err: err}
		}
		return fail(err)
	}
	// the whole file, what earlier tries kept included, against the feed's
	if !strings.EqualFold(fileHash(path), a.SHA256) {
		err = fmt.Errorf("%s does not match its checksum", name)
		if u != a.URL {
			err = fmt.Errorf("%s from the mirror %s does not match its checksum from %s; not installed", name, mirror, Site)
		}
		return fail(err)
	}
	return nil
}

// readErr keeps the error its reader returned, io.EOF aside.
type readErr struct {
	r   io.Reader
	err error
}

func (r *readErr) Read(p []byte) (int, error) {
	n, err := r.r.Read(p)
	if err != nil && err != io.EOF {
		r.err = err
	}
	return n, err
}

// counter reports bytes as they are read.
type counter struct {
	r      io.Reader
	done   int64
	total  int64
	report func(done, total int64)
}

func (c *counter) Read(p []byte) (int, error) {
	n, err := c.r.Read(p)
	c.done += int64(n)
	c.report(c.done, c.total)
	return n, err
}

// Alive reports whether the process pid is still running.
func Alive(pid int) bool { return alive(pid) }

// OpenBeside opens bundle now, a new instance beside this one (open -n),
// tried again while open fails: a restart that hands the gateway over
// (internal/handoff) starts the new version before this one quits. With
// window, its window; else its menu bar icon only, as Relaunch.
func OpenBeside(bundle string, window bool) error {
	cmd := proc.Command("/bin/sh", "-c", retried(openBeside(bundle, window), openWaits))
	cmd.Stdin, cmd.Stdout, cmd.Stderr = nil, nil, nil
	detach(cmd)
	return cmd.Start()
}

// openBeside is the open command OpenBeside runs for bundle.
func openBeside(bundle string, window bool) string {
	if window {
		return fmt.Sprintf("open -n %q", bundle)
	}
	return fmt.Sprintf("open -n %q --args tray", bundle)
}

// StartBeside starts exe now, beside this process, as RelaunchBinary's
// args say: the new version of a restart that hands the gateway over. It
// isn't told to wait for this one (MAGPIE_REPLACES): it takes the gateway
// from it while it runs.
func StartBeside(exe string, window bool, view string) error {
	_, _, err := launch(exe, RelaunchArgs(window, view), os.Environ())
	return err
}
