package guard

import (
	"reflect"
	"testing"
	"unicode/utf8"
)

// A malformed byte decodes to RuneError but consumes one byte, not
// utf8.RuneLen(RuneError)'s three: normalize must step by the decoded
// width, or it both misreports what the text came from and slices past
// the end of a short grapheme (a panic, through Find, in the model's
// worker goroutine).
func TestNormalizeMalformedBytes(t *testing.T) {
	tk := &tokenizer{}
	for _, c := range []struct {
		text string
		want []nchar
	}{
		{"\xff", []nchar{{utf8.RuneError, 0, 1}}},
		{"\xc3", []nchar{{utf8.RuneError, 0, 1}}},
		{"\xed\xa0\x80", []nchar{
			{utf8.RuneError, 0, 1}, {utf8.RuneError, 1, 2}, {utf8.RuneError, 2, 3},
		}},
		{"hi\xffthere", []nchar{
			{'h', 0, 1}, {'i', 1, 2}, {utf8.RuneError, 2, 3},
			{'t', 3, 4}, {'h', 4, 5}, {'e', 5, 6}, {'r', 6, 7}, {'e', 7, 8},
		}},
		{"\xf0\x9f", []nchar{{utf8.RuneError, 0, 1}, {utf8.RuneError, 1, 2}}},
		// #4553's embeddings input, a lone 0xff after a word and a space
		{"hello \xff", []nchar{
			{'h', 0, 1}, {'e', 1, 2}, {'l', 2, 3}, {'l', 3, 4}, {'o', 4, 5},
			{' ', 5, 6}, {utf8.RuneError, 6, 7},
		}},
		// a properly encoded U+FFFD keeps its three bytes
		{"\uFFFD", []nchar{{utf8.RuneError, 0, 3}}},
		// valid text is unchanged
		{"aéb", []nchar{{'a', 0, 1}, {'é', 1, 3}, {'b', 3, 4}}},
	} {
		got := tk.normalize(c.text, 0)
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("normalize(%q) = %v, want %v", c.text, got, c.want)
		}
		for _, nc := range got {
			if nc.s < 0 || nc.e > len(c.text) || nc.s >= nc.e {
				t.Errorf("normalize(%q): offset [%d, %d) is out of the text", c.text, nc.s, nc.e)
			}
		}
	}
	// encode, the path Find runs, for one malformed byte gives one token
	// over that byte
	if got := tk.encode("\xff"); !reflect.DeepEqual(got, []token{{0, 0, 1}}) {
		t.Errorf("encode(\"\\xff\") = %v, want [{0 0 1}]", got)
	}
	for _, tok := range tk.encode("hello \xff") {
		if tok.start < 0 || tok.end > len("hello \xff") {
			t.Errorf("encode(\"hello \\xff\"): token [%d, %d) is out of the text", tok.start, tok.end)
		}
	}
}

// A charsmap can map the malformed byte itself: add must then give its
// output the byte's one-byte span as where it came from.
func TestNormalizeMappedMalformedByte(t *testing.T) {
	tk := &tokenizer{trie: make([]uint32, 0x100), norm: []byte{'X', 0}}
	// the double-array cell for the byte 0xff: its check byte, a value,
	// and the offset landing the walk on the value at norm[0]
	tk.trie[0xff] = 0x3F800 | 0x100 | 0xFF
	for _, c := range []struct {
		text string
		want []nchar
	}{
		{"\xff", []nchar{{'X', 0, 1}}},
		{"\xff\xff a", []nchar{{'X', 0, 1}, {'X', 1, 2}, {' ', 2, 3}, {'a', 3, 4}}},
	} {
		if got := tk.normalize(c.text, 0); !reflect.DeepEqual(got, c.want) {
			t.Errorf("normalize(%q) = %v, want %v", c.text, got, c.want)
		}
	}
}
