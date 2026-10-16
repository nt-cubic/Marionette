package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/usage"
)

func TestUsageRequestAPIsReturnCodexCompactionStates(t *testing.T) {
	sandboxHome(t)
	now := holdUsageClock(t, time.Date(2026, 9, 30, 12, 0, 0, 0, time.UTC))
	at := now.Add(-time.Hour)
	for _, row := range []struct {
		id string
		at time.Time
	}{
		{"resp-api-true", at},
		{"resp-api-false", at.Add(time.Second)},
		{"resp-api-orphan", at.Add(2 * time.Second)},
	} {
		usage.Append(usage.Record{Time: row.at, Agent: "codex", Provider: "openai", Model: "api-model", Effort: "gateway-effort", Kind: "chat",
			Input: 8, Output: 2, Status: 200, ResponseID: row.id})
	}
	calls := []sessions.Call{
		{Time: at, Agent: "codex", ThreadID: "fixture-thread", Session: "fixture-session", ResponseID: "resp-api-true", Model: "native-model", Compaction: true},
		{Time: at.Add(time.Second), Agent: "codex", ThreadID: "fixture-thread", Session: "fixture-session", ResponseID: "resp-api-false", Model: "native-model"},
	}
	old := usage.LogCalls
	usage.LogCalls = func(time.Time) []sessions.Call { return calls }
	t.Cleanup(func() { usage.LogCalls = old })

	guiMux := http.NewServeMux()
	usageRoutes(guiMux, folderOnly{})
	responses := map[string]*httptest.ResponseRecorder{
		"GUI":     httptest.NewRecorder(),
		"gateway": httptest.NewRecorder(),
	}
	guiMux.ServeHTTP(responses["GUI"], httptest.NewRequest("GET", "/api/usage/requests?period=all", nil))
	gatewayRequest := httptest.NewRequest("GET", "/v1/magpie/usage/requests?period=all", nil)
	gatewayRequest.RemoteAddr = "127.0.0.1:5000"
	gateway.New().Handler().ServeHTTP(responses["gateway"], gatewayRequest)
	for name, response := range responses {
		if response.Code != http.StatusOK {
			t.Fatalf("%s request API: %d %s", name, response.Code, response.Body)
		}
		var page ledgerJSON
		if err := json.Unmarshal(response.Body.Bytes(), &page); err != nil {
			t.Fatalf("%s request API JSON: %v: %s", name, err, response.Body)
		}
		rows := map[string]ledgerRow{}
		for _, row := range page.Rows {
			rows[row.ResponseID] = row
		}
		for id, want := range map[string]bool{"resp-api-true": true, "resp-api-false": false} {
			row, ok := rows[id]
			if !ok || row.Compaction == nil || *row.Compaction != want {
				t.Errorf("%s %s compaction = %+v (found=%v), want known %v", name, id, row.Compaction, ok, want)
			}
			if ok && (row.Model != "api-model" || row.Effort != "gateway-effort" || row.Kind != "chat" || row.Input != 8 || row.Output != 2) {
				t.Errorf("%s %s lost gateway details: %+v", name, id, row)
			}
		}
		if row, ok := rows["resp-api-orphan"]; !ok || row.Compaction != nil {
			t.Errorf("%s unmatched gateway row claims native evidence: %+v, found=%v", name, row.Compaction, ok)
		}
	}
}
