package update

import (
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/yetone/magpie/internal/appdir"
)

// The Windows app is one exe with no installer, and the site's download
// lands it in Downloads as magpie-windows-amd64.exe. Left there, the Start
// menu shortcut, Open at login and the `magpie` command all name a file
// the user tidies away, and the command came from a magpie.com written
// beside it in Downloads (lucky_eagle_38410 on Discord). So the desktop
// app, opened from a folder downloads land in, installs itself the way a
// Mac app is moved into Applications: it copies itself to
// %LOCALAPPDATA%\Programs\magpie\magpie.exe — the user's own programs
// folder, no administrator — puts that folder on the user's PATH, and
// opens the copy there, which makes the shortcut, Open at login and its
// updates name it. The download is left where it is: it is the user's
// file. Opened again, it opens the installed copy, or puts itself there
// first when it is the newer version.
//
// It is left alone, and runs where it is as before, when it runs from
// anywhere else (a folder the user chose: D:\Tools, a USB drive), when it
// is portable (a data folder or .portable file beside it, appdir), or
// with MAGPIE_NO_INSTALL=1.

// NoInstallEnv, set to 1, keeps a magpie run from Downloads there.
const NoInstallEnv = "MAGPIE_NO_INSTALL"

// installedFromEnv names, for the installed copy started in its place,
// the exe it was installed from, whose leftovers it clears (Settled).
const installedFromEnv = "MAGPIE_INSTALLED_FROM"

// What Settle asks of the system; tests set them.
var (
	// installTarget is %LOCALAPPDATA%\Programs\magpie\magpie.exe, "" off
	// Windows
	installTarget = installPath
	// downloadFolder says whether dir is one downloads land in:
	// Downloads, the Desktop or a temporary folder (an exe opened from
	// inside a zip); not a folder inside them, the home folder or
	// Documents
	downloadFolder = isDownloadFolder
	// fileVersion is the version in an exe's resources, 0 when unknown
	fileVersion = exeVersion
	// portableData is a portable magpie's data folder, "" installed
	portableData = appdir.Portable
	// webViewHome is the folder WebView2 keeps an installed magpie's
	// profile in by default, %APPDATA%\<exe name>; "" off Windows
	webViewHome = webViewDefault
	// installCLI puts the installed folder on the user's PATH with the
	// command a terminal runs there
	installCLI = cliPlace
)

// Settle installs the desktop app run from a download folder (see
// above), and says what to open instead of running on here: from is this
// exe and to the installed one, both "" when this one runs on. Anything
// that goes wrong leaves this one running, as before.
func Settle() (from, to string) {
	if os.Getenv(NoInstallEnv) == "1" {
		return "", ""
	}
	exe, err := Executable()
	if err != nil {
		return "", ""
	}
	to, err = settle(exe)
	if err != nil {
		log.Println("install:", err)
		return "", ""
	}
	if to == "" {
		return "", ""
	}
	return exe, to
}

// settle is Settle for exe: the installed exe to open instead, "" to run
// exe where it is.
func settle(exe string) (string, error) {
	to := installTarget()
	if to == "" || samePath(exe, to) || !downloadFolder(filepath.Dir(exe)) || portableData() != "" {
		return "", nil
	}
	if !exists(to) {
		if err := installFresh(exe, to); err != nil {
			return "", err
		}
		return to, nil
	}
	if sameFile(exe, to) || sameBytes(exe, to) {
		return to, nil
	}
	mine, theirs := fileVersion(exe), fileVersion(to)
	if mine == 0 || theirs == 0 || mine <= theirs {
		// the installed copy is as new or newer, or it can't be told:
		// that one is kept, and updates itself
		return to, nil
	}
	// a newer version, downloaded again: it goes in place of the
	// installed one, as an update would
	staged, err := copyBeside(exe, to)
	if err != nil {
		return "", err
	}
	if _, err := InstallBinaryAside(staged, to); err != nil {
		os.Remove(staged)
		return "", fmt.Errorf("putting %s in place of %s: %w", filepath.Base(exe), to, err)
	}
	return to, nil
}

// installFresh puts a copy of exe at to, puts its folder on the user's
// PATH with the `magpie` command a terminal runs there (cliPlace), and
// points the installed copy at the webview profile exe had, which is kept
// by its file name.
func installFresh(exe, to string) error {
	dir := filepath.Dir(to)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	staged, err := copyBeside(exe, to)
	if err != nil {
		return err
	}
	if err := moveIn(staged, to); err != nil {
		os.Remove(staged)
		return fmt.Errorf("installing to %s: %w", to, err)
	}
	keepWebView(exe, to)
	if err := installCLI(to, dir); err != nil {
		// the app runs the same without the command; Settings' Command
		// line says it isn't there and offers it again
		log.Println("install: the magpie command:", err)
	}
	return nil
}

