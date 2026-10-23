package agent

import (
	"context"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/proc"
)

// An agent that is a desktop app as well as a CLI has the app's version shown
// beside the CLI's (#1334, laacmilan: with Codex's CLI and its app both
// installed, the Agents page showed only the CLI's version). Both read the
// same settings, so one row stands for both.

// desktopApp is the macOS bundle an agent's desktop app is known by: its
// bundle id, and the names it has been installed under. OpenAI's Codex app
// is ChatGPT.app (com.openai.codex) since it took ChatGPT's name; the bundle
// id tells it from the ChatGPT chat app (com.openai.chat) of the same name.
// win is its executable under %LOCALAPPDATA%\Programs, where a per-user
// installer puts it on Windows. store and publisher identify a Microsoft
// Store package by its package name and publisher id. guid is the key an
// electron-builder installer registers the app under for the current user
// (HKCU\Software\<guid>'s InstallLocation, and its Uninstall entry's
// DisplayVersion): its installer lets the user pick another folder.
type desktopApp struct {
	id        string
	names     []string
	win       string
	store     string
	publisher string
	guid      string
}

var desktopApps = map[string]desktopApp{
	"codex": {id: "com.openai.codex", names: []string{"ChatGPT.app", "Codex.app"}, store: "OpenAI.Codex", publisher: "2p2nqsd0c76g0"},
	// DeepSeek Harness Desktop (0.2.0-rc.2's DeepSeek Harness.app,
	// com.deepseek.dsh) reads ~/.dsh as the CLI does — its own profile,
	// profiles/desktop, beside the CLI's — and needs no CLI: its terminal
	// command is optional (star on Discord: 未找到 CLI with the desktop app
	// in use). On Windows its installer (electron-builder's per-user NSIS,
	// productName DeepSeek Harness) puts it under %LOCALAPPDATA%\Programs
	// by default; the folder can be changed ("Choose an empty folder or the
	// registered DeepSeek Harness installation folder"), and the installer
	// registers it under HKCU\Software\1bf39983-… (#2279, read from
	// 0.2.0-rc.2's dsh-latest-windows-x64.exe installer script).
	"dsh": {id: "com.deepseek.dsh", names: []string{"DeepSeek Harness.app"}, win: filepath.Join("DeepSeek Harness", "DeepSeek Harness.exe"), guid: "1bf39983-50d0-5fe0-9ef4-cece76f67c5e"},
}

// appFolders are where apps are installed; a var so tests can point it
// elsewhere: /Applications and ~/Applications on macOS, %LOCALAPPDATA%\Programs
// on Windows. Store packages are read from the current user's AppX registry.
var appFolders = func() []string {
	switch runtime.GOOS {
	case "darwin":
		home, err := os.UserHomeDir()
		if err != nil {
			return proc.SystemDirs("/Applications")
		}
		return macAppFolders(home)
	case "windows":
		if local := appdir.Getenv("LOCALAPPDATA"); local != "" {
			return []string{filepath.Join(local, "Programs")}
		}
	}
	return nil
}

// macAppFolders are the folders a Mac keeps apps in: /Applications and
// home's Applications. /Applications goes through proc.SystemDirs, so in a
// test binary testenv isolated it lies under the test's home: tests read
// the developer's own /Applications, and on a Mac with DeepSeek Harness.app
// installed dsh's CLI was never missing (#1823, mdwsk88). Every reader of a
// Mac's installed apps takes its folders from here.
func macAppFolders(home string) []string {
	return append(proc.SystemDirs("/Applications"), filepath.Join(home, "Applications"))
}

// windowsStorePackages lists the package full names registered for the
// current user on Windows, nil elsewhere; a var so a test can give its own.
// In a test binary testenv isolated it is none: the AppX registry is the
// machine's own, which no home or PATH a test stages takes away, so on a
// Windows machine with the Codex app installed every package's tests saw
// Codex's app and codex's CLI was never gone there (#2254, ggbdpq). Only
// agent's own TestMain stood in for it before.
var windowsStorePackages = storePackages

func storePackages() []string {
	if proc.Isolated() {
		return nil
	}
	return installedWindowsStorePackages()
}

// windowsRegisteredApp is the folder and version an installer registered
// the app with guid under on Windows (registeredIn), "" elsewhere or when
// none is; a var so tests never read the machine's own registry.
var windowsRegisteredApp = registeredWindowsApp

// registryRead is a string value of a key under a Windows registry hive
// ("HKCU" or "HKLM"), "" when there is none.
type registryRead func(hive, key, name string) string

