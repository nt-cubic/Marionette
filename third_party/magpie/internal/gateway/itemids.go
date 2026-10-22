package gateway

import (
	"github.com/tidwall/gjson"
)

// idTidy keeps each output item of a relayed Responses stream under the
// one id its output_item.added gave it. GitHub Copilot's /responses gives
// an item a new id in every event: the added, each delta, the done and
// response.completed's output each carry another (2026-10-19, a real
// account, gpt-5.5 with web_search). Codex starts an item by the added's
// id and finishes one by the done's, so with two ids a message was shown
// twice, once as it streamed and once finished, and a web search stayed
// started (echo_ts on Discord, once ec7f47432 relayed Codex on Copilot
// instead of translating it). An upstream whose ids already agree goes
// through as it came.
type idTidy struct {
	buf []byte
	ids map[int64]idOf // output_index → the item's id and type, as added
}

type idOf struct{ id, typ string }

func (t *idTidy) write(b []byte) []byte {
	t.buf = append(t.buf, b...)
	var out []byte
	for {
		n := eventEnd(t.buf)
		if n < 0 {
			return out
		}
		out = append(out, t.event(t.buf[:n])...)
		t.buf = t.buf[n:]
	}
}

func (t *idTidy) flush() []byte {
	b := t.buf
	t.buf = nil
	return b
}

func (t *idTidy) event(ev []byte) []byte {
	data := eventData(ev)
	if len(data) == 0 || !gjson.ValidBytes(data) {
		return append([]byte(nil), ev...)
	}
	e := gjson.ParseBytes(data)
	at := e.Get("output_index")
	switch typ := e.Get("type").String(); {
	case typ == "response.output_item.added":
		if id := e.Get("item.id").String(); id != "" && at.Exists() {
			if t.ids == nil {
				t.ids = map[int64]idOf{}
			}
			t.ids[at.Int()] = idOf{id, e.Get("item.type").String()}
		}
		return append([]byte(nil), ev...)
	case typ == "response.completed" || typ == "response.incomplete" || typ == "response.failed":
		return t.ended(ev, data, e)
	case !at.Exists():
		return append([]byte(nil), ev...)
	}
	was, ok := t.ids[at.Int()]
	if !ok {
		return append([]byte(nil), ev...)
	}
	itemID, itemIn := e.Get("item_id"), e.Get("item.id")
	if (!itemID.Exists() || itemID.String() == was.id) && (!itemIn.Exists() || itemIn.String() == was.id) {
		return append([]byte(nil), ev...)
	}
	var m map[string]any
	if decodeNumbers(data, &m) != nil {
		return append([]byte(nil), ev...)
	}
	if itemID.Exists() {
		m["item_id"] = was.id
	}
	if it, _ := m["item"].(map[string]any); it != nil && itemIn.Exists() && (was.typ == "" || it["type"] == was.typ) {
		it["id"] = was.id
	}
	nb, err := marshalPlain(m)
	if err != nil {
		return append([]byte(nil), ev...)
	}
	return withData(ev, nb)
}

// ended is the reply's last event, each item of its output under the id
// it was added with.
func (t *idTidy) ended(ev, data []byte, e gjson.Result) []byte {
	differs := false
	for i, it := range e.Get("response.output").Array() {
		if was, ok := t.ids[int64(i)]; ok && it.Get("id").Exists() && it.Get("id").String() != was.id && (was.typ == "" || it.Get("type").String() == was.typ) {
			differs = true
			break
		}
	}
	if !differs {
		return append([]byte(nil), ev...)
	}
	var m map[string]any
	if decodeNumbers(data, &m) != nil {
		return append([]byte(nil), ev...)
	}
	res, _ := m["response"].(map[string]any)
	out, _ := res["output"].([]any)
	for i, v := range out {
		it, _ := v.(map[string]any)
		was, ok := t.ids[int64(i)]
		if it == nil || !ok {
			continue
		}
		if _, has := it["id"]; has && (was.typ == "" || it["type"] == was.typ) {
			it["id"] = was.id
		}
	}
	nb, err := marshalPlain(m)
	if err != nil {
		return append([]byte(nil), ev...)
	}
	return withData(ev, nb)
}
