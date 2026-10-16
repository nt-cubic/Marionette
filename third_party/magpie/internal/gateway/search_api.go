package gateway

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"html"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/yetone/magpie/internal/provider"
)

// When no provider can search, magpie's web_search tool is answered by a
// search API the user gave a key to (#419): what it found goes to the
// model as it came, each page's title, address and what it says, with no
// model asked to read it first.

// searchHits is how many pages a search API is asked for.
const searchHits = 6

// pageText is the most of a page's text a hit carries.
const pageText = 1500

// foundPage is one page a search API found.
type foundPage struct {
	Title, URL, Text string
	Age              string // how old the search said it is, "" when it didn't
}

// canSearch is whether magpie can answer its web_search tool: plugins/search
// is on (searchServed), and a provider searches for it, or a search API
// does.
func canSearch() bool {
	if !searchServed() {
		return false
	}
	if _, _, ok := searcher(); ok {
		return true
	}
	return len(provider.SearchAPIs()) > 0
}

// CanSearch is canSearch for the agents' rows: whether a web_search
// server tool asked of any model here is answered.
func CanSearch() bool { return canSearch() }

// Searcher names the provider and model magpie searches with for a model
// that can't, "" when none can.
func Searcher() string {
	if p, m, ok := searcher(); ok {
		return searcherName(p, m)
	}
	return ""
}

// searcherName is a searcher by its provider and model; a Kimi Code plan,
// which searches with no model, by its name.
func searcherName(p provider.Provider, model string) string {
	if model == "" {
		return p.Name
	}
	if e, ok := provider.ServedEntryOf(p.ID + "/" + model); ok && e.Name != "" {
		model = e.Name
	}
	return p.Name + " · " + model
}

// AutoSearcher names the provider and model magpie picks to search with
// when Settings names none, "" when none can.
func AutoSearcher() string {
	if p, m, ok := autoSearcher(); ok {
		return searcherName(p, m)
	}
	return ""
}

// NamedSearchModel is the model the searcher Settings names searches with,
// as "<provider>/<model>": what an agent's own web search row asks, so a
// search goes to the searcher the user picked rather than first to the
// agent's model (dsh, 我只会玩单机 on Discord). "" when Settings names
// none, provider search is off, the one named can't be used, or it
// searches with no model (a Kimi Code plan's search service).
func NamedSearchModel() string {
	p, m, why := chosenSearcher()
	if p == nil || why != "" || m == "" {
		return ""
	}
	return p.ID + "/" + m
}

// SearcherUnused is why the searcher Settings names isn't used (one of
// the Searcher* reasons), "" when it is or none is named.
func SearcherUnused() string {
	_, _, why := chosenSearcher()
	return why
}

// apiSearch asks the search APIs, in their order, until one answers.
func (s *Server) apiSearch(ctx context.Context, query string) (string, []Hit, error) {
	query, err := maskedQuery(query, nil)
	if err != nil {
		return "", nil, err
	}
	var errs []error
	for _, a := range provider.SearchAPIs() {
		pages, err := s.askSearchAPIKeys(ctx, a, query)
		if err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", a.Name(), err))
			continue
		}
		if len(pages) == 0 {
			errs = append(errs, fmt.Errorf("%s found nothing", a.Name()))
			continue
		}
		said, hits := pagesSaid(pages)
		return said, hits, nil
	}
	return "", nil, errors.Join(errs...)
}

// askSearchAPIKeys asks a search API with its keys, in searchKeyOrder:
// on to the next while one is refused (out of credit, rate limited,
// turned away; #1477), which then rests (searchRestFor). Anything else, such as
// no answer in time, isn't about the key, and the next isn't tried.
func (s *Server) askSearchAPIKeys(ctx context.Context, a provider.SearchAPI, query string) ([]foundPage, error) {
	keys := a.Keys()
	if len(keys) == 0 {
		keys = []string{""}
	}
	var errs []error
	for _, i := range searchKeyOrder(a, keys, time.Now()) {
		one := a
		one.Key = keys[i]
		ctx, cancel := context.WithTimeout(ctx, searchTimeout/4)
		pages, err := s.askSearchAPI(ctx, one, query)
		cancel()
		if err == nil {
			wakeSearchKey(a.Vendor, keys[i])
			return pages, nil
		}
		var r searchRefused
		refused := errors.As(err, &r)
		if refused && keys[i] != "" {
			d, why := searchRestFor(a.Vendor, r.status, r.retryAfter, time.Now())
			restSearchKey(a.Vendor, keys[i], d, why, time.Now())
		}
		if len(keys) > 1 {
			err = fmt.Errorf("%s: %w", keyLabel(i, keys[i]), err)
		}
		errs = append(errs, err)
		if !refused {
			break
		}
	}
	return nil, errors.Join(errs...)
}