// registeredIn is the folder and version an electron-builder installer
// registered the app with guid under, read through read. It writes
// Software\<guid>'s InstallLocation and an Uninstall\<guid> entry, under
// HKCU for a per-user install and HKLM for one for all users; that entry's
// InstallLocation, else the folder of its DisplayIcon (the app's exe,
// ",0" after it) or UninstallString, is the folder when the first is
// missing. DeepSeek Harness Desktop installed to D:\tools\dsh was found by
// neither %LOCALAPPDATA%\Programs nor HKCU's InstallLocation alone (#2325,
// cjyrainbow).
func registeredIn(guid string, read registryRead) (dir, version string) {
	uninstall := `Software\Microsoft\Windows\CurrentVersion\Uninstall\` + guid
	for _, hive := range []string{"HKCU", "HKLM"} {
		at := read(hive, `Software\`+guid, "InstallLocation")
		if at == "" {
			at = read(hive, uninstall, "InstallLocation")
		}
		for _, name := range []string{"DisplayIcon", "UninstallString"} {
			if at != "" {
				break
			}
			at = registeredExeDir(read(hive, uninstall, name))
		}
		if at = strings.Trim(strings.TrimSpace(at), `"`); at != "" {
			return at, read(hive, uninstall, "DisplayVersion")
		}
	}
	return "", ""
}

// registeredExeDir is the folder of the program a registry value names
// ("D:\tools\dsh\DeepSeek Harness.exe,0", "\"…\Uninstall X.exe\" /currentuser"),
// "" when it names none.
func registeredExeDir(v string) string {
	v = strings.TrimSpace(v)
	if strings.HasPrefix(v, `"`) {
		if end := strings.Index(v[1:], `"`); end >= 0 {
			v = v[1 : 1+end]
		}
	} else if i := strings.LastIndex(strings.ToLower(v), ".exe"); i >= 0 {
		v = v[:i+len(".exe")]
	}
	if !strings.HasSuffix(strings.ToLower(v), ".exe") {
		return ""
	}
	i := strings.LastIndexAny(v, `\/`)
	if i <= 0 {
		return ""
	}
	return v[:i]
}

var (
	plistIDRe      = regexp.MustCompile(`<key>CFBundleIdentifier</key>\s*<string>([^<]+)</string>`)
	plistVersionRe = regexp.MustCompile(`<key>CFBundleShortVersionString</key>\s*<string>([^<]+)</string>`)
)

// macBundle reports whether app's bundle is in one of appFolders, under one
// of its names: with its bundle id, or an Info.plist that isn't XML (a
// binary one says nothing either way, and the name is the app's own).
func macBundle(app desktopApp) bool {
	for _, bundle := range macBundles(app) {
		b, err := os.ReadFile(filepath.Join(bundle, "Contents", "Info.plist"))
		if err != nil {
			continue
		}
		if id := plistIDRe.FindSubmatch(b); id == nil || strings.TrimSpace(string(id[1])) == app.id {
			return true
		}
	}
	return false
}

// macBundles are where app's bundle may be on a Mac, the likeliest first:
// under one of its names in appFolders, or in a folder of theirs one level
// down (sungoku keeps DeepSeek Harness.app in /Applications/AI, and magpie
// 0.1.1253 said 未找到 CLI, #2325), then wherever Spotlight knows its bundle
// id to be (spotlightApps), as Zed's is found (zedAppPath). A caller still
// reads each one's Info.plist.
func macBundles(app desktopApp) []string {
	out := inAppFolders(appFolders(), app.names...)
	if app.id != "" {
		for _, p := range spotlightApps(app.id) {
			if strings.HasSuffix(p, ".app") && !slices.Contains(out, p) {
				out = append(out, p)
			}
		}
	}
	return out
}

// inAppFolders are the paths names may have in dirs, a Mac's app folders:
// in each, then in each folder of its one level down that is no bundle
// itself (/Applications/AI/DeepSeek Harness.app, #2325).
func inAppFolders(dirs []string, names ...string) []string {
	var out []string
	for _, dir := range dirs {
		for _, name := range names {
			out = append(out, filepath.Join(dir, name))
		}
		entries, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		for _, e := range entries {
			if !e.IsDir() || strings.HasSuffix(e.Name(), ".app") || strings.HasPrefix(e.Name(), ".") {
				continue
			}
			for _, name := range names {
				out = append(out, filepath.Join(dir, e.Name(), name))
			}
		}
	}
	return out
}

// spotlightApps are the bundles Spotlight has with bundle id id, on a Mac;
// none elsewhere. A var so tests never ask this machine's Spotlight.
var spotlightApps = spotlightBundles

// spotlight keeps each bundle id's answer for spotlightFresh: the Agents
// page asks whether an app is installed on every read, and an app that
// isn't anywhere would ask mdfind each time.
var spotlight struct {
	sync.Mutex
	m map[string]spotlightAnswer
}

type spotlightAnswer struct {
	paths []string
	at    time.Time
}

