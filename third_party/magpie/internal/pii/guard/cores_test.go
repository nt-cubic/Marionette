package guard

import (
	"fmt"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"
)

// A window takes a core, waiting while every one is held, and of more
// only those free: Finds at once never read on more than Cores()
// between them (yyhhyyyyyy's requests at once, xinvxueyuan's 13 cores).
func TestTakeHoldsOnlyWhatIsFree(t *testing.T) {
	c := cap(cores)
	if c != Cores() {
		t.Fatalf("%d cores to share, Cores() %d", c, Cores())
	}
	held := 0
	for range c - 1 {
		held += take(1)
	}
	got := make(chan int, 1)
	go func() { got <- take(c) }()
	select {
	case n := <-got:
		if n != 1 {
			t.Fatalf("took %d with one free", n)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("waited for more than the one free")
	}
	held++
	go func() { got <- take(c) }()
	select {
	case n := <-got:
		t.Fatalf("took %d with none free", n)
	case <-time.After(50 * time.Millisecond):
	}
	give(1)
	held--
	if n := <-got; n != 1 {
		t.Fatalf("took %d of the one given back", n)
	}
	held++
	give(held)
	if len(cores) != 0 {
		t.Fatalf("%d left held", len(cores))
	}
	if n := take(c); n != c {
		t.Fatalf("took %d of %d free", n, c)
	}
	give(c)
}

// Texts read at once, sharing the cores, find what each finds alone.
func TestFindsAtOnceReadAsOneAtATime(t *testing.T) {
	m, err := Load(dir(t))
	if err != nil {
		t.Fatal(err)
	}
	m.ml = nil
	texts := make([]string, 8)
	for i := range texts {
		var b strings.Builder
		for k := range 3 + 5*i {
			fmt.Fprintf(&b, "第%d位联系人：张三，电话 138%08d，邮箱 user%d@example.com。\n", k, (i+1)*k*7919, k)
		}
		texts[i] = b.String()
	}
	alone := make([][]Span, len(texts))
	for i, s := range texts {
		alone[i] = m.Find(s)
	}
	at := make([][]Span, len(texts))
	var wg sync.WaitGroup
	for i, s := range texts {
		wg.Go(func() { at[i] = m.Find(s) })
	}
	wg.Wait()
	for i := range texts {
		if len(alone[i]) == 0 || !reflect.DeepEqual(at[i], alone[i]) {
			t.Fatalf("text %d: at once %d spans, alone %d", i, len(at[i]), len(alone[i]))
		}
	}
	if len(cores) != 0 {
		t.Fatalf("%d cores left held", len(cores))
	}
}