// searchRefused is a search API's refusal of the key it was asked with,
// by its status: 401/403 turned away, 402/432/433 out of credit (Tavily's
// plan and pay-as-you-go limits), 429 rate limited.
type searchRefused struct {
	status     int
	msg        string
	retryAfter string // its Retry-After, "" when it said none
}

func (e searchRefused) Error() string { return e.msg }

// keyRefusal is whether an answer's status is about the key it was asked
// with (searchRefused).
func keyRefusal(status int) bool {
	switch status {
	case 401, 402, 403, 429, 432, 433:
		return true
	}
	return false
}

// pagesSaid is what the pages a search found say, as its model is given
// it, and the pages as the client hears of them.
func pagesSaid(pages []foundPage) (string, []Hit) {
	var b strings.Builder
	var hits []Hit
	for i, p := range pages {
		fmt.Fprintf(&b, "%d. %s — %s\n", i+1, p.Title, p.URL)
		if p.Text != "" {
			b.WriteString(p.Text + "\n")
		}
		b.WriteString("\n")
		hits = append(hits, Hit{Title: p.Title, URL: p.URL, PageAge: p.Age})
	}
	return strings.TrimSpace(b.String()), hits
}

// askSearchAPI asks one search API for the query.
func (s *Server) askSearchAPI(ctx context.Context, a provider.SearchAPI, query string) ([]foundPage, error) {
	var r *http.Request
	var err error
	post := func(path string, body any) {
		b, _ := json.Marshal(body)
		if r, err = http.NewRequestWithContext(ctx, http.MethodPost, a.Base()+path, bytes.NewReader(b)); err == nil {
			r.Header.Set("Content-Type", "application/json")
		}
	}
	getAt := func(at string, q url.Values) {
		r, err = http.NewRequestWithContext(ctx, http.MethodGet, at+"?"+q.Encode(), nil)
	}
	get := func(path string, q url.Values) { getAt(a.Base()+path, q) }
	switch a.Vendor {
	case "tavily":
		post("/search", map[string]any{"query": query, "max_results": searchHits, "search_depth": "basic"})
	case "brave":
		get("/res/v1/web/search", url.Values{"q": {query}, "count": {fmt.Sprint(searchHits)}})
	case "exa":
		post("/search", map[string]any{"query": query, "numResults": searchHits,
			"contents": map[string]any{"text": map[string]any{"maxCharacters": pageText}}})
	case "firecrawl":
		post("/v2/search", map[string]any{"query": query, "limit": searchHits})
	case "searxng":
		get("/search", url.Values{"q": {query}, "format": {"json"}})
	case "you":
		// with a key, its Search API; without one, its free search (#1626),
		// at an address of the user's own when one is set
		switch {
		case a.Key != "":
			post("/v1/search", map[string]any{"query": query, "count": searchHits})
		case a.URL != "":
			get("/v1/agents/search", url.Values{"query": {query}, "count": {fmt.Sprint(searchHits)}})
		default:
			getAt(provider.YouFreeSearch, url.Values{"query": {query}, "count": {fmt.Sprint(searchHits)}})
		}
	default:
		return nil, fmt.Errorf("magpie can't ask %q", a.Vendor)
	}
	if err != nil {
		return nil, err
	}
	r.Header.Set("Accept", "application/json")
	r.Header.Set("User-Agent", SearchAgent)
	if a.Vendor == "you" {
		// the tag You.com asked for, to tell magpie's searches apart (#1626)
		r.Header.Set("User-Agent", SearchAgent+" youdotcom-integration/yetone-magpie")
	}
	if a.Key != "" {
		switch a.Vendor {
		case "brave":
			r.Header.Set("X-Subscription-Token", a.Key)
		case "exa", "you":
			r.Header.Set("X-API-Key", a.Key)
		default:
			r.Header.Set("Authorization", "Bearer "+a.Key)
		}
	}
	// the key goes to the address the API is set at (#1580) and nowhere
	// else: Go drops only Authorization on a redirect to another host,
	// never Brave's or Exa's own key headers, so such a redirect isn't
	// followed, and its answer is said as the error it is
	c := *s.client
	c.CheckRedirect = func(next *http.Request, via []*http.Request) error {
		if len(via) >= 10 {
			return errors.New("stopped after 10 redirects")
		}
		if next.URL.Scheme != via[0].URL.Scheme || next.URL.Host != via[0].URL.Host {
			return http.ErrUseLastResponse
		}
		return nil
	}
	res, err := c.Do(r)
	if err != nil {
		if ctx.Err() != nil {
			return nil, errors.New("no answer in time")
		}
		return nil, err
	}
	defer res.Body.Close()
	b, _ := io.ReadAll(io.LimitReader(res.Body, 4<<20))
	if res.StatusCode >= 300 && res.StatusCode < 400 && res.Header.Get("Location") != "" {
		return nil, fmt.Errorf("%s: it sends the search on to %s, which isn't given its key; set that address instead", res.Status, res.Header.Get("Location"))
	}
	if res.StatusCode >= 300 {
		msg := provider.APIError(b, res.Status)
		if a.Vendor == "you" {
			// You.com's error is a code ("payment_required"), its message
			// beside it the words
			var e struct{ Message string }
			if json.Unmarshal(b, &e) == nil && e.Message != "" {
				msg = e.Message
			}
		}
		// an API that says the key back in its error doesn't get it into
		// magpie's logs or the model's turn
		if a.Key != "" {
			msg = strings.ReplaceAll(msg, a.Key, provider.MaskSearchKey(a.Key))
		}
		if keyRefusal(res.StatusCode) {
			return nil, searchRefused{res.StatusCode, msg, res.Header.Get("Retry-After")}
		}
		return nil, errors.New(msg)
	}
	pages, err := readSearchAPI(a.Vendor, b)
	if err != nil {
		return nil, fmt.Errorf("unexpected reply: %w", err)
	}
	if len(pages) > searchHits {
		pages = pages[:searchHits]
	}
	return pages, nil
}

