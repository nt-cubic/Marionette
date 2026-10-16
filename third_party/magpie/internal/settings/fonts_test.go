package settings

import (
	"reflect"
	"testing"

	"github.com/yetone/magpie/internal/fonts"
	"github.com/yetone/magpie/internal/testenv"
)

func TestFontChoicesPersistAndStayLocal(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	ui := &fonts.Face{Family: "HarmonyOS Sans SC", Name: "Medium", Weight: 500, Style: "normal", Stretch: 100}
	code := &fonts.Face{Family: "Consolas", Name: "Italic", Weight: 400, Style: "italic", Stretch: 100}
	if err := Save(Settings{UIFont: ui, CodeFont: code}); err != nil {
		t.Fatal(err)
	}
	got := Load()
	if !reflect.DeepEqual(got.UIFont, ui) || !reflect.DeepEqual(got.CodeFont, code) {
		t.Fatalf("font choices lost: %#v %#v", got.UIFont, got.CodeFont)
	}
	other := Settings{Theme: "dark"}
	other.KeepOwn(got)
	if !reflect.DeepEqual(other.UIFont, ui) || !reflect.DeepEqual(other.CodeFont, code) {
		t.Fatal("sync replaced this computer's fonts")
	}
	other.KeepOwn(Settings{})
	if other.UIFont != nil || other.CodeFont != nil {
		t.Fatal("sync imported fonts into a computer using defaults")
	}
	bad := *ui
	bad.Weight = 1001
	if Save(Settings{UIFont: &bad}) == nil {
		t.Fatal("saved an invalid font weight")
	}
	if !reflect.DeepEqual(Load().UIFont, ui) {
		t.Fatal("a rejected choice overwrote the saved font")
	}
	if err := Save(Settings{}); err != nil {
		t.Fatal(err)
	}
	if Load().UIFont != nil || Load().CodeFont != nil {
		t.Fatal("default fonts did not persist")
	}
}
