package gui

import "fmt"

// magpie's windows are WebView2 on Windows, a runtime Windows 10 doesn't
// always have. Without it Wails hands the failure to ErrorHandler and exits,
// and the desktop build has no console for that to land in: a double-click
// came up to nothing (#1806, ufec). Run asks before any window is made and,
// missing, says so in a box of Windows' own.

// webView2Page is Microsoft's download page for the runtime.
const webView2Page = "https://developer.microsoft.com/microsoft-edge/webview2/"

// webView2Missing reads Wails' own answer, Env.Info().PlatformInfo's
// "WebView2": the runtime's version, "" when its lookup failed, the lookup
// a window's WebView2 environment makes. A key Wails doesn't set at all
// says nothing, and is read as there: refusing to start a magpie that would
// have worked is worse than the silent exit.
func webView2Missing(info map[string]any) bool {
	v, ok := info["WebView2"]
	return ok && fmt.Sprint(v) == ""
}

// webView2Notice is the box's title and text in lang (trayLang's), English
// where it has none. It names the page and winget's package (#1806: ufec
// asked for the winget way too), and asks whether to open the page; Yes
// opens it.
func webView2Notice(lang string) (title, body string) {
	t, ok := webView2Words[lang]
	if !ok {
		t = webView2Words["en"]
	}
	return t[0], fmt.Sprintf(t[1], webView2Page)
}

var webView2Words = map[string][2]string{
	"en": {"magpie needs WebView2",
		"magpie's windows need Microsoft Edge WebView2 Runtime, which isn't installed on this computer.\r\n\r\n" +
			"Install it from:\r\n%s\r\n\r\nor in a terminal:\r\nwinget install Microsoft.EdgeWebView2Runtime\r\n\r\n" +
			"Until then, \"magpie web\" opens magpie in your browser and \"magpie tui\" in a terminal.\r\n\r\n" +
			"Open the download page now?"},
	"zh": {"magpie 需要 WebView2",
		"magpie 的窗口需要 Microsoft Edge WebView2 运行时，这台电脑上没有安装。\r\n\r\n" +
			"请从这里安装：\r\n%s\r\n\r\n或在终端里运行：\r\nwinget install Microsoft.EdgeWebView2Runtime\r\n\r\n" +
			"装好之前，可以用 \"magpie web\" 在浏览器里打开 magpie，或用 \"magpie tui\" 在终端里打开。\r\n\r\n" +
			"现在打开下载页面吗？"},
	"zh-TW": {"magpie 需要 WebView2",
		"magpie 的視窗需要 Microsoft Edge WebView2 執行階段，這台電腦上沒有安裝。\r\n\r\n" +
			"請從這裡安裝：\r\n%s\r\n\r\n或在終端機裡執行：\r\nwinget install Microsoft.EdgeWebView2Runtime\r\n\r\n" +
			"裝好之前，可以用 \"magpie web\" 在瀏覽器裡開啟 magpie，或用 \"magpie tui\" 在終端機裡開啟。\r\n\r\n" +
			"現在開啟下載頁面嗎？"},
	"ja": {"magpie には WebView2 が必要です",
		"magpie のウィンドウには Microsoft Edge WebView2 ランタイムが必要ですが、このコンピューターにはインストールされていません。\r\n\r\n" +
			"こちらからインストールしてください：\r\n%s\r\n\r\nまたはターミナルで：\r\nwinget install Microsoft.EdgeWebView2Runtime\r\n\r\n" +
			"それまでは \"magpie web\" でブラウザーから、\"magpie tui\" でターミナルから magpie を使えます。\r\n\r\n" +
			"今すぐダウンロードページを開きますか？"},
	"de": {"magpie braucht WebView2",
		"Die Fenster von magpie brauchen die Microsoft Edge WebView2 Runtime, die auf diesem Computer nicht installiert ist.\r\n\r\n" +
			"Installieren Sie sie von:\r\n%s\r\n\r\noder im Terminal:\r\nwinget install Microsoft.EdgeWebView2Runtime\r\n\r\n" +
			"Bis dahin öffnet \"magpie web\" magpie im Browser und \"magpie tui\" im Terminal.\r\n\r\n" +
			"Die Download-Seite jetzt öffnen?"},
}
