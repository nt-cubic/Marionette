package otlp

import (
	"testing"

	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/usage"
)

func TestMain(m *testing.M) { testenv.Main(m) }

// testX is the exporter these tests drive, otel its running half.
var (
	testX = New()
	otel  = &testX.running
)

// StartOTel registers testX as an observer and serves it, as plugins/otel
// and the gateway do.
func StartOTel() func() {
	remove := usage.Observe(testX)
	stop := usage.ServeObservers()
	return func() { stop(); remove() }
}

func OTelSessionAgent(agent string) bool     { return testX.SessionAgent(agent) }
func OTelSession(agent, session string) bool { return testX.Session(agent, session) }
func offerOTel(r Record)                     { testX.Ledger(r) }
