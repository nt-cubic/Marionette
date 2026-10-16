package qoder

import "testing"

// A model at a price_factor of 0 is free, in snake or camel case; is_free
// counts only on a listing with no price.
func TestParseModelsFree(t *testing.T) {
	body := []byte(`{"chat":[
		{"key":"flag","enable":true,"is_free":true,"price_factor":1},
		{"key":"camel-flag","enable":true,"isFree":true},
		{"key":"zero","enable":true,"price_factor":0},
		{"key":"camel-zero","enable":true,"priceFactor":0},
		{"key":"paid","enable":true,"is_free":false,"price_factor":0.5},
		{"key":"unsaid","enable":true}
	]}`)
	ms, err := ParseModels(body, ProviderKey)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]bool{"flag": false, "camel-flag": true, "zero": true, "camel-zero": true, "paid": false, "unsaid": false}
	if len(ms) != len(want) {
		t.Fatalf("models %+v", ms)
	}
	for _, m := range ms {
		if m.Free != want[m.ID] {
			t.Errorf("%s: Free = %v, want %v", m.ID, m.Free, want[m.ID])
		}
	}
}

// Qoder's listing as it came on 2026-10-02: its app shows Qwen3.8-Max at
// 0.5× with an off-peak badge (错峰 4 折), Qwen3.8-Flash at 0× with 0.1×
// struck through — both carry is_free true. Only Flash is free.
func TestParseModelsFreeIsThePrice(t *testing.T) {
	offPeak := `{"active":false,"badge":{"en":"Off-Peak 60% off","zh":"错峰 4 折"},"timezone":"Asia/Singapore","rule_id":"idle_time_model_credit_discount","discount_factor":0.4,"before_promotion_price_factor":0.5,"window_start":"22:00","window_end":"08:00"}`
	body := []byte(`{"chat":[
		{"key":"qmodel_38max","display_name":"Qwen3.8-Max","enable":true,"is_reasoning":true,"price_factor":0.5,"is_free":true,"promotion":` + offPeak + `},
		{"key":"qfmodel","display_name":"Qwen3.8-Flash","enable":true,"is_reasoning":true,"price_factor":0.0,"original_price_factor":0.1,"is_free":true},
		{"key":"qmodel_latest","display_name":"Qwen3.7-Max","enable":true,"price_factor":0.5,"original_price_factor":0.5},
		{"key":"kmodel_latest","display_name":"Kimi-K3","enable":true,"price_factor":1.4,"is_free":false},
		{"key":"window","enable":true,"price_factor":0,"is_free":true,"promotion":{"active":true,"discount_factor":0.0001,"before_promotion_price_factor":0.5}},
		{"key":"window-camel","enable":true,"priceFactor":0,"promotion":{"active":true,"beforePromotionPriceFactor":1}},
		{"key":"over","enable":true,"price_factor":0,"promotion":{"active":false,"before_promotion_price_factor":0.5}},
		{"key":"nothing","enable":true,"price_factor":0,"promotion":{"active":true,"before_promotion_price_factor":0}},
		{"key":"unpriced","enable":true,"is_free":true},
		{"key":"unpriced2","enable":true,"is_free":false}
	]}`)
	ms, err := ParseModels(body, ProviderKey)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]bool{"qmodel_38max": false, "qfmodel": true, "qmodel_latest": false, "kmodel_latest": false,
		"window": false, "window-camel": false, "over": true, "nothing": true, "unpriced": true, "unpriced2": false}
	if len(ms) != len(want) {
		t.Fatalf("models %+v", ms)
	}
	for _, m := range ms {
		if m.Free != want[m.ID] {
			t.Errorf("%s: Free = %v, want %v", m.ID, m.Free, want[m.ID])
		}
	}
}

// Each model carries its price as Qoder's client shows it beside the
// model (01huadalang on Discord: pick the cheap ones without opening
// Qoder): Qwen3.8-Max 0.5×, Kimi-K3 1.4×, Qwen3.8-Flash 0× (free) with
// 0.1× struck through; a running promotion's price before it struck
// through, and its 0 that isn't free read as that price times its
// discount. A listing with no price gives none.
func TestParseModelsRate(t *testing.T) {
	body := []byte(`{"chat":[
		{"key":"qmodel_38max","enable":true,"price_factor":0.5,"is_free":true,"promotion":{"active":false,"discount_factor":0.4,"before_promotion_price_factor":0.5}},
		{"key":"qmodel_38max-offpeak","enable":true,"price_factor":0.2,"promotion":{"active":true,"discount_factor":0.4,"before_promotion_price_factor":0.5}},
		{"key":"qfmodel","enable":true,"price_factor":0.0,"original_price_factor":0.1,"is_free":true},
		{"key":"qmodel_latest","enable":true,"price_factor":0.5,"original_price_factor":0.5},
		{"key":"kmodel_latest","enable":true,"priceFactor":1.4,"originalPriceFactor":2},
		{"key":"window","enable":true,"price_factor":0,"promotion":{"active":true,"discount_factor":0.5,"before_promotion_price_factor":1}},
		{"key":"window-camel","enable":true,"priceFactor":0,"promotion":{"active":true,"discountFactor":0.25,"beforePromotionPriceFactor":2}},
		{"key":"unpriced","enable":true,"is_free":true}
	]}`)
	ms, err := ParseModels(body, ProviderKey)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string][2]float64{"qmodel_38max": {0.5, 0}, "qmodel_38max-offpeak": {0.2, 0.5}, "qfmodel": {0, 0.1},
		"qmodel_latest": {0.5, 0}, "kmodel_latest": {1.4, 2}, "window": {0.5, 1}, "window-camel": {0.5, 2}, "unpriced": {0, 0}}
	if len(ms) != len(want) {
		t.Fatalf("models %+v", ms)
	}
	for _, m := range ms {
		if w := want[m.ID]; m.Rate != w[0] || m.RateWas != w[1] {
			t.Errorf("%s: Rate, RateWas = %v, %v, want %v, %v", m.ID, m.Rate, m.RateWas, w[0], w[1])
		}
	}
}