// readSearchAPI reads the pages out of a search API's reply.
func readSearchAPI(vendor string, b []byte) ([]foundPage, error) {
	if vendor == "you" {
		return readYouSearch(b)
	}
	type result struct {
		Title       string `json:"title"`
		URL         string `json:"url"`
		Content     string `json:"content"`
		Description string `json:"description"`
		Text        string `json:"text"`
		Markdown    string `json:"markdown"`
	}
	var out struct {
		Results []result `json:"results"` // tavily, exa, searxng
		Web     struct {
			Results []result `json:"results"`
		} `json:"web"` // brave
		Data json.RawMessage `json:"data"` // firecrawl: {web: […]}, or […] on its v1
	}
	if err := json.Unmarshal(b, &out); err != nil {
		return nil, err
	}
	rs := out.Results
	switch vendor {
	case "brave":
		rs = out.Web.Results
	case "firecrawl":
		var v2 struct {
			Web []result `json:"web"`
		}
		if json.Unmarshal(out.Data, &v2) != nil || v2.Web == nil {
			json.Unmarshal(out.Data, &rs)
		} else {
			rs = v2.Web
		}
	}
	var pages []foundPage
	for _, r := range rs {
		if r.URL == "" {
			continue
		}
		text := r.Content
		for _, t := range []string{r.Text, r.Description, r.Markdown} {
			if text == "" {
				text = t
			}
		}
		title := searchPlain(r.Title)
		if title == "" {
			title = r.URL
		}
		pages = append(pages, foundPage{Title: title, URL: r.URL, Text: clipText(searchPlain(text), pageText)})
	}
	return pages, nil
}

// readYouSearch reads You.com's reply, free or keyed alike: its pages
// under results.web, then results.news, each with a description and the
// snippets that matched.
func readYouSearch(b []byte) ([]foundPage, error) {
	type hit struct {
		Title       string   `json:"title"`
		URL         string   `json:"url"`
		Description string   `json:"description"`
		Snippets    []string `json:"snippets"`
		PageAge     string   `json:"page_age"`
	}
	var out struct {
		Results *struct {
			Web  []hit `json:"web"`
			News []hit `json:"news"`
		} `json:"results"`
	}
	if err := json.Unmarshal(b, &out); err != nil {
		return nil, err
	}
	if out.Results == nil {
		return nil, errors.New("no results in it")
	}
	var pages []foundPage
	for _, h := range append(out.Results.Web, out.Results.News...) {
		if h.URL == "" {
			continue
		}
		text := strings.Join(h.Snippets, " ")
		if text == "" {
			text = h.Description
		}
		title := searchPlain(h.Title)
		if title == "" {
			title = h.URL
		}
		pages = append(pages, foundPage{Title: title, URL: h.URL, Text: clipText(searchPlain(text), pageText), Age: h.PageAge})
	}
	return pages, nil
}

var searchTags = regexp.MustCompile(`<[^>]*>`)

// searchPlain is a search API's text without the tags it marks matches with
// (Brave's <strong>), on one line.
func searchPlain(s string) string {
	return strings.Join(strings.Fields(html.UnescapeString(searchTags.ReplaceAllString(s, ""))), " ")
}

// clipText cuts s to at most n bytes, at a rune.
func clipText(s string, n int) string {
	if len(s) <= n {
		return s
	}
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n] + "…"
}
