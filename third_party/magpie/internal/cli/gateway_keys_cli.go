package cli

import (
	"cmp"
	"fmt"
	"io"
	"os"
	"slices"
	"strconv"
	"strings"
	"text/tabwriter"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/budget"
	"github.com/yetone/magpie/internal/provider"
)

func gatewayKeys(args []string) error { return gatewayKeysTo(os.Stdout, args) }

func gatewayKeysTo(out io.Writer, args []string) error {
	args = args[1:]
	if len(args) == 0 {
		args = []string{"list"}
	}
	action := args[0]
	if action == "limit" {
		access.MigrateLegacyLANKeyBestEffort()
		return gatewayKeyLimit(out, args[1:])
	}
	if action == "models" {
		access.MigrateLegacyLANKeyBestEffort()
		return gatewayKeyModels(out, args[1:])
	}
	if action == "accounts" {
		access.MigrateLegacyLANKeyBestEffort()
		return gatewayKeyAccounts(out, args[1:])
	}
	if action == "quotas" {
		access.MigrateLegacyLANKeyBestEffort()
		return gatewayKeyQuotas(out, args[1:])
	}
	// add <name> --key <value>: the key keeps a value its clients already
	// send (love1sbug on X); "--key -" reads it from stdin, out of the
	// shell's history
	var own string
	if action == "add" && len(args) == 4 && args[2] == "--key" {
		own = args[3]
		if own == "-" {
			b, err := io.ReadAll(io.LimitReader(os.Stdin, 4096))
			if err != nil {
				return err
			}
			own = strings.TrimSpace(string(b))
			if own == "" {
				return fmt.Errorf("no key came on stdin")
			}
		}
		args = args[:2]
	}
	if (action == "list" && len(args) != 1) || (action != "list" && len(args) != 2) {
		return fmt.Errorf("usage: magpie gateway-key list | add <name> [--key <value> | --key -] | rotate <id> | remove <id> | limit <id> [list | off | reset [<n>] | remove <n> | [add] day|week|month|<N>d|<N>h [--tokens N] [--cost USD] [--cache-reads]] | models <id> [all | <provider>/<model>|<provider>/* ...] | accounts <id> [all | <provider>/<account>|<provider>/<key id> ...] | quotas <id> [on | off]")
	}
	switch action {
	case "list", "add", "rotate", "remove":
	default:
		return fmt.Errorf("unknown gateway-key command %q", action)
	}
	access.MigrateLegacyLANKeyBestEffort()
	if action == "list" {
		keys, err := access.List()
		if err != nil {
			return err
		}
		w := tabwriter.NewWriter(out, 0, 4, 2, ' ', 0)
		fmt.Fprintln(w, "ID\tNAME\tSTATE\tGATEWAY KEY\tLIMIT\tMODELS\tACCOUNTS")
		now := time.Now()
		// an account is shown by who is signed in, kept by its stable id (#905)
		names := provider.AccountNames()
		for _, k := range keys {
			state := "enabled"
			if k.Off {
				state = "disabled"
			}
			models := "all"
			if len(k.Models) > 0 {
				models = strings.Join(k.Models, ",")
			}
			accounts := "all"
			if len(k.Accounts) > 0 {
				shown := make([]string, len(k.Accounts))
				for i, a := range k.Accounts {
					shown[i] = cmp.Or(names[a], a)
				}
				accounts = strings.Join(shown, ",")
			}
			fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%s\t%s\t%s\n", k.ID, strings.Map(func(r rune) rune {
				if r < 32 || r == 127 {
					return ' '
				}
				return r
			}, k.Name), state, k.Masked, limitWords(k, now), models, accounts)
		}
		return w.Flush()
	}
	in := access.Change{Key: args[1]}
	if action == "add" {
		in = access.Change{Name: args[1], Secret: own}
	}
	secret, err := access.Update(action+"-key", in)
	if err != nil {
		return err
	}
	if secret != "" {
		_, err = fmt.Fprintln(out, secret)
	} else {
		_, err = fmt.Fprintln(out, "Gateway key removed")
	}
	return err
}

