package gateway

// The browser MCP server's tools, carried out by the extension
// (internal/browserext/ext/background.js), whose argument names these
// schemas give.

const browserInstructions = `These tools use the user's own browser (Chrome, Edge or another Chromium browser with Magpie Browser installed): its real tabs, signed in to the sites the user is signed in to. Use it when a task needs the user's sessions or a real browser.
How to work:
- Start with browser_tabs or browser_snapshot. Calls go to the tab the user is looking at until you open or select another; tabs you open go in the "Magpie" tab group.
- browser_snapshot shows the page as an accessibility tree. Elements you can act on carry a ref (e12); pass it to browser_click, browser_type and the rest. Refs stay the element's while the page keeps it. Every action answers with the page after it, so you rarely need another snapshot.
- Prefer refs to browser_evaluate. Use browser_screenshot when the layout or an image matters.
- This is the user's browser: don't close tabs you didn't open, don't change their settings, and ask before sending, buying, posting or deleting anything on their behalf. When a site wants a sign-in, a captcha or a 2FA code, ask the user to do it in the tab, then go on.`

func browserTabArg() map[string]any {
	return map[string]any{"type": "string", "description": "The tab to act in (t123, from browser_tabs). Without it: the tab this session works in, or the user's current tab."}
}

func browserRefArg(what string) map[string]any {
	return map[string]any{"type": "string", "description": what + " (a ref from the latest snapshot, like e12)"}
}

func browserTool(name, title, desc string, readOnly bool, props map[string]any, required ...string) map[string]any {
	props["tab"] = browserTabArg()
	schema := map[string]any{"type": "object", "properties": props}
	if len(required) > 0 {
		schema["required"] = required
	}
	return map[string]any{
		"name": name, "title": title, "description": desc, "inputSchema": schema,
		"annotations": map[string]any{"readOnlyHint": readOnly, "openWorldHint": true},
	}
}

