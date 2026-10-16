// The app deliberately undoes unsolicited scrollIntoView (including Playwright's
// automatic scroll). Bring a control into sight with real wheel input first.
// Wholly in sight: one cut off by a fraction of a pixel at the view's foot
// was scrolled the rest of the way by Playwright's click itself, before the
// app saw the click, and the page moved 1px under it (860 to 861 on the
// Usage page's Fetch from archive, once the archive switch in its header
// had moved the rows 4px down).
async function inView(page, locator) {
  await locator.waitFor({state:"visible"});
  if (await locator.evaluate(e => !e.closest("#view-usage"))) return;
  const view = page.locator("#view-usage");
  const v = await view.boundingBox();
  await page.mouse.move(v.x + v.width / 2, v.y + v.height / 2);
  for (let i = 0; i < 80; i++) {
    const b = await locator.boundingBox();
    if (!b) { await page.waitForTimeout(40); continue; }
    // one taller than the view: its top in sight
    if (b.y >= v.y && (b.y + b.height <= v.y + v.height || (b.height > v.height && b.y < v.y + v.height))) {
      await page.waitForTimeout(300);
      return;
    }
    const delta = b.y < v.y ? -20 : 20;
    await page.mouse.wheel(0, delta);
    await page.waitForTimeout(40);
  }
  throw new Error("control did not come into sight: " + JSON.stringify({view:v,target:await locator.boundingBox(),scroll:await view.evaluate(e=>e.scrollTop)}));
}

// reach is inView for any page: the reader wheels the view the control is
// in (its .view) until the control is wholly in that view and nothing drawn
// over it (a sticky bar, the window's foot) takes the point Playwright will
// press. Playwright's own scroll is put back by the app, as any scroll that
// isn't the reader's, and one partly in sight it doesn't scroll at all, so
// its press can wait on a control out of sight or under a bar till it times
// out (in WebKit; Chromium gets through some of these by a few pixels).
// Each wheel step waits for its scroll to land before the control is
// looked at again: a wheel's scroll reaches the page a frame or more after
// it is sent, and on a busy machine a fixed pause read where the control
// was before it. The next step was sent on that stale reading, the view
// went past the control, under the header, and the click that followed
// was on a control out of sight (#2370).
async function reach(page, locator) {
  await locator.waitFor({ state: "visible" });
  const id = await locator.evaluate((e) => e.closest(".view")?.id || "");
  if (!id) return;
  const v = await page.locator("#" + id).boundingBox();
  await page.mouse.move(v.x + v.width / 2, v.y + v.height / 2);
  let stalls = 0;
  for (let i = 0; i < 120; i++) {
    const at = await locator.evaluate((e) => {
      const b = e.getBoundingClientRect(), v = e.closest(".view").getBoundingClientRect();
      const x = b.left + b.width / 2, y = b.top + b.height / 2;
      const inside = b.top >= v.top && b.bottom <= v.bottom, hit = document.elementFromPoint(x, y);
      return { clear: inside && !!hit && (e === hit || e.contains(hit)), down: y > v.top + v.height / 2 };
    });
    if (at.clear) { await page.waitForTimeout(300); return; }
    // landed, or the view is at its end and the wheel moves nothing: three
    // of those in turn and the control can't be brought clear
    stalls = (await wheeled(page, id, at.down ? 20 : -20)) ? 0 : stalls + 1;
    if (stalls >= 3) break;
  }
  throw new Error("control did not come clear into sight: " + JSON.stringify({ view: v, target: await locator.boundingBox() }));
}

// wheeled sends one wheel over the view and waits for its scroll to land;
// false when the view didn't move in a second (it is at its end).
async function wheeled(page, id, dy) {
  const was = await page.locator("#" + id).evaluate((v) => v.scrollTop);
  await page.mouse.wheel(0, dy);
  return page.waitForFunction(([id, was]) => document.getElementById(id).scrollTop !== was, [id, was], { timeout: 1000 }).then(() => true, () => false);
}

// wheelTo scrolls the page the way a reader does, down till the control's
// top is at y or above, so a click that moved the page would show; then
// reach makes sure it is clear in sight. Each wheel lands before the
// control is looked at again (#2370).
async function wheelTo(page, locator, y) {
  await locator.waitFor({ state: "visible" });
  const id = await locator.evaluate((e) => e.closest(".view")?.id || "");
  const v = await page.locator("#" + id).boundingBox();
  await page.mouse.move(v.x + v.width / 2, v.y + v.height / 2);
  for (let i = 0; i < 80; i++) {
    if ((await locator.evaluate((e) => e.getBoundingClientRect().top)) <= y) break;
    if (!(await wheeled(page, id, 50))) break;
  }
  await reach(page, locator);
}

async function click(page, locator) {
  await inView(page, locator);
  await locator.click();
}

// Settings is drawn from each read of the settings, and opening the page
// reads them twice: the page's own read, then the window's load() once the
// state is in. Every row is replaced by the second draw, so a row found
// after the first is detached under whatever the test does to it next
// (#1653: scrollIntoViewIfNeeded in WebKit on a busy machine). Call
// watchSettingsReads before page.goto; settingsDrawn then waits until load()
// has asked for its read and every read asked for has been drawn. A page is
// watched once however many times it is opened.
const watched = new WeakSet();
async function watchSettingsReads(page) {
  if (watched.has(page)) return;
  watched.add(page);
  await page.addInitScript(() => {
    const reads = window.__settingsReads = { asked: 0, drawn: 0 };
    const fetch0 = window.fetch;
    window.fetch = function (input, init) {
      const p = fetch0.apply(this, arguments);
      const url = new URL(typeof input === "string" ? input : input.url, location.href);
      if (url.pathname !== "/api/settings" || (init?.method || "GET") !== "GET") return p;
      reads.asked++;
      // drawn a task after its body is in: loadSettings draws in the
      // microtasks that follow
      const drawn = () => setTimeout(() => reads.drawn++);
      return p.then((res) => {
        const text = res.text;
        res.text = function () { return text.call(this).finally(drawn); };
        return res;
      }, (e) => { drawn(); throw e; });
    };
  });
}

// load.done is set in the same run of load() that then asks for the
// settings, with no wait between, so once it is set the read is counted.
const settingsDrawn = (page) => page.waitForFunction(() => load.done && __settingsReads.asked === __settingsReads.drawn);

// openPart opens a part of Settings (a tab, or its entry in the sidebar's
// strip) the way the keyboard does. A click waits for the tab to hold still
// over two animation frames, and on a loaded machine WebKit draws no frames
// for seconds while the page itself runs on, so the click can sit out the
// test's whole timeout over a tab that isn't moving (#2150). Enter on the
// focused button clicks it with no frame to wait for.
async function openPart(page, id) {
  await page.locator(`:is(#setTab-${id}, .nav-s[data-of=setTab-${id}]):visible`).press("Enter");
}

module.exports = { inView, reach, wheelTo, click, openPart, watchSettingsReads, settingsDrawn };