// gatewayKeyLimit shows or sets a key's limits (#585, several since #2227):
//
//	magpie gateway-key limit <id> [list]                what it has used of each limit
//	magpie gateway-key limit <id> off                   no limit
//	magpie gateway-key limit <id> day|week|month|<N>d|<N>h [--tokens N] [--cost USD] [--cache-reads]
//	                                                    its only limit, or the one of several with that window
//	magpie gateway-key limit <id> add <window> [--tokens N] [--cost USD] [--cache-reads]
//	                                                    one more limit; a request passes only while every one has something left
//	magpie gateway-key limit <id> remove <n>            take off the nth limit
//	magpie gateway-key limit <id> reset [<n>]           count from 0 again, every limit or the nth; a new N-day or N-hour cycle from now (#1509)
func gatewayKeyLimit(out io.Writer, args []string) error {
	usage := fmt.Errorf("usage: magpie gateway-key limit <id> [off | reset [<n>] | remove <n> | [add] day|week|month|<N>d|<N>h [--tokens N] [--cost USD] [--cache-reads]]")
	if len(args) == 0 {
		return usage
	}
	id := args[0]
	if len(args) > 1 {
		keys, err := access.List()
		if err != nil {
			return err
		}
		i := slices.IndexFunc(keys, func(k access.Key) bool { return k.ID == id })
		if i < 0 {
			return fmt.Errorf("Key not found")
		}
		have := keys[i].Limits()
		// nth reads a limit's number as the list shows it, from 1
		nth := func(v string) (*int, error) {
			n, err := strconv.Atoi(v)
			if err != nil || n < 1 || n > len(have) {
				if len(have) == 0 {
					return nil, fmt.Errorf("%s has no limit", keys[i].Name)
				}
				return nil, fmt.Errorf("a limit's number is 1 to %d, as `magpie gateway-key limit %s` lists them: %q", len(have), id, v)
			}
			n--
			return &n, nil
		}
		switch args[1] {
		case "list":
			if len(args) > 2 {
				return usage
			}
		case "reset":
			in := access.Change{Key: id}
			if len(args) > 3 {
				return usage
			}
			if len(args) == 3 {
				if in.Index, err = nth(args[2]); err != nil {
					return err
				}
			}
			if _, err := access.Update("reset-limit-key", in); err != nil {
				return err
			}
		case "off":
			if len(args) > 2 {
				return usage
			}
			if _, err := access.Update("limit-key", access.Change{Key: id}); err != nil {
				return err
			}
		case "remove":
			if len(args) != 3 {
				return usage
			}
			n, err := nth(args[2])
			if err != nil {
				return err
			}
			if _, err := access.Update("limit-key", access.Change{Key: id, Index: n}); err != nil {
				return err
			}
		case "add":
			if len(args) < 3 {
				return usage
			}
			lim, err := parseKeyLimit(args[2], args[3:], usage)
			if err != nil {
				return err
			}
			n := len(have)
			if _, err := access.Update("limit-key", access.Change{Key: id, Limit: lim, Index: &n}); err != nil {
				return err
			}
		default:
			lim, err := parseKeyLimit(args[1], args[2:], usage)
			if err != nil {
				return err
			}
			in := access.Change{Key: id, Limit: lim}
			// of several, the one counted in the same window is set; the
			// only one is replaced, as before #2227
			if len(have) > 1 {
				v, _ := lim.Valid()
				n := slices.IndexFunc(have, func(l *access.Limit) bool {
					return v != nil && l.Period == v.Period && l.Days == v.Days && l.Hours == v.Hours
				})
				if n < 0 {
					return fmt.Errorf("%s has %d limits and none %s: add one with `magpie gateway-key limit %s add %s ...`, or take one off with `remove <n>`", keys[i].Name, len(have), perPhrase(v), id, args[1])
				}
				in.Index = &n
			}
			if _, err := access.Update("limit-key", in); err != nil {
				return err
			}
		}
	}
	keys, err := access.List()
	if err != nil {
		return err
	}
	i := slices.IndexFunc(keys, func(k access.Key) bool { return k.ID == id })
	if i < 0 {
		return fmt.Errorf("Key not found")
	}
	k, now := keys[i], time.Now()
	all := budget.All(k, now)
	if len(all) == 0 {
		_, err = fmt.Fprintf(out, "%s: no limit\n", k.Name)
		return err
	}
	w := tabwriter.NewWriter(out, 0, 4, 2, ' ', 0)
	fmt.Fprintf(w, "Key\t%s (%s)\n", k.Name, k.ID)
	for n, st := range all {
		if len(all) > 1 {
			fmt.Fprintf(w, "\t\n")
			fmt.Fprintf(w, "Limit %d\t%s, all must have something left\n", n+1, perPhrase(&access.Limit{Period: st.Period, Days: st.Days, Hours: st.Hours}))
		}
		fmt.Fprintf(w, "Window\tthis %s, %s – %s (local time)\n", (&access.Limit{Period: st.Period, Days: st.Days, Hours: st.Hours}).Per(), st.Start.Format("2006-01-02 15:04"), st.Reset.Format("2006-01-02 15:04"))
		if st.TokenLimit > 0 {
			counted := "input, output and cache writes"
			if st.CacheReads {
				counted += ", and cache reads"
			}
			fmt.Fprintf(w, "Tokens\t%d used of %d, %d left (%s)\n", st.Tokens, st.TokenLimit, st.TokensLeft, counted)
		} else {
			fmt.Fprintf(w, "Tokens\t%d used, no cap\n", st.Tokens)
		}
		unpriced := ""
		if st.Unpriced > 0 {
			unpriced = fmt.Sprintf(", %d calls without a known price not in it", st.Unpriced)
		}
		if st.CostLimit > 0 {
			fmt.Fprintf(w, "Cost\t$%.2f used of $%.2f, $%.2f left (estimate at Usage prices%s)\n", st.Cost, st.CostLimit, st.CostLeft, unpriced)
		} else {
			fmt.Fprintf(w, "Cost\t$%.2f used, no cap (estimate%s)\n", st.Cost, unpriced)
		}
		state := "open"
		if st.Spent {
			state = "spent: requests are refused until " + st.Reset.Format("2006-01-02 15:04")
		}
		fmt.Fprintf(w, "State\t%s\n", state)
	}
	return w.Flush()
}

