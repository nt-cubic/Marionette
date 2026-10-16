package otlp

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"io"
	"log"
	"net/http"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/yetone/magpie/internal/netproxy"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

const otelQueueSize = 128
const otelBatchSize = 32
const otelBodiesBatch = 4

// otelQueueBytes is how much of the queued records' bodies may be held at
// once, so a whole body (#538) can't fill memory: a record past it is
// dropped as the queue's own overflow is.
const otelQueueBytes = 128 << 20

// otelBodiesBatchBytes is how much of one request's bodies go in one OTLP
// POST at most, so a batch of whole bodies stays inside a collector's limit.
const otelBodiesBatchBytes = 16 << 20

// Record is a usage row as the exporter sends it.
type Record = usage.Record

// Exporter sends the gateway's requests, the ledger's rows and the agents'
// sessions to an OpenTelemetry collector, as settings.OTel says. It is a
// usage.Observer: plugins/otel registers one while it is on, and magpie's
// core never exports on its own.
type Exporter struct {
	running atomic.Pointer[otelExporter]
}

// New is an exporter, not yet serving.
func New() *Exporter { return &Exporter{} }

type otelItem struct {
	record Record
	span   *OTelSpan // nil for a ledger row
	config settings.OTel
}

type otelExporter struct {
	queue      chan otelItem
	stop, done chan struct{}
	ctx        context.Context
	cancel     context.CancelFunc
	client     *http.Client
	salt       [8]byte
	last       time.Time
	dropped    atomic.Uint64
	bytes      atomic.Int64 // the queued records' bodies, in bytes
	sessions   atomic.Pointer[sessionAvailability]
	identities sessions.TraceSessionIndex
}

// OTelEvery is how often the exporter sends what it holds, and
// OTelSessionsEvery how often it reads the agents' transcripts for
// sessions. Read once when Serve starts an exporter; tests shorten them
// so a test that waits for an export doesn't wait seconds.
var OTelEvery, OTelSessionsEvery = 5 * time.Second, 2 * time.Second

// Serve runs only in the process serving the gateway
// (usage.ServeObservers). Stopping drains accepted metadata for at most
// three seconds, after in-flight calls finish.
func (x *Exporter) Serve() func() {
	e := newOTelExporter()
	x.running.Store(e)
	if _, err := settings.OTelExport(); err != nil {
		log.Printf("otel: %s", err)
	}
	watchCtx, stopWatch := context.WithCancel(e.ctx)
	go e.watchSessions(watchCtx, time.Now(), OTelSessionsEvery)
	go e.run(OTelEvery)
	var once sync.Once
	return func() {
		once.Do(func() {
			stopWatch()
			x.running.CompareAndSwap(e, nil)
			close(e.stop)
			timer := time.NewTimer(3 * time.Second)
			defer timer.Stop()
			select {
			case <-e.done:
			case <-timer.C:
				e.cancel()
				<-e.done
			}
		})
	}
}

func newOTelExporter() *otelExporter {
	ctx, cancel := context.WithCancel(context.Background())
	e := &otelExporter{queue: make(chan otelItem, otelQueueSize), stop: make(chan struct{}), done: make(chan struct{}), ctx: ctx, cancel: cancel,
		last: time.Now(), client: &http.Client{Timeout: 3 * time.Second,
			Transport:     &http.Transport{Proxy: netproxy.Func, MaxIdleConnsPerHost: 2, IdleConnTimeout: 90 * time.Second},
			CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}}
	rand.Read(e.salt[:])
	return e
}

// enabled is whether it serves and settings say to export.
func (x *Exporter) enabled() bool {
	if x.running.Load() == nil {
		return false
	}
	config, err := settings.OTelExport()
	return err == nil && config.Enabled
}

// Ledger offers a usage row. One of a request the gateway sent as spans
// (Watched), or a local agent's turn its own transcript sends, is passed
// over.
func (x *Exporter) Ledger(r Record) {
	if r.Watched {
		return
	}
	e := x.running.Load()
	if e == nil {
		return
	}
	config, err := settings.OTelExport()
	if err != nil || !config.Enabled {
		return
	}
	session := r.NativeSession
	if session == "" {
		session = r.Session
	}
	if r.Local && r.CallerKeyID == "" && r.Via == "" && r.Kind == "" && e.sessionObserved(r.Agent, session, config) {
		return
	}
	e.offerWith(r, nil, config)
}

// offerSpan offers one of a watched request's spans.
func (x *Exporter) offerSpan(r Record, span *OTelSpan) {
	e := x.running.Load()
	if e == nil {
		return
	}
	config, err := settings.OTelExport()
	if err != nil || !config.Enabled {
		return
	}
	e.offerWith(r, span, config)
}

func (e *otelExporter) offerWith(r Record, span *OTelSpan, config settings.OTel) {
	if !config.Bodies {
		r.BodyIn, r.BodyOut = "", ""
	}
	e.offer(otelItem{record: r, span: span, config: config})
}

// Bodies is how much of the bodies go with the spans (#538): the gateway
// fills Record.BodyIn/BodyOut only for some, and keeps a reply whole for
// it only for whole.
func (x *Exporter) Bodies() usage.Bodies {
	if x.running.Load() == nil {
		return usage.NoBodies
	}
	config, err := settings.OTelExport()
	switch {
	case err != nil || !config.Enabled || !config.Bodies:
		return usage.NoBodies
	case config.BodiesWhole:
		return usage.WholeBodies
	}
	return usage.CapturedBodies
}

