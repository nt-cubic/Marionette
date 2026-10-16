package usage

import (
	"slices"
	"sync"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// swapKnown marks a row whose Swapped flag was settled when it was priced,
// so reading it back needs no second look at the two names.
const swapKnown = 64

// pricedAs is what a gateway call is priced as, the same for every call of
// one provider, model, served model and effort.
type pricedAs struct {
	provider string
	flags    uint16
	price    *catalog.Price
}

// blockPricer prices the gateway's raw blocks as a query reads them. What a
// row's price, provider id, swap and routing are depends on a few of its
// names only, so they are worked out once for each set of names, not once
// per call: 750k calls name a few hundred.
type blockPricer struct {
	aliases map[string]string
	wires   map[string]string
	price   func(Record) *catalog.Price
	by      map[[4]string]pricedAs
	agents  map[string][2]string
	pooled  []*[]packedRow
}

// pricedRows are the rows of the blocks a query priced, handed back when
// it is done with them: a query over the whole history decodes every block,
// and allocating and clearing their rows again each time was a third of it.
var pricedRows = sync.Pool{New: func() any { s := make([]packedRow, 0, logBlockRows); return &s }}

// release hands the rows of the blocks it priced back, once the query no
// longer reads them (a page's rows are copies).
func (b *blockPricer) release() {
	for _, rows := range b.pooled {
		pricedRows.Put(rows)
	}
	b.pooled = nil
}

func newBlockPricer(aliases, wires map[string]string, price func(Record) *catalog.Price) *blockPricer {
	return &blockPricer{aliases: aliases, wires: wires, price: price, by: map[[4]string]pricedAs{}, agents: map[string][2]string{}}
}

// as is priceRow's answer for a gateway call of these names.
func (b *blockPricer) as(prov, model, served, effort string) pricedAs {
	k := [4]string{prov, model, served, effort}
	if a, ok := b.by[k]; ok {
		return a
	}
	if id, ok := b.aliases[prov]; ok {
		prov = id
	}
	a := pricedAs{provider: prov, flags: swapKnown}
	sent := provider.SentNameIn(b.wires, prov, model, effort)
	if served != "" && Swapped(sent, served) && !(SameSpelled(model, served) || ServingName(model, served) || RelayRenamed(model, served)) {
		a.flags |= 2
	}
	if GroupRouted(sent, served) {
		a.flags |= 32
	}
	a.price = b.price(Record{Provider: prov, Model: model, Served: served, Effort: effort})
	b.by[k] = a
	return a
}

// agent is reportedAs's answer for a client, the agent counted and the
// client's own name when the user's counting changed it.
func (b *blockPricer) agent(ua string) (string, string) {
	a, ok := b.agents[ua]
	if !ok {
		a[0], a[1] = reportedAs(ua)
		b.agents[ua] = a
	}
	return a[0], a[1]
}

// block is the priced copy of a raw block, made for one query and not kept.
func (b *blockPricer) block(packed *rowChunk) *rowChunk {
	buf := pricedRows.Get().(*[]packedRow)
	raw := packed.unpackInto(*buf)
	// unpackInto decodes into the buffer whenever it is large enough
	if raw != nil && raw != packed && cap(*buf) >= len(raw.Rows) {
		b.pooled = append(b.pooled, buf)
	} else {
		pricedRows.Put(buf)
	}
	if raw == nil {
		return nil
	}
	c := *raw
	if raw == packed {
		c.Rows = slices.Clone(raw.Rows) // a block too small to compress is shared
	}
	c.Strings = slices.Clip(c.Strings)
	added := map[string]uint32{}
	intern := func(s string, was uint32) uint32 {
		if c.Strings[was] == s {
			return was
		}
		if id, ok := added[s]; ok {
			return id
		}
		id := uint32(len(c.Strings))
		c.Strings = append(c.Strings, s)
		added[s] = id
		return id
	}
	type ids struct {
		provider uint32
		flags    uint16
		price    *catalog.Price
	}
	names := map[[4]uint32]ids{}
	agents := map[uint32][2]uint32{}
	for i := range c.Rows {
		p := &c.Rows[i]
		k := [4]uint32{p.Text[1], p.Text[5], p.Text[7], p.Text[8]}
		got, ok := names[k]
		if !ok {
			a := b.as(c.Strings[k[0]], c.Strings[k[1]], c.Strings[k[2]], c.Strings[k[3]])
			got = ids{intern(a.provider, k[0]), a.flags, a.price}
			names[k] = got
		}
		agent, ok := agents[p.Text[0]]
		if !ok {
			a, client := b.agent(c.Strings[p.Text[0]])
			agent = [2]uint32{intern(a, p.Text[0]), intern(client, p.Text[rowClient])}
			agents[p.Text[0]] = agent
		}
		p.Text[0], p.Text[1], p.Text[rowClient] = agent[0], got.provider, agent[1]
		p.Flags = p.Flags&^(1|2|32) | got.flags
		if p.Flags&rowOwnCost != 0 {
			// a reported call that said what it cost costs that (#1874)
			if p.Tokens[0]+p.Tokens[1] > 0 {
				p.Flags |= 1
			}
			continue
		}
		p.Cost = 0
		if got.price != nil && p.Tokens[0]+p.Tokens[1] > 0 {
			p.Flags |= 1
			p.Cost = got.price.CostSplit(int(p.Tokens[0]), int(p.Tokens[1]), int(p.Tokens[2]), int(p.Tokens[3]), int(p.Tokens[5]))
		}
	}
	return &c
}
