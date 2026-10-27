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
