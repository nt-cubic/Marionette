// Regression for the multi-image annotation session helpers (src/lib/imageAttachments.ts).
// Run: npx tsx scripts/verify-image-annotator.mts
import {
  annotationProgress,
  applyMarksById,
  attachmentFromPath,
  markFromPending,
  marksByIdOf,
  type ImageMark,
} from "../src/lib/imageAttachments";

let failed = 0;
function check(name: string, ok: boolean, detail?: unknown) {
  if (ok) console.log("  ok ", name);
  else {
    failed += 1;
    console.log("  FAIL", name, detail ?? "");
  }
}

const a = attachmentFromPath("D:/shots/a.png");
const b = attachmentFromPath("D:/shots/b.png");
const c = { ...attachmentFromPath("D:/shots/c.png"), marks: [{ id: "keep", kind: "point", nx: 0.1, ny: 0.2, comment: "old" } as ImageMark] };
const list = [a, b, c];

// markFromPending
check("blank comment → no mark", markFromPending({ kind: "point", nx: 0.5, ny: 0.5 }, "   ") === null);
const p = markFromPending({ kind: "point", nx: 0.25, ny: 0.75 }, "  hello ");
check("point mark keeps coords and trims comment", p?.kind === "point" && p.nx === 0.25 && p.ny === 0.75 && p.comment === "hello", p);
const r = markFromPending({ kind: "rect", nx: 0.1, ny: 0.2, nw: 0.3, nh: 0.4 }, "box");
check("rect mark keeps size", r?.kind === "rect" && r.nw === 0.3 && r.nh === 0.4, r);
check("marks get distinct ids", p != null && r != null && p.id !== r.id);

// marksByIdOf / applyMarksById
const work = marksByIdOf(list);
check("working copy starts from each attachment's marks", work[c.id]?.length === 1 && work[a.id]?.length === 0);
const edited = { ...work, [a.id]: [p!], [b.id]: [r!] };
const next = applyMarksById(list, edited);
check("apply writes the marks of every image", next[0]?.marks[0]?.comment === "hello" && next[1]?.marks[0]?.comment === "box");
check("apply keeps untouched images and does not mutate input", next[2]?.marks[0]?.id === "keep" && a.marks.length === 0);
check("unknown ids are ignored", applyMarksById(list, { nope: [p!] }).every((x, i) => x === list[i]));
check("removing all marks of an image is applied", applyMarksById(list, { [c.id]: [] })[2]?.marks.length === 0);

// annotationProgress
const prog = annotationProgress(list, edited);
check("progress counts images with marks and total marks", prog.done === 3 && prog.total === 3 && prog.marks === 3, prog);
const none = annotationProgress([a, b], {});
check("progress with nothing marked", none.done === 0 && none.marks === 0 && none.total === 2, none);
check("progress falls back to attachment marks when not in the working copy", annotationProgress([c], {}).marks === 1);

console.log(failed === 0 ? "\n全部通过" : `\n${failed} 项失败`);
process.exit(failed === 0 ? 0 : 1);
