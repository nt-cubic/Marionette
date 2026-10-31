package update

import (
	"os"
	"path/filepath"
	ptr "unsafe"

	"golang.org/x/sys/windows"

	"github.com/yetone/magpie/internal/appdir"
)

// installPath is %LOCALAPPDATA%\Programs\magpie\magpie.exe: the user's own
// programs folder (FOLDERID_UserProgramFiles), where per-user installers
// put their apps, which needs no administrator.
func installPath() string {
	local := appdir.Getenv("LOCALAPPDATA")
	if local == "" {
		if p, err := windows.KnownFolderPath(windows.FOLDERID_UserProgramFiles, 0); err == nil && p != "" {
			return filepath.Join(p, "magpie", "magpie.exe")
		}
		return ""
	}
	return filepath.Join(local, "Programs", "magpie", "magpie.exe")
}

// isDownloadFolder: Downloads and Desktop, where they are kept (the
// known folders, which OneDrive or the user may have moved) and under the
// home folder, and any temporary folder, where Explorer opens an exe from
// inside a zip. A folder of the user's own, even one inside these, isn't.
func isDownloadFolder(dir string) bool {
	var dirs []string
	if h, err := appdir.Home(); err == nil {
		dirs = append(dirs, filepath.Join(h, "Downloads"), filepath.Join(h, "Desktop"))
	}
	for _, id := range []*windows.KNOWNFOLDERID{windows.FOLDERID_Downloads, windows.FOLDERID_Desktop} {
		if p, err := windows.KnownFolderPath(id, 0); err == nil && p != "" {
			dirs = append(dirs, p)
		}
	}
	for _, d := range dirs {
		if samePath(dir, d) {
			return true
		}
	}
	for _, t := range []string{os.TempDir(), appdir.Getenv("TEMP"), appdir.Getenv("TMP")} {
		if inFolder(dir, t) {
			return true
		}
	}
	return false
}

// exeVersion is the file version in the exe's resources (make
// release-windows writes the release's there), as one number to compare:
// 0 when it has none, as a build of one's own.
func exeVersion(path string) uint64 {
	n, err := windows.GetFileVersionInfoSize(path, nil)
	if err != nil || n == 0 {
		return 0
	}
	buf := make([]byte, n)
	if err := windows.GetFileVersionInfo(path, 0, n, ptr.Pointer(&buf[0])); err != nil {
		return 0
	}
	var fixed *windows.VS_FIXEDFILEINFO
	var size uint32
	if err := windows.VerQueryValue(ptr.Pointer(&buf[0]), `\`, ptr.Pointer(&fixed), &size); err != nil || fixed == nil || size == 0 {
		return 0
	}
	return uint64(fixed.FileVersionMS)<<32 | uint64(fixed.FileVersionLS)
}

// webViewDefault is where WebView2 keeps the profile of the program at
// exe when magpie names none: %APPDATA%\<exe name> (Wails' default).
func webViewDefault(exe string) string {
	a := os.Getenv("AppData")
	if a == "" {
		return ""
	}
	return filepath.Join(a, filepath.Base(exe))
}
