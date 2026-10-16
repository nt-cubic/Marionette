package cli

import (
	"bufio"
	"errors"
	"fmt"
	"maps"
	"os"
	"slices"
	"strings"

	"golang.org/x/term"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/edit"
)

// backupCmd writes every provider, setting, profile and agent model, and
// the library, to one file sealed with a passphrase:
// magpie backup [--no-keys] [--no-library] [file].
func backupCmd(args []string) error {
	keys, lib, file := true, true, ""
	for _, a := range args {
		switch {
		case a == "--no-keys":
			keys = false
		case a == "--no-library":
			lib = false
		case strings.HasPrefix(a, "-"):
			return fmt.Errorf("unknown flag %s (magpie backup [--no-keys] [--no-library] [file])", a)
		case file == "":
			file = a
		default:
			return errors.New("usage: magpie backup [--no-keys] [--no-library] [file]")
		}
	}
	if file == "" {
		file = "magpie" + backup.Ext
	}
	b, err := backup.Collect(keys, version)
	if err != nil {
		return err
	}
	if !lib {
		b.Library = nil
	}
	pass, err := passphrase("Passphrase for the backup: ", true)
	if err != nil {
		return err
	}
	data, err := backup.Seal(b, pass)
	if err != nil {
		return err
	}
	if err := edit.WriteAtomic(file, data); err != nil {
		return err
	}
	os.Chmod(file, 0o600)
	what := "with their keys"
	if !keys {
		what = "without keys"
	}
	fmt.Printf("%s %s: %d providers %s, %d profiles, %d agent settings",
		green.Render("saved"), file, len(b.Providers), what, len(b.Profiles), len(b.Agents))
	if l := b.Library; l != nil {
		fmt.Printf("; library: %d MCP servers, %d skills", len(l.MCP), len(l.Skills))
	}
	fmt.Println()
	fmt.Println(muted.Render("Subscriptions are not in it: sign in to them on the other machine. Keep the passphrase: without it the file can't be opened."))
	return nil
}

// restoreCmd puts a backup in: magpie restore [--no-agents] [--no-library] <file>.
func restoreCmd(args []string) error {
	parts, file := backup.All, ""
	for _, a := range args {
		switch {
		case a == "--no-agents":
			parts.Agents = false
		case a == "--no-library":
			parts.Library = false
		case strings.HasPrefix(a, "-"):
			return fmt.Errorf("unknown flag %s (magpie restore [--no-agents] [--no-library] <file>)", a)
		case file == "":
			file = a
		default:
			return errors.New("usage: magpie restore [--no-agents] [--no-library] <file>")
		}
	}
	if file == "" {
		return errors.New("usage: magpie restore [--no-agents] [--no-library] <file>")
	}
	data, err := os.ReadFile(file)
	if err != nil {
		return err
	}
	pass, err := passphrase("Passphrase: ", false)
	if err != nil {
		return err
	}
	b, err := backup.Open(data, pass)
	if err != nil {
		return err
	}
	r, err := backup.Restore(b, parts)
	if err != nil {
		return err
	}
	fmt.Printf("%s providers: %d added, %d replaced", green.Render("restored"), r.Added, r.Replaced)
	if r.Settings {
		fmt.Print("; settings")
	}
	fmt.Printf("; %d profiles; %d agent settings changed", r.Profiles, r.Agents)
	if r.Library {
		fmt.Print("; library")
	}
	fmt.Println()
	if len(r.NeedKey) > 0 {
		fmt.Println(muted.Render("Needs a key (magpie provider key <id>): " + strings.Join(r.NeedKey, ", ")))
	}
	if len(r.SignIn) > 0 {
		fmt.Println(muted.Render("The backup's settings use these, not signed in on this computer (a backup carries settings, not sign-ins; to use one here: magpie accounts add <id>): " + strings.Join(r.SignIn, ", ")))
	}
	if len(r.Plugins) > 0 {
		fmt.Println(muted.Render("Plugins added: " + strings.Join(r.Plugins, ", ")))
	}
	for _, spec := range slices.Sorted(maps.Keys(r.PluginErrors)) {
		fmt.Println(muted.Render(fmt.Sprintf("Couldn't add the plugin %s: %s", spec, r.PluginErrors[spec])))
	}
	for _, p := range r.LibraryProblems {
		fmt.Println(muted.Render(fmt.Sprintf("Library: %s couldn't get %s: %s", p.Agent, p.What, p.Error)))
	}
	if len(r.Skipped) > 0 {
		fmt.Println(muted.Render("Left as they are (agent not here, or its model can't be reached yet): " + strings.Join(r.Skipped, ", ")))
	}
	return nil
}

// passphrase reads one from the terminal without echo, asked twice when
// it is being set; piped in, it is the next line of stdin.
func passphrase(prompt string, confirm bool) (string, error) {
	return secret("passphrase", prompt, confirm)
}

// stdin is read through one reader, so a second secret piped in is the
// second line and not lost to the first one's buffer.
var stdin = bufio.NewReader(os.Stdin)

// secret reads what (a passphrase, a password) as passphrase does; piped
// in, it is the next line of stdin.
func secret(what, prompt string, confirm bool) (string, error) {
	fd := int(os.Stdin.Fd())
	if !term.IsTerminal(fd) {
		line, err := stdin.ReadString('\n')
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			if err != nil {
				return "", fmt.Errorf("no %s on stdin: %w", what, err)
			}
			return "", fmt.Errorf("the %s is empty", what)
		}
		return line, nil
	}
	read := func(p string) (string, error) {
		fmt.Fprint(os.Stderr, p)
		b, err := term.ReadPassword(fd)
		fmt.Fprintln(os.Stderr)
		return string(b), err
	}
	pass, err := read(prompt)
	if err != nil {
		return "", err
	}
	if pass == "" {
		return "", fmt.Errorf("the %s is empty", what)
	}
	if confirm {
		again, err := read("Again: ")
		if err != nil {
			return "", err
		}
		if again != pass {
			return "", fmt.Errorf("the %ss differ", what)
		}
	}
	return pass, nil
}