func browserTools() []any {
	str := func(desc string) map[string]any { return map[string]any{"type": "string", "description": desc} }
	boolean := func(desc string) map[string]any { return map[string]any{"type": "boolean", "description": desc} }
	num := func(desc string) map[string]any { return map[string]any{"type": "number", "description": desc} }
	enum := func(desc string, vals ...string) map[string]any {
		return map[string]any{"type": "string", "enum": vals, "description": desc}
	}
	strs := func(desc string) map[string]any {
		return map[string]any{"type": "array", "items": map[string]any{"type": "string"}, "description": desc}
	}
	return []any{
		browserTool("browser_tabs", "Tabs", "List the browser's tabs, open a new one (in the Magpie tab group), switch the tab calls go to, or close one.", false, map[string]any{
			"action":     enum("list (default), new, select or close", "list", "new", "select", "close"),
			"url":        str("For new: the page to open in it."),
			"background": boolean("For new: open it without bringing it to the front."),
		}),
		browserTool("browser_navigate", "Navigate", "Open a URL in the tab, or go back, forward or reload. Answers with the page once it has loaded.", false, map[string]any{
			"url": str("The address to open (https:// is added when it has no scheme)."),
			"go":  enum("Instead of url: back, forward or reload.", "back", "forward", "reload"),
		}),
		browserTool("browser_snapshot", "Snapshot", "Read the page as an accessibility tree, each element you can act on with a ref.", true, map[string]any{
			"interactive": boolean("Only the elements you can act on, and headings: a much shorter read of a long page."),
			"ref":         browserRefArg("Read only this part of the page"),
			"maxChars":    num("The most characters to return (default 60000)."),
		}),
		browserTool("browser_click", "Click", "Click an element, as the user would with the mouse.", false, map[string]any{
			"ref":       browserRefArg("The element"),
			"double":    boolean("Double-click."),
			"button":    enum("left (default), right or middle", "left", "right", "middle"),
			"modifiers": map[string]any{"type": "array", "items": map[string]any{"type": "string", "enum": []string{"Alt", "Control", "Meta", "Shift"}}, "description": "Keys held while clicking."},
		}, "ref"),
		browserTool("browser_type", "Type", "Type text into a field or an editable element.", false, map[string]any{
			"ref":    browserRefArg("The field"),
			"text":   str("What to type."),
			"clear":  boolean("Replace what the field holds (default true); false adds to it."),
			"submit": boolean("Press Enter after."),
			"slowly": boolean("Type a key at a time, for a field that reacts to each key (an autocomplete)."),
		}, "ref", "text"),
		browserTool("browser_press", "Press key", "Press a key or a chord: Enter, Escape, Tab, ArrowDown, PageDown, Backspace, Control+a, Meta+Shift+k.", false, map[string]any{
			"key": str("The key or chord."),
			"ref": browserRefArg("Focus this element first"),
		}, "key"),
		browserTool("browser_hover", "Hover", "Move the mouse over an element, to open a menu or a tooltip.", false, map[string]any{
			"ref": browserRefArg("The element"),
		}, "ref"),
		browserTool("browser_select", "Select option", "Choose options in a native select list, by value or label.", false, map[string]any{
			"ref":    browserRefArg("The list"),
			"values": strs("The options to choose."),
		}, "ref", "values"),
		browserTool("browser_scroll", "Scroll", "Scroll the page or an element, or bring an element into view (ref with no direction).", false, map[string]any{
			"direction": enum("down (default), up, left or right", "down", "up", "left", "right"),
			"amount":    num("Pixels; default most of the window."),
			"ref":       browserRefArg("Scroll inside this element, or into view without a direction"),
		}),
		browserTool("browser_drag", "Drag", "Drag one element onto another.", false, map[string]any{
			"from": browserRefArg("What to drag"),
			"to":   browserRefArg("Where to drop it"),
		}, "from", "to"),
		browserTool("browser_upload", "Upload files", "Choose files in a file field, or in the picker a button opens.", false, map[string]any{
			"ref":   browserRefArg("The file field or the button that opens the picker"),
			"paths": strs("The files' full paths on this computer."),
		}, "ref", "paths"),
		browserTool("browser_wait", "Wait", "Wait for text to show or go, for the address to change, or a number of seconds.", true, map[string]any{
			"text":     str("Wait until the page shows this text."),
			"textGone": str("Wait until the page no longer shows this text."),
			"url":      str("Wait until the address contains this."),
			"seconds":  num("Just wait this long (at most 60)."),
			"timeout":  num("Give up after this many seconds (default 10, at most 60)."),
		}),
		browserTool("browser_screenshot", "Screenshot", "Take a screenshot of the window, the whole page or one element.", true, map[string]any{
			"ref":      browserRefArg("Only this element"),
			"fullPage": boolean("The whole page, not only what the window shows."),
			"png":      boolean("PNG instead of JPEG."),
		}),
		browserTool("browser_evaluate", "Run script", "Run JavaScript in the page and return its JSON result. A function is called (with the element when ref is given); an expression is evaluated.", false, map[string]any{
			"script": str("An expression, or a function like () => document.title or (el) => el.value."),
			"ref":    browserRefArg("The element the function is called with"),
		}, "script"),
		browserTool("browser_text", "Page text", "Read the page's visible text, without the tree.", true, map[string]any{
			"maxChars": num("The most characters to return (default 40000)."),
		}),
		browserTool("browser_console", "Console", "Read the tab's console messages, script errors and failed requests since magpie started watching it.", true, map[string]any{
			"level": enum("all (default), errors or network", "all", "errors", "network"),
			"limit": num("The most recent messages to return (default 50)."),
		}),
		browserTool("browser_dialog", "Dialog", "Answer the page's alert, confirm or prompt dialog; with none open, answer the next one.", false, map[string]any{
			"accept":     boolean("Accept (default true) or dismiss."),
			"promptText": str("For a prompt: the text to answer with."),
		}),
	}
}

func browserToolNamed(name string) bool {
	for _, t := range browserTools() {
		if t.(map[string]any)["name"] == name {
			return true
		}
	}
	return false
}
