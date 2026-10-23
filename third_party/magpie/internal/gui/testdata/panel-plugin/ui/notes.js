// A panel in each provider's editor: magpie.context.provider says which
// provider it is in, and the note is kept in the plugin's own options.
(async () => {
  await magpie.ready;
  const id = magpie.context.provider.id;
  let opts = await magpie.options.get();
  const box = document.getElementById("note");
  box.value = opts.notes?.[id] || "";
  let timer = 0;
  // options are set once a second at most: the last keystroke's, a
  // little after it
  box.oninput = () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      opts = await magpie.options.set({ ...opts, notes: { ...opts.notes, [id]: box.value } });
      document.getElementById("saved").textContent = "Saved";
    }, 1200);
  };
})();
