// Package guard runs AstrLink Guard (QuantumNous/astrlink-guard,
// Apache-2.0), a token classifier for secrets and personal data in Chinese
// and English text, on the CPU in Go: its int8 ONNX model (37 MB) read by
// onnx.go and run by bert.go, its tokenizer by tokenizer.go. Nothing
// native is linked or started.
package guard

import (
	"fmt"
	"math"
	"path/filepath"
)

// labels are the model's BIO tags, by output index (its labels.json).
var labels = []string{"O",
	"B-email", "I-email", "B-phone", "I-phone", "B-account", "I-account",
	"B-payment_card", "I-payment_card", "B-ip_address", "I-ip_address",
	"B-url", "I-url", "B-common_secret", "I-common_secret",
	"B-private_address", "I-private_address", "B-private_date", "I-private_date",
	"B-private_person", "I-private_person"}

// Files are the model's files Load reads, in its folder.
var Files = []string{"model_int8.onnx", "model_int8.onnx_data", "tokenizer.json"}

// Model is the model, loaded.
type Model struct {
	tok *tokenizer
	net *net
}

// Span is what the model found: text[Start:End], a Label of its (email,
// phone, account, payment_card, ip_address, url, common_secret,
// private_address, private_date, private_person), and the least of its
// tokens' probabilities.
type Span struct {
	Start, End int
	Label      string
	Score      float64
}

// MinScore is the probability under which a span is dropped: the
// publisher's default.
const MinScore = 0.8

// window is how many tokens the model reads at once, besides <s> and
// </s>; a longer text is read in windows overlapping by overlap, each
// token labelled by the window it is furthest inside.
const (
	window  = maxPos - 2
	overlap = 128
)

// Load reads the model in dir.
func Load(dir string) (*Model, error) {
	tok, err := loadTokenizer(filepath.Join(dir, "tokenizer.json"))
	if err != nil {
		return nil, fmt.Errorf("tokenizer: %w", err)
	}
	nt, err := loadNet(filepath.Join(dir, "model_int8.onnx"))
	if err != nil {
		return nil, fmt.Errorf("model: %w", err)
	}
	if nt.labels != len(labels) {
		return nil, fmt.Errorf("model: %d labels, not %d", nt.labels, len(labels))
	}
	return &Model{tok, nt}, nil
}

// Find is what the model finds in text, at MinScore.
func (m *Model) Find(text string) []Span {
	toks := m.tok.encode(text)
	n := len(toks)
	if n == 0 {
		return nil
	}
	lab := make([]int, n)
	prob := make([]float64, n)
	margin := make([]int, n)
	for i := range margin {
		margin[i] = -1
	}
	for start := 0; ; start = start + window - overlap {
		end := min(start+window, n)
		ids := make([]int, 0, end-start+2)
		ids = append(ids, 0)
		for _, t := range toks[start:end] {
			ids = append(ids, t.id)
		}
		ids = append(ids, 2)
		lg := m.net.logits(ids)
		k := len(labels)
		for i := 0; i < end-start; i++ {
			j := start + i
			mg := min(i, end-start-1-i)
			if mg <= margin[j] {
				continue
			}
			row := lg[(i+1)*k : (i+2)*k]
			best, mx := 0, row[0]
			for c, v := range row {
				if v > mx {
					best, mx = c, v
				}
			}
			var sum float64
			for _, v := range row {
				sum += math.Exp(float64(v - mx))
			}
			margin[j], lab[j], prob[j] = mg, best, 1/sum
		}
		if end == n {
			break
		}
	}
	return decode(toks, lab, prob)
}

// decode reads the tokens' BIO tags (lab, by index into labels) into
// spans: a B tag, or an I tag of another kind than the span before it,
// starts one; an I tag of the same kind extends it; O ends it. A span's
// score is its least token's, and those under MinScore are dropped.
// Tokens of no text (a lone "▁") neither start nor end a span.
func decode(toks []token, lab []int, prob []float64) []Span {
	var spans []Span
	cur := -1
	for j, t := range toks {
		if t.start == t.end {
			continue
		}
		tag := labels[lab[j]]
		if tag == "O" {
			cur = -1
			continue
		}
		kind := tag[2:]
		if tag[0] == 'B' || cur < 0 || spans[cur].Label != kind {
			spans = append(spans, Span{t.start, t.end, kind, prob[j]})
			cur = len(spans) - 1
			continue
		}
		spans[cur].End = t.end
		spans[cur].Score = min(spans[cur].Score, prob[j])
	}
	out := spans[:0]
	for _, s := range spans {
		if s.Score >= MinScore {
			out = append(out, s)
		}
	}
	return out
}
