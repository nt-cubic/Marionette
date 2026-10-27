package masking

import (
	"testing"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/settings"
)

// Masking asks the model the user picked; with none picked, Privacy
// Filter, which was the only one when the model was turned on.
func TestMaskingUsesThePickedModel(t *testing.T) {
	for name, want := range map[string]string{"": pii.PrivacyFilter, pii.PrivacyFilter: pii.PrivacyFilter, pii.AstrLinkGuard: pii.AstrLinkGuard} {
		if got := picked(settings.Settings{RedactModel: true, RedactModelName: name}).ID(); got != want {
			t.Errorf("picked %q: %s, want %s", name, got, want)
		}
	}
}