// perPhrase is a limit's window as the CLI says it: "per week", "every 5
// hours".
func perPhrase(l *access.Limit) string {
	if l == nil {
		return ""
	}
	switch l.Period {
	case "days":
		return fmt.Sprintf("every %d days", l.Days)
	case "hours":
		return fmt.Sprintf("every %d hours", l.Hours)
	}
	return "per " + l.Period
}

// parseKeyLimit reads a limit as typed: its window, then --tokens,
// --cost and --cache-reads.
func parseKeyLimit(window string, rest []string, usage error) (*access.Limit, error) {
	lim := &access.Limit{Period: window}
	if n, ok := strings.CutSuffix(window, "d"); ok {
		days, err := strconv.Atoi(n)
		if err != nil || days < 1 || days > access.MaxDays {
			return nil, fmt.Errorf("an N-day cycle is a whole number of days from 1 to %d, like 10d: %q", access.MaxDays, window)
		}
		lim = &access.Limit{Period: "days", Days: days}
	} else if n, ok := strings.CutSuffix(window, "h"); ok {
		// every N hours (#2102), a subscription's 5-hour window's pace
		hours, err := strconv.Atoi(n)
		if err != nil || hours < 1 || hours > access.MaxHours {
			return nil, fmt.Errorf("an N-hour cycle is a whole number of hours from 1 to %d, like 5h: %q", access.MaxHours, window)
		}
		lim = &access.Limit{Period: "hours", Hours: hours}
	} else if !slices.Contains(access.Periods, lim.Period) || lim.Period == "days" || lim.Period == "hours" {
		return nil, fmt.Errorf("a limit's period is day, week, month, <N>d (every N days) or <N>h (every N hours), not %q", lim.Period)
	}
	for i := 0; i < len(rest); i++ {
		flag, val, has := strings.Cut(rest[i], "=")
		next := func() (string, error) {
			if has {
				return val, nil
			}
			if i+1 >= len(rest) {
				return "", fmt.Errorf("%s needs a value", flag)
			}
			i++
			return rest[i], nil
		}
		switch flag {
		case "--tokens":
			v, err := next()
			if err != nil {
				return nil, err
			}
			n, err := parseTokens(v)
			if err != nil || n < 0 {
				return nil, fmt.Errorf("--tokens is a count, like 1000000 or 2m: %q", v)
			}
			lim.Tokens = int64(n)
		case "--cost":
			v, err := next()
			if err != nil {
				return nil, err
			}
			if lim.Cost, err = strconv.ParseFloat(strings.TrimPrefix(v, "$"), 64); err != nil {
				return nil, fmt.Errorf("--cost is US dollars, like 5 or 2.50: %q", v)
			}
		case "--cache-reads":
			lim.CacheReads = true
		default:
			return nil, usage
		}
	}
	if !lim.Limited() {
		return nil, fmt.Errorf("say --tokens, --cost or both, or off for no limit")
	}
	return lim, nil
}

