package autostart

import (
	"testing"

	"github.com/yetone/magpie/internal/testenv"
)

// The tests run in a home of their own, never the user's real files, and
// never ask the system about the user's own login item.
func TestMain(m *testing.M) {
	keepToTheSandbox()
	testenv.Main(m)
}
