package gui

import (
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/fonts"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

func TestFontDiscoveryCacheAndFailure(t *testing.T) {
	face := fonts.Face{Family: "HarmonyOS Sans SC", Name: "Medium", Weight: 500, Style: "normal", Stretch: 100}
	want := []fonts.Family{{Name: face.Family, Styles: []fonts.Face{face}}}
	calls, broken := 0, false
	mux := http.NewServeMux()
	fontRoutes(mux, false, func() ([]fonts.Family, error) {
		calls++
		if broken {
			return nil, errors.New("font service unavailable")
		}
		return want, nil
	})
	read := func(url string, status int) *httptest.ResponseRecorder {
		t.Helper()
		r := httptest.NewRecorder()
		mux.ServeHTTP(r, httptest.NewRequest("GET", url, nil))
		if r.Code != status {
			t.Fatalf("%s: %d %s", url, r.Code, r.Body)
		}
		return r
	}
	r := read("/api/fonts", 200)
	var got []fonts.Family
	if err := json.Unmarshal(r.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("catalogue = %#v", got)
	}
	read("/api/fonts", 200)
	if calls != 1 {
		t.Fatalf("cached list rediscovered %d times", calls)
	}
	broken = true
	read("/api/fonts?refresh=1", 503)
	if r = read("/api/fonts", 200); !strings.Contains(r.Body.String(), face.Family) {
		t.Fatal("failure erased the cached list")
	}
	broken = false
	read("/api/fonts?refresh=1", 200)
	if calls != 3 {
		t.Fatalf("refresh did not re-read fonts: %d", calls)
	}
	web := http.NewServeMux()
	fontRoutes(web, true, func() ([]fonts.Family, error) { t.Fatal("browser enumerated server fonts"); return nil, nil })
	r = httptest.NewRecorder()
	web.ServeHTTP(r, httptest.NewRequest("GET", "/api/fonts", nil))
	if r.Code != 404 {
		t.Fatalf("web font discovery returned %d", r.Code)
	}
}

func TestFontSettingsAPIAndBoot(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	notices := 0
	onFonts = func() { notices++ }
	t.Cleanup(func() { onFonts = nil })
	h := Handler(nil, nil)
	face := &fonts.Face{Family: `Reader's "中文"`, Name: "Medium", Weight: 500, Style: "normal", Stretch: 100}
	post := func(handler http.Handler, body any, status int) {
		t.Helper()
		b, _ := json.Marshal(body)
		r := httptest.NewRecorder()
		handler.ServeHTTP(r, httptest.NewRequest("POST", "/api/settings", strings.NewReader(string(b))))
		if r.Code != status {
			t.Fatalf("settings returned %d: %s", r.Code, r.Body)
		}
	}
	post(h, map[string]any{"uiFont": face, "codeFont": face}, 200)
	post(h, map[string]any{"theme": "dark", "lang": "zh"}, 200)
	if notices != 1 {
		t.Fatalf("font change notifications = %d, want 1", notices)
	}
	if !reflect.DeepEqual(settings.Load().UIFont, face) || !reflect.DeepEqual(settings.Load().CodeFont, face) {
		t.Fatal("an unrelated settings save lost the fonts")
	}
	boot := func(handler http.Handler) map[string]json.RawMessage {
		t.Helper()
		r := httptest.NewRecorder()
		handler.ServeHTTP(r, httptest.NewRequest("GET", "/boot.js", nil))
		body := strings.TrimSuffix(strings.TrimPrefix(r.Body.String(), "window.bootPrefs = "), ";\n")
		var got map[string]json.RawMessage
		if err := json.Unmarshal([]byte(body), &got); err != nil {
			t.Fatal(err)
		}
		return got
	}
	var first fonts.Face
	if err := json.Unmarshal(boot(h)["uiFont"], &first); err != nil || !reflect.DeepEqual(&first, face) {
		t.Fatalf("first paint font = %#v: %v", first, err)
	}
	web := Handler(webHost{}, nil)
	post(web, map[string]any{"uiFont": nil, "codeFont": nil}, 200)
	if !reflect.DeepEqual(settings.Load().UIFont, face) {
		t.Fatal("browser save replaced a desktop font")
	}
	if _, ok := boot(web)["uiFont"]; ok {
		t.Fatal("browser received a desktop font override")
	}
	post(h, map[string]any{"uiFont": map[string]any{"family": "bad", "name": "Regular", "weight": 1001}}, 400)
	if !reflect.DeepEqual(settings.Load().UIFont, face) {
		t.Fatal("invalid choice replaced the saved font")
	}
	post(h, map[string]any{"uiFont": nil}, 200)
	if notices != 2 {
		t.Fatalf("only successful font changes should notify: %d", notices)
	}
	if settings.Load().UIFont != nil || !reflect.DeepEqual(settings.Load().CodeFont, face) {
		t.Fatal("reset did not affect just the interface font")
	}
}