// gatewayKeyModels shows or sets the models a key may use (#882):
//
//	magpie gateway-key models <id>                          the models it may use
//	magpie gateway-key models <id> all                      every model
//	magpie gateway-key models <id> openai/gpt-5 anthropic/*  only these
func gatewayKeyModels(out io.Writer, args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("usage: magpie gateway-key models <id> [all | <provider>/<model>|<provider>/* ...]")
	}
	id := args[0]
	if len(args) > 1 {
		ms := args[1:]
		if len(ms) == 1 && ms[0] == "all" {
			ms = nil
		}
		if _, err := access.Update("models-key", access.Change{Key: id, Models: ms}); err != nil {
			return err
		}
	}
	keys, err := access.List()
	if err != nil {
		return err
	}
	i := slices.IndexFunc(keys, func(k access.Key) bool { return k.ID == id })
	if i < 0 {
		return fmt.Errorf("Key not found")
	}
	k := keys[i]
	if len(k.Models) == 0 {
		_, err = fmt.Fprintf(out, "%s: every model\n", k.Name)
		return err
	}
	_, err = fmt.Fprintf(out, "%s: only %s\n", k.Name, strings.Join(k.Models, ", "))
	return err
}

// limitWords is a key's limits in the list: "-" for none, else what it
// has used of each, "1200/1000000 tokens per day; $1.00/$20.00 per week".
func limitWords(k access.Key, now time.Time) string {
	all := budget.All(k, now)
	if len(all) == 0 {
		return "-"
	}
	var each []string
	for _, st := range all {
		var parts []string
		if st.TokenLimit > 0 {
			parts = append(parts, fmt.Sprintf("%d/%d tokens", st.Tokens, st.TokenLimit))
		}
		if st.CostLimit > 0 {
			parts = append(parts, fmt.Sprintf("$%.2f/$%.2f", st.Cost, st.CostLimit))
		}
		s := strings.Join(parts, " ") + " " + perPhrase(&access.Limit{Period: st.Period, Days: st.Days, Hours: st.Hours})
		if st.Spent {
			s += " (spent until " + st.Reset.Format("01-02 15:04") + ")"
		}
		each = append(each, s)
	}
	return strings.Join(each, "; ")
}

// gatewayKeyAccounts shows or sets a key's account whitelist (#905): the
// accounts and keys it may use, "<provider>/<account>" — a signed-in
// account by its name, a key by its KeyID — every one when none are set.
func gatewayKeyAccounts(out io.Writer, args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("usage: magpie gateway-key accounts <id> [all | <provider>/<account>|<provider>/<key id> ...]")
	}
	id := args[0]
	if len(args) > 1 {
		as := args[1:]
		if len(as) == 1 && as[0] == "all" {
			as = nil
		}
		if _, err := access.Update("accounts-key", access.Change{Key: id, Accounts: as}); err != nil {
			return err
		}
	}
	keys, err := access.List()
	if err != nil {
		return err
	}
	i := slices.IndexFunc(keys, func(k access.Key) bool { return k.ID == id })
	if i < 0 {
		return fmt.Errorf("Key not found")
	}
	k := keys[i]
	if len(k.Accounts) == 0 {
		_, err = fmt.Fprintf(out, "%s: every account\n", k.Name)
		return err
	}
	// shown by who is signed in, kept by the account's stable id (#905)
	names := provider.AccountNames()
	shown := make([]string, len(k.Accounts))
	for i, a := range k.Accounts {
		shown[i] = cmp.Or(names[a], a)
	}
	_, err = fmt.Fprintf(out, "%s: only %s\n", k.Name, strings.Join(shown, ", "))
	return err
}

// gatewayKeyQuotas shows or sets whether a key held to a budget, models
// or accounts is told the quotas of what it may use (#2216): the cards of
// the providers whose every model it may use, of its accounts and keys.
//
//	magpie gateway-key quotas <id>         whether it is
//	magpie gateway-key quotas <id> on|off  tell it them, or none (#1981)
func gatewayKeyQuotas(out io.Writer, args []string) error {
	usage := fmt.Errorf("usage: magpie gateway-key quotas <id> [on | off]")
	if len(args) == 0 || len(args) > 2 {
		return usage
	}
	id := args[0]
	if len(args) == 2 {
		if args[1] != "on" && args[1] != "off" {
			return usage
		}
		if _, err := access.Update("quotas-key", access.Change{Key: id, Quotas: args[1] == "on"}); err != nil {
			return err
		}
	}
	keys, err := access.List()
	if err != nil {
		return err
	}
	i := slices.IndexFunc(keys, func(k access.Key) bool { return k.ID == id })
	if i < 0 {
		return fmt.Errorf("Key not found")
	}
	k := keys[i]
	switch {
	case !k.Quotas:
		_, err = fmt.Fprintf(out, "%s: told no quotas when held to a budget, models or accounts\n", k.Name)
	default:
		_, err = fmt.Fprintf(out, "%s: told the quotas of the providers and accounts it may use\n", k.Name)
	}
	return err
}