// recordBytes is how much of a record the queue holds: its bodies, the
// metadata being small beside them
func recordBytes(r Record) int { return len(r.BodyIn) + len(r.BodyOut) }

func (e *otelExporter) offer(item otelItem) {
	n := int64(recordBytes(item.record))
	if n > 0 {
		for {
			held := e.bytes.Load()
			if n > otelQueueBytes-held {
				e.dropped.Add(1) // telemetry must never wait for a slow collector, nor grow without bound
				return
			}
			if e.bytes.CompareAndSwap(held, held+n) {
				break
			}
		}
	}
	select {
	case e.queue <- item:
	default:
		if n > 0 {
			e.bytes.Add(-n)
		}
		e.dropped.Add(1) // telemetry must never wait for a slow collector
	}
}

func (e *otelExporter) run(every time.Duration) {
	defer close(e.done)
	defer e.cancel()
	defer e.client.CloseIdleConnections()
	ticker := time.NewTicker(every)
	defer ticker.Stop()
	batch := make([]otelItem, 0, otelBatchSize)
	flush := func() {
		e.flush(batch)
		for _, it := range batch {
			if n := int64(recordBytes(it.record)); n > 0 {
				e.bytes.Add(-n)
			}
		}
		batch = batch[:0]
	}
	for {
		select {
		case it := <-e.queue:
			batch = append(batch, it)
			if len(batch) == otelBatchSize {
				flush()
			}
		case <-ticker.C:
			flush()
		case <-e.stop:
			for {
				select {
				case it := <-e.queue:
					batch = append(batch, it)
					if len(batch) == otelBatchSize {
						flush()
					}
				default:
					flush()
					return
				}
			}
		}
	}
}

func (e *otelExporter) flush(batch []otelItem) {
	if len(batch) == 0 {
		return
	}
	config, err := settings.OTelExport()
	if err != nil || !config.Enabled {
		return
	}
	var records []otelItem
	for _, it := range batch {
		// Never send queued records to a new destination or with changed credentials.
		if reflect.DeepEqual(it.config, config) {
			records = append(records, it)
		}
	}
	if len(records) == 0 {
		return
	}
	now := time.Now()
	// spans carrying bodies (up to 256 KiB each way, whole when asked for)
	// go a few at a time, so one request stays well inside a collector's
	// size limit; a record's own bodies decide when a batch is full too
	step := len(records)
	if config.Bodies {
		step = otelBodiesBatch
	}
	for i := 0; i < len(records); {
		j := min(i+step, len(records))
		if config.Bodies {
			bytes := 0
			for j = i; j < len(records) && j < i+step; j++ {
				bytes += recordBytes(records[j].record)
				if j > i && bytes > otelBodiesBatchBytes {
					break
				}
			}
		}
		e.send(config, "traces", e.traces(records[i:j]))
		i = j
	}
	if config.Metrics {
		ledger := make([]Record, 0, len(records))
		for _, it := range records {
			if s := it.span; s == nil || !s.Root && !s.Update && (!s.Session || s.Type == "generation") {
				ledger = append(ledger, it.record)
			}
		}
		if len(ledger) > 0 {
			e.send(config, "metrics", otelMetrics(ledger, e.last, now))
		}
	}
	e.last = now
}

func (e *otelExporter) send(config settings.OTel, signal string, payload any) {
	b, _ := json.Marshal(payload)
	base := strings.TrimRight(config.Endpoint, "/")
	base = strings.TrimSuffix(strings.TrimSuffix(base, "/v1/traces"), "/v1/metrics")
	endpoint := base + "/v1/" + signal
	delay := time.Second
	for attempt := 0; attempt < 3; attempt++ {
		current, err := settings.OTelExport()
		if err != nil || !current.Enabled || !reflect.DeepEqual(current, config) {
			return
		}
		req, err := http.NewRequestWithContext(e.ctx, "POST", endpoint, bytes.NewReader(b))
		if err != nil {
			return
		}
		for k, v := range config.Headers {
			req.Header.Set(k, v)
		}
		req.Header.Set("Content-Type", "application/json")
		res, err := e.client.Do(req)
		retry := err != nil
		if res != nil {
			response, _ := io.ReadAll(io.LimitReader(res.Body, 64<<10))
			res.Body.Close()
			if res.StatusCode == http.StatusOK {
				var result struct {
					PartialSuccess struct {
						RejectedSpans      json.Number `json:"rejectedSpans"`
						RejectedDataPoints json.Number `json:"rejectedDataPoints"`
					} `json:"partialSuccess"`
				}
				json.Unmarshal(response, &result)
				if result.PartialSuccess.RejectedSpans != "" && result.PartialSuccess.RejectedSpans != "0" || result.PartialSuccess.RejectedDataPoints != "" && result.PartialSuccess.RejectedDataPoints != "0" {
					log.Printf("otel: %s export partially rejected", signal)
				}
				return
			}
			retry = res.StatusCode == 429 || res.StatusCode == 502 || res.StatusCode == 503 || res.StatusCode == 504
			if n, err := strconv.Atoi(res.Header.Get("Retry-After")); err == nil && n > 0 {
				delay = time.Duration(min(n, 60)) * time.Second
			} else if at, err := http.ParseTime(res.Header.Get("Retry-After")); err == nil && time.Until(at) > 0 {
				delay = time.Until(at)
			}
		}
		if !retry || attempt == 2 {
			if e.ctx.Err() == nil {
				log.Printf("otel: %s export failed; batch dropped", signal)
			}
			return
		}
		timer := time.NewTimer(min(delay, 60*time.Second))
		select {
		case <-e.ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
		delay *= 2
	}
}