const spotlightFresh = 5 * time.Minute

// spotlightSandbox is testenv.Marker, which agent can't import: set in a
// test binary testenv isolated, where this Mac's Spotlight would find the
// developer's own apps (#1823) from every package that asks for one.
const spotlightSandbox = "MAGPIE_TEST_SANDBOX"

func spotlightBundles(id string) []string {
	if runtime.GOOS != "darwin" || os.Getenv(spotlightSandbox) != "" {
		return nil
	}
	spotlight.Lock()
	defer spotlight.Unlock()
	if a, ok := spotlight.m[id]; ok && time.Since(a.at) < spotlightFresh {
		return a.paths
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	var paths []string
	out, err := proc.CommandContext(ctx, "mdfind", "kMDItemCFBundleIdentifier == '"+id+"'").Output()
	if err == nil {
		for _, p := range strings.Split(strings.TrimSpace(string(out)), "\n") {
			if p = strings.TrimSpace(p); p != "" {
				paths = append(paths, p)
			}
		}
	}
	if spotlight.m == nil {
		spotlight.m = map[string]spotlightAnswer{}
	}
	spotlight.m[id] = spotlightAnswer{paths: paths, at: time.Now()}
	return paths
}

// AppVersion is the version of the agent's desktop app installed here, ""
// for an agent without one magpie knows, one not installed, or an Info.plist
// that doesn't say (a binary one isn't read).
func (a *Agent) AppVersion() string {
	app, ok := desktopApps[a.ID]
	if !ok || a.WSL != "" {
		return ""
	}
	return app.version()
}

// version is the version of app installed here, "" when it isn't or doesn't
// say (see AppVersion).
func (app desktopApp) version() string {
	for _, bundle := range macBundles(app) {
		b, err := os.ReadFile(filepath.Join(bundle, "Contents", "Info.plist"))
		if err != nil {
			continue
		}
		id, v := plistIDRe.FindSubmatch(b), plistVersionRe.FindSubmatch(b)
		if id == nil || v == nil || strings.TrimSpace(string(id[1])) != app.id {
			continue
		}
		return strings.TrimSpace(string(v[1]))
	}
	if app.store != "" {
		return storeAppVersion(windowsStorePackages(), app.store, app.publisher)
	}
	if dir, v := app.registered(); dir != "" {
		return v
	}
	return ""
}

// registered is the folder the app's installer registered, where its
// executable is, and the version it registered; "" when there is none.
func (app desktopApp) registered() (dir, version string) {
	if app.guid == "" || app.win == "" {
		return "", ""
	}
	dir, version = windowsRegisteredApp(app.guid)
	if dir == "" || !isFile(filepath.Join(dir, filepath.Base(app.win))) {
		return "", ""
	}
	return dir, strings.TrimSpace(version)
}

// storeAppVersion finds the newest version of an app among Microsoft Store
// package full names (name_version_architecture_resourceid_publisher). Only
// the main package counts, the one with no resource id: the AppX registry
// keeps a resource package (…_neutral_split.scale-125_…) of a version the
// main package has moved past, so one is no sign the app is installed.
func storeAppVersion(packages []string, name, publisher string) string {
	prefix, suffix := name+"_", "_"+publisher
	var newest [4]uint64
	found := ""
	for _, pkg := range packages {
		if !strings.HasPrefix(pkg, prefix) || !strings.HasSuffix(pkg, suffix) {
			continue
		}
		fields := strings.Split(strings.TrimSuffix(strings.TrimPrefix(pkg, prefix), suffix), "_")
		if len(fields) != 3 || fields[2] != "" {
			continue
		}
		parts := strings.Split(fields[0], ".")
		if len(parts) != len(newest) {
			continue
		}
		var version [4]uint64
		valid := true
		for i, part := range parts {
			var err error
			version[i], err = strconv.ParseUint(part, 10, 16)
			if err != nil {
				valid = false
				break
			}
		}
		if valid && (found == "" || slices.Compare(version[:], newest[:]) > 0) {
			newest, found = version, fields[0]
		}
	}
	return found
}

// HasApp reports whether the agent's desktop app is installed here: its
// bundle on macOS (AppVersion), its executable on Windows: under
// %LOCALAPPDATA%\Programs, or in the folder its installer registered.
func (a *Agent) HasApp() bool {
	app, ok := desktopApps[a.ID]
	return ok && a.WSL == "" && app.installed()
}

// installed reports whether app is installed on this machine (see HasApp).
func (app desktopApp) installed() bool {
	if app.version() != "" {
		return true
	}
	if app.win == "" {
		return false
	}
	for _, dir := range appFolders() {
		if isFile(filepath.Join(dir, app.win)) {
			return true
		}
	}
	dir, _ := app.registered()
	return dir != ""
}
