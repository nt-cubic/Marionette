package settings

import "testing"

// The masking model is downloaded on one computer: a sync never turns it
// on on another, nor off on this one.
func TestRedactModelIsThisComputers(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	from := Settings{RedactModel: true, RedactModelName: "astrlink-guard"}
	from.KeepOwn(Settings{})
	if from.RedactModel || from.RedactModelName != "" {
		t.Fatal("a sync turned the model on")
	}
	from.KeepOwn(Settings{RedactModel: true, RedactModelName: "privacy-filter"})
	if !from.RedactModel || from.RedactModelName != "privacy-filter" {
		t.Fatal("a sync turned the model off, or changed it")
	}
	if err := SetRedactModel(true); err != nil {
		t.Fatal(err)
	}
	if !Load().RedactModel {
		t.Fatal("not saved")
	}
	if err := SetRedactModel(false); err != nil {
		t.Fatal(err)
	}
	if Load().RedactModel {
		t.Fatal("not turned off")
	}
	if err := UseRedactModel("astrlink-guard"); err != nil {
		t.Fatal(err)
	}
	if st := Load(); !st.RedactModel || st.RedactModelName != "astrlink-guard" {
		t.Fatalf("not saved: %v %q", st.RedactModel, st.RedactModelName)
	}
	// turned off, the model picked is kept for turning it on again
	if err := SetRedactModel(false); err != nil {
		t.Fatal(err)
	}
	if st := Load(); st.RedactModel || st.RedactModelName != "astrlink-guard" {
		t.Fatalf("off: %v %q", st.RedactModel, st.RedactModelName)
	}
}

// Off is the user's own, which the model on by default never turns on
// again; the default's download, done, is saved as the default's only
// where the user neither picked a model nor turned it off meanwhile.
func TestRedactModelOffIsTheUsers(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if ok, err := UseDefaultRedactModel("astrlink-guard"); !ok || err != nil {
		t.Fatalf("default not saved: %v %v", ok, err)
	}
	if st := Load(); st.RedactModel || !st.RedactModelAuto || st.RedactModelName != "astrlink-guard" {
		t.Fatalf("default: %+v", st)
	}
	if err := SetRedactModel(false); err != nil {
		t.Fatal(err)
	}
	if st := Load(); st.RedactModel || !st.RedactModelOff {
		t.Fatalf("off: %v %v", st.RedactModel, st.RedactModelOff)
	}
	if ok, _ := UseDefaultRedactModel("astrlink-guard"); ok {
		t.Fatal("the default turned on what the user turned off")
	}
	if err := UseRedactModel("privacy-filter"); err != nil {
		t.Fatal(err)
	}
	if st := Load(); !st.RedactModel || st.RedactModelOff {
		t.Fatalf("on: %v %v", st.RedactModel, st.RedactModelOff)
	}
	if ok, _ := UseDefaultRedactModel("astrlink-guard"); ok || Load().RedactModelName != "privacy-filter" {
		t.Fatal("the default changed the user's pick")
	}
	from := Settings{RedactModelOff: true, RedactModelAuto: true}
	from.KeepOwn(Settings{})
	if from.RedactModelOff || from.RedactModelAuto {
		t.Fatal("a sync turned the model off, or on by default")
	}
}
