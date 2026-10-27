// @vitest-environment happy-dom
// diff 뷰 컴포넌트(M2-15): 칠한 줄 · 글자 표시 · 보기 전환의 스크롤 자리 · F7 이동 · 확장 지점.

import { StateField } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { EditorView as View } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { buildDiffModel } from "./diff-model";
import {
  activeDiff,
  type DiffContent,
  type DiffController,
  type DiffMode,
  DiffView,
} from "./diff-view";
import { fileOf, paintedLines, parsePatch } from "./testing";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// 진짜 git 의 출력(git 2.54, 격리한 HOME · 빈 전역 설정 — diff-model.test.ts 와 같은 것).
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

const CONTENT: DiffContent = {
  file: fileOf(PATCH),
  oldText: "fn one() {\n\n    1\n}\nx\ntail1\ntail2\ntail3\nend\n",
  newText: "fn uno() {\n\n    2\n}\ny\ntail1\ntail2\ntail3\nend\n",
  language: null,
};

const PHRASES = { "$ unchanged lines": "$ unchanged" };

let cleanup: (() => void) | undefined;

beforeEach(() => {
  // happy-dom 은 선택을 바꾸는 그 자리에서 `selectionchange` 를 동기로 보낸다 — CodeMirror 의 업데이트 도중에 온 것은 버린다
  // (src/editor/editor-ops.test.tsx 와 같은 까닭).
  const dispatch = document.dispatchEvent.bind(document);
  vi.spyOn(document, "dispatchEvent").mockImplementation((e) => {
    const updating = [...document.querySelectorAll<HTMLElement>(".cm-editor")].some(
      (dom) =>
        (View.findFromDOM(dom) as unknown as { updateState: number } | null)?.updateState !== 0,
    );
    return e.type === "selectionchange" && updating ? true : dispatch(e);
  });
});

afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  vi.restoreAllMocks();
});

function mount(
  content: DiffContent,
  initial: DiffMode,
  slots?: Parameters<typeof DiffView>[0]["slots"],
) {
  const host = document.createElement("div");
  host.style.height = "400px";
  document.body.append(host);
  const root = createRoot(host);
  const state: { controller?: DiffController; setMode?: (m: DiffMode) => void } = {};
  function Harness() {
    const [mode, setMode] = useState(initial);
    state.setMode = setMode;
    return (
      <DiffView
        content={content}
        mode={mode}
        phrases={PHRASES}
        slots={slots}
        onController={(c) => {
          if (c) state.controller = c;
        }}
      />
    );
  }
  act(() => root.render(<Harness />));
  cleanup = () => {
    act(() => root.unmount());
    host.remove();
  };
  return { host, state };
}

describe("DiffView", () => {
  it.each(["split", "unified"] as const)(
    "%s: patch 의 - · + 줄만 칠한다 — 묶음 사이의 빈 줄 · `}` 줄은 칠하지 않는다",
    (mode) => {
      const { host, state } = mount(CONTENT, mode);
      const painted = paintedLines(host, state.controller!);
      const [p] = parsePatch(PATCH);
      expect(painted.deleted).toEqual(p!.deleted);
      expect(painted.added).toEqual(p!.added);
    },
  );

  it("나란히 보기의 바뀐 낱말에 글자 표시를 단다", () => {
    const { state } = mount(CONTENT, "split");
    const [a, b] = state.controller!.views();
    const words = (v: EditorView) =>
      [...v.contentDOM.querySelectorAll(".pw-diff-char-changed")].map((e) => e.textContent);
    expect(words(a!)).toEqual(["one", "1", "x"]);
    expect(words(b!)).toEqual(["uno", "2", "y"]);
  });

  it("통합 ↔ 나란히 전환에 화면 맨 위 줄이 남는다", async () => {
    const lines = Array.from({ length: 400 }, (_, i) => `line ${i + 1}`);
    const changed = [...lines];
    changed[199] = "changed 200";
    changed[299] = "changed 300";
    const patch = [
      "diff --git a/big.txt b/big.txt",
      "--- a/big.txt",
      "+++ b/big.txt",
      "@@ -197,7 +197,7 @@",
      ...lines.slice(196, 199).map((l) => ` ${l}`),
      "-line 200",
      "+changed 200",
      ...lines.slice(200, 203).map((l) => ` ${l}`),
      "@@ -297,7 +297,7 @@",
      ...lines.slice(296, 299).map((l) => ` ${l}`),
      "-line 300",
      "+changed 300",
      ...lines.slice(300, 303).map((l) => ` ${l}`),
      "",
    ].join("\n");
    const content: DiffContent = {
      file: fileOf(patch),
      oldText: `${lines.join("\n")}\n`,
      newText: `${changed.join("\n")}\n`,
      language: null,
    };
    const { state } = mount(content, "unified");
    act(() => state.controller!.scrollToLine(299));
    const before = state.controller!.topLine();
    expect(before).toBe(299);
    act(() => state.setMode!("split"));
    expect(state.controller!.mode).toBe("split");
    expect(state.controller!.topLine()).toBe(before);
    act(() => state.controller!.scrollToLine(201));
    act(() => state.setMode!("unified"));
    expect(state.controller!.mode).toBe("unified");
    expect(state.controller!.topLine()).toBe(201);
  });

  it("F7 의 이동: 다음 · 앞 묶음으로 커서를 옮기고, 끝에서는 옮기지 않는다", () => {
    const { host, state } = mount(CONTENT, "unified");
    const view = state.controller!.views()[0]!;
    act(() => view.focus());
    host.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    expect(activeDiff.getState().current).toBe(state.controller);
    const line = () => view.state.doc.lineAt(view.state.selection.main.head).number;
    act(() => view.dispatch({ selection: { anchor: 0 } }));
    let moved = false;
    act(() => {
      moved = state.controller!.go(1);
    });
    expect(moved).toBe(true);
    expect(line()).toBe(3);
    act(() => void state.controller!.go(1));
    expect(line()).toBe(5);
    expect(state.controller!.go(1)).toBe(false);
    act(() => void state.controller!.go(-1));
    expect(line()).toBe(3);
  });

  it("확장 지점: hunkControls 가 쪽마다 받은 확장을 그 뷰에 끼운다", () => {
    const seen: string[] = [];
    const marker = StateField.define<string>({ create: () => "slot", update: (v) => v });
    const { state } = mount(CONTENT, "split", {
      hunkControls: (c) => {
        seen.push(`${c.side}:${c.model.groups.length}`);
        return marker;
      },
    });
    expect(seen).toEqual(["a:3", "b:3"]);
    for (const v of state.controller!.views()) expect(v.state.field(marker, false)).toBe("slot");
  });

  it("모델의 묶음 수가 hunk 의 묶음과 같다", () => {
    expect(buildDiffModel(CONTENT.file).groups).toHaveLength(3);
  });
});
