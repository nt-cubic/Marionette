package davsync

import (
	"testing"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
)

// #1821: an upload with keys from a magpie that knows no access key (one
// before v0.1.1142, or one it was never given on) has the provider's key
// and no AccessKey ID or Secret. The server's are kept while the key is
// the same, so the next computer to sync doesn't lose them; a keyless
// upload keeps the server's secret headers as it keeps its keys.
func TestTakeKeepsAccessKey(t *testing.T) {
	server := provider.Provider{ID: "volcengine", Name: "Old", Chat: "https://ark.cn-beijing.volces.com/api/coding/v3",
		Key: "ark-k", AccessKeyID: "AK-server", SecretAccessKey: "SK-server",
		Headers: map[string]string{"X-Api-Token": "hdr-server"}}
	for _, c := range []struct {
		name           string
		keys           bool
		incoming       provider.Provider
		wantAK, wantHd string
	}{
		{"same key, no access key", true, provider.Provider{Key: "ark-k"}, "AK-server", ""},
		{"another key", true, provider.Provider{Key: "ark-other"}, "", ""},
		{"keyless upload", false, provider.Provider{}, "AK-server", "hdr-server"},
	} {
		t.Run(c.name, func(t *testing.T) {
			in := c.incoming
			in.ID, in.Name, in.Chat = "volcengine", "New", server.Chat
			to := backup.Bundle{Keys: true, Providers: []provider.Provider{server}}
			take(&to, backup.Bundle{Keys: c.keys, Providers: []provider.Provider{in}}, "providers")
			got := to.Providers[0]
			if got.AccessKeyID != c.wantAK || (c.wantAK != "") != (got.SecretAccessKey == "SK-server") || got.Name != "New" {
				t.Errorf("merged %+v, want access key %q", got, c.wantAK)
			}
			if got.Headers["X-Api-Token"] != c.wantHd {
				t.Errorf("headers %v, want %q", got.Headers, c.wantHd)
			}
		})
	}
}
