import { Text } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import {
  buildDiffModel,
  charRanges,
  groupChanges,
  newToOld,
  nextGroup,
  normalizeText,
  oldToNew,
} from "./diff-model";
import { parsePatch } from "./testing";

const doc = (s: string) => Text.of(s.split("\n"));

// 진짜 git 의 출력(`git diff -U3`, git 2.54 — 격리한 HOME · 빈 전역 설정): 빈 줄 하나 · `}` 한 줄을 사이에 둔 변경 — merge 의
// 덩어리는 이것을 합친다.
const PATCH = `diff --git a/x.rs b/x.rs
index 5cc13cb..98b8cd0 100644
--- a/x.rs
+++ b/x.rs
@@ -1,8 +1,8 @@
-fn one() {
+fn uno() {
${" "}
-    1
+    2
 }
-x
+y
 tail1
 tail2
 tail3
`;

const OLD = "fn one() {\n\n    1\n}\nx\ntail1\ntail2\ntail3\nend\n";
const NEW = "fn uno() {\n\n    2\n}\ny\ntail1\ntail2\ntail3\nend\n";

describe("buildDiffModel", () => {
  it("칠할 줄이 patch 의 - · + 줄과 같고, 빈 줄 · 짧은 문맥을 사이에 둔 묶음을 따로 둔다", () => {
    const [file] = parsePatch(PATCH);
    const model = buildDiffModel(file!);
    expect([...model.deleted]).toEqual(file!.deleted);
    expect([...model.added]).toEqual(file!.added);
    expect(model.groups).toEqual([
      { oldFrom: 1, oldTo: 2, newFrom: 1, newTo: 2 },
      { oldFrom: 3, oldTo: 4, newFrom: 3, newTo: 4 },
      { oldFrom: 5, oldTo: 6, newFrom: 5, newTo: 6 },
    ]);
  });

  it("더하기만 · 빼기만인 묶음은 빈 쪽의 자리를 그 줄 앞으로 둔다", () => {
    // 진짜 git 의 출력(`git diff -U0`, git 2.54).
    const [file] = parsePatch(`diff --git a/f b/f
index 213dc5f..f1e9e74 100644
--- a/f
+++ b/f
@@ -0,0 +1,2 @@
+a
+b
@@ -5,2 +6,0 @@ n3
-x
-y
`);
    expect(buildDiffModel(file!).groups).toEqual([
      { oldFrom: 1, oldTo: 1, newFrom: 1, newTo: 3 },
      { oldFrom: 5, oldTo: 7, newFrom: 7, newTo: 7 },
    ]);
  });

  it("merge 에 넘기는 변경은 묶음의 줄 범위(줄바꿈 포함)다", () => {
    const model = buildDiffModel(parsePatch(PATCH)[0]!);
    const a = doc(OLD);
    const b = doc(NEW);
    const [first] = groupChanges(model, a, b);
    expect(a.sliceString(first!.fromA, first!.toA)).toBe("fn one() {\n");
    expect(b.sliceString(first!.fromB, first!.toB)).toBe("fn uno() {\n");
  });

  it("글자 단위 강조는 바뀐 낱말만, 줄을 넘지 않는다", () => {
    const model = buildDiffModel(parsePatch(PATCH)[0]!);
    const a = doc(OLD);
    const b = doc(NEW);
    const { a: ra, b: rb } = charRanges(model.groups[0]!, a, b);
    expect(ra.map((r) => a.sliceString(r.from, r.to))).toEqual(["one"]);
    expect(rb.map((r) => b.sliceString(r.from, r.to))).toEqual(["uno"]);
  });

  it("줄 번호를 두 쪽 사이로 옮기고 다음 · 앞 묶음을 찾는다", () => {
    const [file] = parsePatch(`diff --git a/f b/f
--- a/f
+++ b/f
@@ -2,0 +3,2 @@
+n1
+n2
`);
    const model = buildDiffModel(file!);
    expect(oldToNew(model, 5)).toBe(7);
    expect(newToOld(model, 7)).toBe(5);
    expect(newToOld(model, 4)).toBe(3);
    expect(nextGroup(model, 0, 1)).toBe(0);
    expect(nextGroup(model, 3, 1)).toBeUndefined();
    expect(nextGroup(model, 9, -1)).toBe(0);
  });

  it("CRLF 는 줄 수를 바꾸지 않고 \\n 으로 보이고, 홀로 있는 CR 은 남긴다", () => {
    expect(normalizeText("a\r\nb\r\n")).toBe("a\nb\n");
    expect(normalizeText("a\rb")).toBe("a\rb");
  });
});
