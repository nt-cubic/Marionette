package cli

import (
	"net/http"
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/testenv"
)

// The tests run in a home of their own, never the user's real files, and
// off the network (testenv.Offline): the plugin tests start plugin hosts,
// and a host started in a home without a models.dev catalog fetched the
// real one for up to 30s (#1922).
func TestMain(m *testing.M) {
	os.Exit(testenv.RunIn(m, func(string) { testenv.Offline() }))
}

// A host start's models.dev fetch is refused at once, never sent (#1922).
func TestStaysOffTheNetwork(t *testing.T) {
	resp, err := http.Get("https://models.dev/api.json")
	if err == nil {
		resp.Body.Close()
		t.Fatalf("models.dev answered %s: the package's tests reach the network", resp.Status)
	}
	if !strings.Contains(err.Error(), "tests stay off the network") {
		t.Fatalf("models.dev: %v, not testenv's refusal", err)
	}
}
