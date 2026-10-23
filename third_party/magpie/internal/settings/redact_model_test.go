package settings

import "testing"

// The masking model is downloaded on one computer: a sync never turns it
// on on another, nor off on this one.
func TestRedactModelIsThisComputers(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	from := Settings{RedactModel: true}
	from.KeepOwn(Settings{})
	if from.RedactModel {
		t.Fatal("a sync turned the model on")
	}
	from.KeepOwn(Settings{RedactModel: true})
	if !from.RedactModel {
		t.Fatal("a sync turned the model off")
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
}
