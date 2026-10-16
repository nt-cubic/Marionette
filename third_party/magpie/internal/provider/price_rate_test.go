package provider

import (
	"testing"

	"github.com/yetone/magpie/internal/settings"
)

// A provider's price rate (#819) scales the list price a call is counted
// at, as a relay charging 0.8 of the official price bills; a price the user
// set for a model is what they said it costs, and stays as set.
func TestPriceRateScalesListPrice(t *testing.T) {
	priceHome(t)
	priceCatalog(t)
	if err := Save(Provider{ID: "openai", Name: "OpenAI", Chat: "https://api.openai.com/v1", Key: "k", Models: []string{"sol"}, PriceRate: 0.5}); err != nil {
		t.Fatal(err)
	}
	pr, ok := EffectivePrice("openai", "sol")
	if !ok || pr.Input != 1 || pr.Output != 5 || pr.CacheRead != 0.125 || pr.CacheWrite != 1.25 {
		t.Fatalf("at half the list price: %+v %v", pr, ok)
	}
	if err := settings.Save(settings.Settings{ModelPrices: map[string]settings.ModelPrice{"openai/sol": statePrice(3)}}); err != nil {
		t.Fatal(err)
	}
	if pr, ok := EffectivePrice("openai", "sol"); !ok || pr.Input != 3 || pr.Output != 3 {
		t.Fatalf("a price set for the model is not scaled: %+v %v", pr, ok)
	}
}

func TestPriceRateOK(t *testing.T) {
	for _, r := range []float64{0, 0.8, 1.5, 0.125, 1000} {
		if bad := PriceRateOK(r); bad != "" {
			t.Errorf("%v: %s", r, bad)
		}
	}
	for _, r := range []float64{-1, 1000.5, 0.1234} {
		if PriceRateOK(r) == "" {
			t.Errorf("%v taken", r)
		}
	}
}
