// In-page find (Ctrl F while the browser surface is focused).
//
// Runs in the agent world through the same install-on-demand shape as
// probe.js: the script is embedded in the evaluation that calls it, so a
// navigation between two calls installs a fresh copy instead of answering
// from a dead document's range table. Nothing here reloads or navigates the
// page, and nothing is sent to the conversation.
(function () {
  if (globalThis.__marionetteFind) return;

  var MAX_MATCHES = 2000;
  var state = {
    query: "",
    caseSensitive: false,
    ranges: [],
    index: -1,
    truncated: false
  };

  function ensureHighlightStyle() {
    var style = document.getElementById("marionette-find-style");
    if (style) return style;
    style = document.createElement("style");
    style.id = "marionette-find-style";
    style.textContent =
      "::highlight(marionette-find){background:rgba(255,213,0,.45);color:inherit;}" +
      "::highlight(marionette-find-current){background:rgba(255,153,0,.85);color:inherit;}";
    (document.head || document.documentElement).appendChild(style);
    return style;
  }

  function visible(node) {
    var el = node.parentElement;
    if (!el) return false;
    var tag = el.tagName;
    if (
      tag === "SCRIPT" ||
      tag === "STYLE" ||
      tag === "NOSCRIPT" ||
      tag === "TEMPLATE" ||
      tag === "TITLE"
    ) {
      return false;
    }
    if (el.closest('[hidden],[aria-hidden="true"]')) return false;
    try {
      if (
        typeof el.checkVisibility === "function" &&
        !el.checkVisibility({ checkOpacity: false, checkVisibilityCSS: true })
      ) {
        return false;
      }
    } catch (e) {
      // Older engines: keep the node; the browser's own find is no better.
    }
    return true;
  }

  function textNodes(root) {
    if (!root || !document.createTreeWalker) return [];
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: function (node) {
        return visible(node) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
      }
    });
    var nodes = [];
    var node;
    while ((node = walker.nextNode())) nodes.push(node);
    return nodes;
  }

  function build(query, caseSensitive) {
    state.query = query;
    state.caseSensitive = caseSensitive;
    state.ranges = [];
    state.index = -1;
    state.truncated = false;
    if (!query) return;
    var needle = caseSensitive ? query : query.toLowerCase();
    if (!needle) return;
    var nodes = textNodes(document.body);
    for (var i = 0; i < nodes.length; i++) {
      var text = nodes[i].nodeValue || "";
      var hay = caseSensitive ? text : text.toLowerCase();
      var from = 0;
      while (from <= hay.length) {
        var at = hay.indexOf(needle, from);
        if (at < 0) break;
        var range = document.createRange();
        range.setStart(nodes[i], at);
        range.setEnd(nodes[i], at + needle.length);
        state.ranges.push(range);
        if (state.ranges.length >= MAX_MATCHES) {
          state.truncated = true;
          return;
        }
        from = at + Math.max(1, needle.length);
      }
    }
  }

  function paint() {
    try {
      if (!globalThis.CSS || !CSS.highlights || typeof Highlight === "undefined") return;
      ensureHighlightStyle();
      CSS.highlights.delete("marionette-find");
      CSS.highlights.delete("marionette-find-current");
      if (state.ranges.length) {
        CSS.highlights.set("marionette-find", new Highlight(...state.ranges));
        if (state.index >= 0) {
          CSS.highlights.set(
            "marionette-find-current",
            new Highlight(state.ranges[state.index])
          );
        }
      }
    } catch (e) {
      // Highlight API unavailable in this world: the selection still moves.
    }
  }

  function reveal(index) {
    state.index = index;
    var selection = document.getSelection ? document.getSelection() : null;
    if (!selection) {
      paint();
      return;
    }
    if (index < 0 || !state.ranges[index]) {
      try {
        selection.removeAllRanges();
      } catch (e) {}
      paint();
      return;
    }
    var range = state.ranges[index];
    try {
      selection.removeAllRanges();
      selection.addRange(range);
      var rect = range.getBoundingClientRect();
      var vh = window.innerHeight || document.documentElement.clientHeight || 0;
      var vw = window.innerWidth || document.documentElement.clientWidth || 0;
      if (rect.height || rect.width) {
        if (rect.top < 0 || rect.bottom > vh || rect.left < 0 || rect.right > vw) {
          var top =
            (window.pageYOffset || document.documentElement.scrollTop || 0) +
            rect.top +
            rect.height / 2 -
            vh / 2;
          var left =
            (window.pageXOffset || document.documentElement.scrollLeft || 0) +
            rect.left +
            rect.width / 2 -
            vw / 2;
          window.scrollTo(Math.max(0, left), Math.max(0, top));
        }
      }
    } catch (e) {
      // Scrolling is a courtesy; the count and selection still work.
    }
    paint();
  }

  function answer() {
    return {
      count: state.ranges.length,
      index: state.index,
      truncated: state.truncated
    };
  }

  globalThis.__marionetteFind = {
    search: function (query, caseSensitive) {
      build(String(query || ""), !!caseSensitive);
      reveal(state.ranges.length ? 0 : -1);
      return answer();
    },
    step: function (backwards) {
      if (!state.ranges.length) {
        reveal(-1);
        return answer();
      }
      var n = state.ranges.length;
      var next = (state.index + (backwards ? -1 : 1)) % n;
      if (next < 0) next += n;
      reveal(next);
      return answer();
    },
    clear: function () {
      try {
        var selection = document.getSelection && document.getSelection();
        if (selection) selection.removeAllRanges();
      } catch (e) {}
      try {
        if (globalThis.CSS && CSS.highlights) {
          CSS.highlights.delete("marionette-find");
          CSS.highlights.delete("marionette-find-current");
        }
      } catch (e) {}
      var style = document.getElementById("marionette-find-style");
      if (style && style.parentNode) style.parentNode.removeChild(style);
      state = {
        query: "",
        caseSensitive: false,
        ranges: [],
        index: -1,
        truncated: false
      };
      return answer();
    }
  };
})();
