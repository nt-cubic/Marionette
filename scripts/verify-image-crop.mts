/** Pure crop geometry used by the image attachment dialog. */
import assert from "node:assert/strict";
import {
  FULL_IMAGE_CROP,
  clampImageCropRect,
  isFullImageCrop,
  remapMarksForCrop,
  resizeImageCrop,
  type ImageMark,
} from "../src/lib/imageAttachments.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

check("full image starts as an unchanged crop", () => {
  assert.equal(isFullImageCrop(FULL_IMAGE_CROP), true);
  assert.equal(isFullImageCrop(resizeImageCrop(FULL_IMAGE_CROP, "w", 0, 0)), true);
});

check("each edge resizes from its own side", () => {
  assert.deepEqual(resizeImageCrop(FULL_IMAGE_CROP, "w", 0.2, 0), {
    nx: 0.2,
    ny: 0,
    nw: 0.8,
    nh: 1,
  });
  assert.deepEqual(resizeImageCrop(FULL_IMAGE_CROP, "e", -0.25, 0), {
    nx: 0,
    ny: 0,
    nw: 0.75,
    nh: 1,
  });
  assert.deepEqual(resizeImageCrop(FULL_IMAGE_CROP, "n", 0, 0.3), {
    nx: 0,
    ny: 0.3,
    nw: 1,
    nh: 0.7,
  });
  assert.deepEqual(resizeImageCrop(FULL_IMAGE_CROP, "s", 0, -0.4), {
    nx: 0,
    ny: 0,
    nw: 1,
    nh: 0.6,
  });
});

check("corners move both axes and cannot cross", () => {
  const rect = resizeImageCrop(FULL_IMAGE_CROP, "nw", 0.35, 0.2, 0.02);
  assert.equal(rect.nx, 0.35);
  assert.equal(rect.ny, 0.2);
  assert.equal(rect.nw, 0.65);
  assert.equal(rect.nh, 0.8);

  const clamped = resizeImageCrop(FULL_IMAGE_CROP, "w", 2, 0, 0.1);
  assert.ok(clamped.nw >= 0.1 - 1e-9, JSON.stringify(clamped));
  assert.ok(clamped.nx + clamped.nw <= 1, JSON.stringify(clamped));
});

check("clamp keeps odd values inside the image", () => {
  assert.deepEqual(clampImageCropRect({ nx: 1.2, ny: -1, nw: 0.5, nh: 2 }), {
    nx: 0.5,
    ny: 0,
    nw: 0.5,
    nh: 1,
  });
});

check("marks are remapped into cropped coordinates", () => {
  const crop = { nx: 0.25, ny: 0.1, nw: 0.5, nh: 0.8 };
  const marks: ImageMark[] = [
    { id: "p", kind: "point", nx: 0.5, ny: 0.5, comment: "center" },
    { id: "r", kind: "rect", nx: 0.2, ny: 0.2, nw: 0.4, nh: 0.4, comment: "clipped" },
    { id: "out", kind: "point", nx: 0.9, ny: 0.9, comment: "outside" },
  ];
  const out = remapMarksForCrop(marks, crop);
  assert.equal(out.length, 2);
  assert.deepEqual(out.find((m) => m.id === "p"), {
    id: "p",
    kind: "point",
    nx: 0.5,
    ny: 0.5,
    comment: "center",
  });
  const rect = out.find((m) => m.id === "r");
  assert.ok(rect && rect.kind === "rect");
  assert.equal(rect.nx, 0);
  assert.ok(Math.abs(rect.nw - 0.7) < 1e-9);
  assert.ok(Math.abs(rect.ny - 0.125) < 1e-9);
  assert.ok(Math.abs(rect.nh - 0.5) < 1e-9);
});

console.log(`\nimage crop: ${passed} checks passed.`);
