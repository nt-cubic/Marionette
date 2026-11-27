/**
 * verify-pasted-text-fold - P1-3 large-paste folding
 *
 * Guards the parts that are easy to regress without a real paste:
 *  - UTF-8 byte thresholds (CJK folds at ~10.9k chars, not 32k)
 *  - the draft ceiling respects selection replacement
 *  - Ctrl+Shift+V stays an inline bypass
 *  - one content-addressed reference, never a duplicate chip
 *  - staging failure keeps the draft and surfaces a localized message
 *  - the Rust storage command is registered and the Composer is wired to it
 *
 * Run: npx tsx scripts/verify-pasted-text-fold.mts
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  COMPOSER_TEXT_LIMIT_BYTES,
  PASTE_FOLD_BYTES,
  findPastedTextReferenceCaret,
  formatByteSize,
  insertReferenceAtSelection,
  isInlinePasteBypass,
  replaceSelection,
  shouldFoldPastedText,
  utf8ByteLength,
} from "../src/lib/pastedTextFold";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const composer = readFileSync(join(root, "src/components/Composer.tsx"), "utf8");
const api = readFileSync(join(root, "src/lib/api.ts"), "utf8");
const commands = readFileSync(join(root, "src-tauri/src/commands.rs"), "utf8");
const mainRs = readFileSync(join(root, "src-tauri/src/main.rs"), "utf8");
const zh = readFileSync(join(root, "src/locales/parts/fe-composer.zh.ts"), "utf8");
const en = readFileSync(join(root, "src/locales/parts/fe-composer.en.ts"), "utf8");

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` - ${detail}` : ""}`);
  }
}

// 1. UTF-8 measurement, not JS code units.
check("ASCII byte length", utf8ByteLength("abc") === 3);
check("CJK byte length is 3 per character", utf8ByteLength("\u4e2d") === 3);
check("astral emoji byte length is 4", utf8ByteLength("\u{1F389}") === 4);

check(
  "32 KiB CJK paste folds (10,923 chars = 32,769 bytes)",
  shouldFoldPastedText("\u4e2d".repeat(10923), "", 0, 0) === true,
);
check(
  "just-under-32 KiB CJK paste stays inline (10,922 chars)",
  shouldFoldPastedText("\u4e2d".repeat(10922), "", 0, 0) === false,
);
check(
  "32 KiB ASCII paste folds",
  shouldFoldPastedText("x".repeat(PASTE_FOLD_BYTES), "", 0, 0) === true,
);
check(
  "one byte under the paste threshold stays inline",
  shouldFoldPastedText("x".repeat(PASTE_FOLD_BYTES - 1), "", 0, 0) === false,
);

// 2. The draft ceiling uses the post-replacement value.
const shortPaste = "x".repeat(1000);
check(
  "inserting into a full draft folds",
  shouldFoldPastedText(shortPaste, "y".repeat(COMPOSER_TEXT_LIMIT_BYTES), 0, 0) === true,
);
check(
  "reaching exactly the ceiling does not count as exceeding it",
  shouldFoldPastedText(
    shortPaste,
    "y".repeat(COMPOSER_TEXT_LIMIT_BYTES - shortPaste.length),
    0,
    0,
  ) === false,
);
check(
  "one byte past the ceiling folds",
  shouldFoldPastedText(
    shortPaste,
    "y".repeat(COMPOSER_TEXT_LIMIT_BYTES - shortPaste.length + 1),
    0,
    0,
  ) === true,
);
check(
  "replacing a large selection uses the replaced range for the ceiling",
  shouldFoldPastedText(
    shortPaste,
    "y".repeat(COMPOSER_TEXT_LIMIT_BYTES),
    0,
    5000,
  ) === false,
);

// 3. Selection replacement semantics.
check(
  "replaceSelection replaces only the selected range",
  replaceSelection("hello world", 6, 11, "there") === "hello there",
);
check(
  "reversed selection is normalised",
  replaceSelection("hello world", 11, 6, "there") === "hello there",
);

const tailReplace = insertReferenceAtSelection("hello world", 6, 11, "[ref]");
check(
  "reference replaces a tail selection",
  tailReplace.value === "hello [ref]" && tailReplace.caret === 11,
  tailReplace.value,
);
const middleReplace = insertReferenceAtSelection("abcdef", 2, 4, "[ref]");
check(
  "reference replaces a middle selection and drops the old text",
  middleReplace.value === "ab\n[ref]\nef" &&
    !middleReplace.value.includes("cd") &&
    middleReplace.caret === 9,
  middleReplace.value,
);
check(
  "reference insertion pads adjacent prose with line breaks",
  insertReferenceAtSelection("abc", 3, 3, "[ref]").value === "abc\n[ref]",
);

// 4. Inline bypass and reference dedupe.
check("Ctrl+Shift+V is an inline bypass", isInlinePasteBypass({ ctrlKey: true, metaKey: false, shiftKey: true }));
check("Cmd+Shift+V is an inline bypass", isInlinePasteBypass({ ctrlKey: false, metaKey: true, shiftKey: true }));
check("Ctrl+V is not a bypass", isInlinePasteBypass({ ctrlKey: true, metaKey: false, shiftKey: false }) === false);
check("Shift+Insert style shift alone is not a bypass", isInlinePasteBypass({ ctrlKey: false, metaKey: false, shiftKey: true }) === false);

const path = "C:\\Users\\me\\.marionette\\attachments\\pasted-text\\paste-1234.txt";
const draftWithRef = `hello\n[Pasted text attachment - 32.0 KiB] ${path}\nworld`;
check(
  "existing reference is found at its line end",
  findPastedTextReferenceCaret(draftWithRef, path) === draftWithRef.indexOf(path) + path.length,
);
check("missing reference returns -1", findPastedTextReferenceCaret("hello", path) === -1);

// 5. Size labels match binary thresholds.
check("1 KiB label", formatByteSize(1024) === "1.0 KiB");
check("32 KiB label", formatByteSize(PASTE_FOLD_BYTES) === "32.0 KiB");
check("1 MiB label", formatByteSize(1024 * 1024) === "1.0 MiB");
check("small byte label", formatByteSize(500) === "500 B");

// 6. Wiring checks (source contracts this slice must keep).
check("Composer imports the folding helper", composer.includes("from \"../lib/pastedTextFold\""));
check("Composer stages with savePastedText", composer.includes("await savePastedText(pastedText)"));
check("Composer checks the inline bypass before staging", composer.includes("isInlinePasteBypass(event)"));
check(
  "Composer folds before taking over the paste",
  composer.indexOf("shouldFoldPastedText(pastedText") > 0 &&
    composer.indexOf("shouldFoldPastedText(pastedText") <
      composer.indexOf("event.preventDefault();", composer.indexOf("shouldFoldPastedText(pastedText")),
);
check(
  "Composer refuses a duplicate reference while staging",
  composer.includes("findPastedTextReferenceCaret(current, saved.path)") &&
    composer.includes("return current;"),
);
check(
  "staging failure names a localized reason",
  composer.includes("t(\"composer.pasteTextFailed\", { reason })") &&
    composer.includes("composer.pasteTextNeedsDesktop"),
);
check("Composer keeps the dropped-path bookkeeping", composer.includes("droppedPathsRef.current.add(saved.path);"));

check("API wrapper posts save_pasted_text", api.includes("save_pasted_text") && api.includes("savePastedText"));
check("Rust command exists", commands.includes("pub fn save_pasted_text(text: String)"));
check("Rust command is registered", mainRs.includes("commands::save_pasted_text"));
check(
  "Rust storage is Marionette-owned and outside the project tree",
  commands.includes("join(\"attachments\")") && commands.includes("join(\"pasted-text\")"),
);
check("Rust staging is content-addressed", commands.includes("Sha256::digest(bytes)"));

for (const key of [
  "composer.pasteTextReference",
  "composer.pasteTextFailed",
  "composer.pasteTextNeedsDesktop",
  "composer.pasteTextAlreadyAttached",
]) {
  check(`zh locale has ${key}`, zh.includes(`${key}`));
  check(`en locale has ${key}`, en.includes(`${key}`));
}

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log("\npasted-text fold: all checks passed");
