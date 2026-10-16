package usage

import (
	"net/http"
	"slices"
	"sync"
)

// Observer is the gateway's observer point (P3 of
// docs/subsystems/plugin-runtime.md): it watches what the gateway serves
// and what the ledger is written, and never changes either. A plugin
// registers one with Observe while it is on (plugins/otel registers
// internal/otlp's exporter); with none registered the gateway does no
// observing work at all, not even keeping a reply's whole body.
//
// It lives here, not in the gateway, because every row it sees is a usage
// Record and the ledger's other writers (budget, routing decisions) reach
// it too.
type Observer interface {
	// Serve runs it while this process serves the gateway; stop ends it,
	// after the calls in flight have finished
	Serve() (stop func())
	// Bodies is how much of a request's bodies it wants now
	Bodies() Bodies
	// Begin is asked as each gateway request starts; nil leaves the
	// request unwatched by it
	Begin(Request) Watch
	// Ledger is each row Append writes. A row of a request some observer
	// watched has Watched set.
	Ledger(Record)
}

// Watch is one request an Observer watches.
type Watch interface {
	// Attempt is each routing attempt, unbilled failures and retries
	// included, timed as the attempt and not the whole request
	Attempt(Record)
	// End is the request's end: its time, agent, model, status and length
	End(Record)
}

// Request is what an Observer is told of a request as it starts.
type Request struct {
	Header http.Header
	// Agent the agent that sent it, Session its native session when it
	// names one, else magpie's
	Agent, Session string
	// Kind its call kind ("web_search", "vision", …), "auxiliary" for
	// Claude's tool-less small helper calls, "" for an agent's own turn
	Kind string
	// Plain is a loopback request with no gateway key, not passed on from
	// another magpie: one the agent's own transcript can hold
	Plain bool
}

// Bodies is how much of a request's and reply's bodies an observer wants.
type Bodies int

const (
	NoBodies       Bodies = iota
	CapturedBodies        // the first 256 KiB Recent calls keeps
	WholeBodies           // uncut: the reply is spooled as it streams
)

var (
	obsMu     sync.Mutex
	observers []*observing
	serving   int // ServeObservers calls not yet stopped
)

type observing struct {
	o    Observer
	stop func() // while serving
}

// Observe registers o until remove is called. While this process serves
// the gateway it is started at once.
func Observe(o Observer) (remove func()) {
	obsMu.Lock()
	ob := &observing{o: o}
	if serving > 0 {
		ob.stop = o.Serve()
	}
	observers = append(observers, ob)
	obsMu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			obsMu.Lock()
			observers = slices.DeleteFunc(slices.Clone(observers), func(x *observing) bool { return x == ob })
			stop := ob.stop
			ob.stop = nil
			obsMu.Unlock()
			if stop != nil {
				stop()
			}
		})
	}
}

// ServeObservers starts every observer registered, and each one
// registered later, until stop: the gateway calls it as it starts serving.
func ServeObservers() (stop func()) {
	obsMu.Lock()
	serving++
	if serving == 1 {
		for _, ob := range observers {
			ob.stop = ob.o.Serve()
		}
	}
	obsMu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			obsMu.Lock()
			serving--
			var stops []func()
			if serving == 0 {
				for _, ob := range observers {
					if ob.stop != nil {
						stops = append(stops, ob.stop)
						ob.stop = nil
					}
				}
			}
			obsMu.Unlock()
			for _, stop := range stops {
				stop()
			}
		})
	}
}

// Observers are the observers registered now.
func Observers() []Observer {
	obsMu.Lock()
	defer obsMu.Unlock()
	out := make([]Observer, len(observers))
	for i, ob := range observers {
		out[i] = ob.o
	}
	return out
}

// ObservedBodies is the most of a request's bodies any observer wants now.
func ObservedBodies() Bodies {
	b := NoBodies
	for _, o := range Observers() {
		b = max(b, o.Bodies())
	}
	return b
}

func observeLedger(r Record) {
	for _, o := range Observers() {
		o.Ledger(r)
	}
}

// BodyCut ends a body the gateway captured only the start of.
const BodyCut = "\n… (cut here by magpie)"