// keepWebView has the copy at to keep the webview profile exe had: the
// pages' storage there would be lost to a new profile under magpie.exe's
// name. Nothing when that one has a profile already.
func keepWebView(exe, to string) {
	home := webViewHome
	old, now := home(exe), home(to)
	if old == "" || now == "" || samePath(old, now) || exists(now) || !exists(old) {
		return
	}
	if err := os.WriteFile(filepath.Join(filepath.Dir(to), appdir.WebViewNote), []byte(old+"\n"), 0o644); err != nil {
		log.Println("install: the webview profile:", err)
	}
}

// copyBeside copies exe into a new file in to's folder, ready to be moved
// in at to; the file is its path.
func copyBeside(exe, to string) (string, error) {
	src, err := os.Open(exe)
	if err != nil {
		return "", err
	}
	defer src.Close()
	f, err := os.CreateTemp(filepath.Dir(to), ".magpie-install-*")
	if err != nil {
		return "", err
	}
	_, err = io.Copy(f, src)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		os.Remove(f.Name())
		return "", fmt.Errorf("copying %s to %s: %w", filepath.Base(exe), filepath.Dir(to), err)
	}
	return f.Name(), nil
}

// Forward opens the installed magpie at to with this one's arguments, and
// waits for it to say it is up; the caller then quits. A restart to
// update waits on this one for that word (MAGPIE_STARTED): it is passed on
// once the installed one has said it. When to can't be started, this one
// runs on.
func Forward(from, to string) error {
	marker := filepath.Join(os.TempDir(), fmt.Sprintf("magpie-installed-%d", os.Getpid()))
	os.Remove(marker)
	defer os.Remove(marker)
	var env []string
	for _, e := range os.Environ() {
		if !strings.HasPrefix(e, startedEnv+"=") && !strings.HasPrefix(e, installedFromEnv+"=") {
			env = append(env, e)
		}
	}
	env = append(env, startedEnv+"="+marker, installedFromEnv+"="+from)
	exited, kill, err := launch(to, os.Args[1:], env)
	if err == nil {
		err = awaitStarted(marker, exited, kill)
	}
	if err != nil {
		return fmt.Errorf("opening %s: %w", to, err)
	}
	reportStarted()
	return nil
}

// Settled clears, in the installed magpie Forward opened, what magpie left
// beside the exe it was installed from: what updates moved aside there,
// an update downloaded for it (magpie.exe.new), and the magpie.com or magpie.cmd magpie wrote there to run it, which
// the `magpie` on PATH is now. A file it didn't write, or wrote for
// another program, is left.
func Settled() {
	from := os.Getenv(installedFromEnv)
	os.Unsetenv(installedFromEnv)
	if from == "" || !filepath.IsAbs(from) {
		return
	}
	clearBeside(from)
}

func clearBeside(from string) {
	RemoveOld(from)
	// an update the download had fetched is never put in now: the
	// installed one updates itself
	removeStaged(from)
	dir := filepath.Dir(from)
	if p := filepath.Join(dir, comName); comRuns(p, from) {
		RemoveOld(p)
		if err := os.Remove(p); err != nil && !os.IsNotExist(err) {
			log.Println("install: removing", p+":", err)
		}
	}
	if p := filepath.Join(dir, shimName); shimRuns(p, from) {
		if err := os.Remove(p); err != nil && !os.IsNotExist(err) {
			log.Println("install: removing", p+":", err)
		}
	}
}

// forwards says whether exe is a download Settle opens the installed
// magpie in place of: nothing is written beside it for a terminal, which
// runs the installed one.
func forwards(exe string) bool {
	if runtime.GOOS != "windows" || os.Getenv(NoInstallEnv) == "1" {
		return false
	}
	to := installTarget()
	return to != "" && !samePath(exe, to) && downloadFolder(filepath.Dir(exe)) && portableData() == "" && exists(to)
}

// samePath says whether two paths name the same file, Windows' way: in
// any case.
func samePath(a, b string) bool {
	return strings.EqualFold(filepath.Clean(a), filepath.Clean(b))
}

// sameBytes says whether the files at a and b hold the same bytes.
func sameBytes(a, b string) bool {
	sa, err1 := os.Stat(a)
	sb, err2 := os.Stat(b)
	if err1 != nil || err2 != nil || sa.Size() != sb.Size() {
		return false
	}
	ha, hb := fileHash(a), fileHash(b)
	return ha != "" && ha == hb
}

// inFolder says whether path is dir or lies in it, in any case.
func inFolder(path, dir string) bool {
	if dir == "" || !filepath.IsAbs(dir) {
		return false
	}
	rel, err := filepath.Rel(strings.ToLower(filepath.Clean(dir)), strings.ToLower(filepath.Clean(path)))
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}
