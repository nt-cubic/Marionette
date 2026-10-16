package provider

import (
	"encoding/json"
	"testing"
)

// A magpie from before #2227 answers with "limit" alone: its card is
// that one limit's windows, as before.
func TestKeyLimitCardOneLimit(t *testing.T) {
	var a remoteKeyAnswer
	if err := json.Unmarshal([]byte(`{"limited":true,"key":"Laptop","limit":{"period":"day","tokens":350,"tokenLimit":1000,"tokensLeft":650,"cost":1.5,"costLimit":5,"costLeft":3.5}}`), &a); err != nil {
		t.Fatal(err)
	}
	g, ws := keyLimitCard(&a)
	if !g.Limited || g.Period != "day" || *g.TokensLeft != 650 || *g.CostLeft != 3.5 || len(ws) != 2 || ws[0].Name != "Tokens today" || ws[1].Name != "Spend today" || ws[1].Amount != 1.5 {
		t.Fatalf("one limit: %+v %+v", g, ws)
	}
}
