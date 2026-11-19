// @vitest-environment happy-dom
// 브레이크포인트 거터 · 문서 위치(17-m6-plan M6-18 ① · 완료 기준 ① ②): 진짜 문서(`LiveText`)와 에디터 뷰에 거터를 걸고, Host 는 가짜다.
//
// - ① 줄을 넣은 편집 뒤 브레이크포인트가 따라 밀린다 — 문서의 위치(창의 `Breakpoints`)와 뷰의 거터 둘 다. 미저장 동안은 Host 에
//   보내지 않고, 저장(디스크와 같아짐)하면 옮긴 줄을 보낸다.
// - ② 조건 브레이크포인트의 편집: 거터 우클릭 → 편집 팝업에서 조건 · hit count 를 적고 [닫기] → 그 줄에 들고 Host 에 간다.

import { LocalizationProvider } from "@fluent/react";
import type { EventMessage } from "@pitwall/rpc-client";
import { EditorView } from "@codemirror/view";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";

import { LiveText } from "../editor/live-text";
import { bundledLocalization } from "../i18n/testing";
import { BreakpointEditor } from "./breakpoint-dialogs";
import {
  type CurrentFrame,
  type EditorDebug,
  breakpointGutter,
  gutterLines,
} from "./breakpoint-gutter";
import { type BreakpointDoc, Breakpoints } from "./breakpoints";

vi.mock("../keymap/keymap-provider", () => ({ useKeymapPopup: () => {} }));

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const PATH = "/main/src/main.rs";
const REL = "src/main.rs";

class FakeHost {
  calls: { method: string; params: Record<string, unknown> }[] = [];
  listener: ((e: EventMessage) => void) | undefined;
  saved: unknown[] = [];
  readonly client = {
    call: (async (method: string, params: Record<string, unknown>) => {
      this.calls.push({ method, params });
      if (method === "debug.breakpoints.list")
        return { files: [{ path: REL, breakpoints: this.saved }], problem: null };
      return { path: params.path, breakpoints: params.breakpoints };
    }) as never,
    subscribe: (async (_p: unknown, listener: (e: EventMessage) => void) => {
      this.listener = listener;
      return { subscriptionId: 1, unsubscribe: async () => {} };
    }) as never,
  };

  sets() {
    return this.calls.filter((c) => c.method === "debug.breakpoints.set").map((c) => c.params);
  }
}

function source(live: LiveText): BreakpointDoc {
  return {
    onChange(listener) {
      listener({ kind: "reset", text: live.text });
      return live.onChange((changes, before) =>
        listener({ kind: "edit", changes, before, after: live.text }),
      );
    },
    dirty: () => live.dirty,
    onDirty: (listener) => live.onDirtyChange(listener),
  };
}

const settle = () => act(async () => new Promise((r) => setTimeout(r, 10)));
const views: EditorView[] = [];
let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  for (const v of views.splice(0)) v.destroy();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

async function setup(text: string, saved: unknown[]) {
  const host = new FakeHost();
  host.saved = saved;
  const breakpoints = new Breakpoints();
  breakpoints.connect(host.client, "p1");
  await settle();
  const live = new LiveText({ text, lineEnding: "lf", bom: false });
  const edits: { path: string; line: number }[] = [];
  const debug: EditorDebug = {
    breakpoints,
    attach: (path) => breakpoints.attach(path, REL, source(live)),
    current: createStore(() => ({ frame: undefined as CurrentFrame | undefined })),
    editBreakpoint: (path, line) => edits.push({ path, line }),
    describe: () => "",
  };
  const made = live.createState("tab-1", breakpointGutter(debug, PATH, undefined));
  const view = new EditorView({
    parent: document.body,
    state: made.state,
    dispatchTransactions: (trs, v) => live.dispatch(trs, v),
  });
  views.push(view);
  live.attach("tab-1", view, () => undefined);
  await settle();
  return { host, breakpoints, live, view, edits };
}

describe("breakpoints in the gutter", () => {
  it("follow an inserted line and reach the Host only once the file is saved", async () => {
    const { host, breakpoints, live, view } = await setup("a\nb\nc\nd\n", [
      { line: 3, condition: "i > 3" },
    ]);
    expect(breakpoints.lines(PATH).map((b) => b.line)).toEqual([3]);
    expect(gutterLines(view.state)).toEqual([3]);

    // 위에 두 줄을 넣는다(사람이 친 편집).
    act(() => view.dispatch({ changes: { from: 0, insert: "x\ny\n" }, userEvent: "input.type" }));
    await settle();
    expect(breakpoints.lines(PATH)).toEqual([{ line: 5, condition: "i > 3" }]);
    expect(gutterLines(view.state)).toEqual([5]);
    // 미저장 편집의 줄은 문서에만.
    expect(host.sets()).toEqual([]);

    // 저장 — 문서가 디스크와 같아졌다.
    act(() => live.markClean());
    await settle();
    expect(host.sets()).toEqual([
      { projectId: "p1", path: REL, breakpoints: [{ line: 5, condition: "i > 3" }] },
    ]);
  });

  it("toggle on a click and are sent at once while the file has no unsaved edits", async () => {
    const { host, breakpoints, view } = await setup("a\nb\nc\n", []);
    act(() => breakpoints.toggle(PATH, 2));
    await settle();
    expect(gutterLines(view.state)).toEqual([2]);
    expect(host.sets().at(-1)).toEqual({
      projectId: "p1",
      path: REL,
      breakpoints: [{ line: 2, enabled: true }],
    });
    act(() => breakpoints.toggle(PATH, 2));
    await settle();
    expect(gutterLines(view.state)).toEqual([]);
    expect(host.sets().at(-1)).toEqual({ projectId: "p1", path: REL, breakpoints: [] });
  });

  it("take lines the adapter moved when the document has no unsaved edits", async () => {
    const { host, breakpoints, view } = await setup("a\nb\nc\nd\n", [{ line: 1 }]);
    act(() =>
      host.listener?.({
        subscriptionId: 1,
        projectId: "p1",
        topic: "debug.breakpoints",
        payload: {
          projectId: "p1",
          runId: "r1",
          path: REL,
          breakpoints: [{ line: 3 }],
          states: [{ line: 3, verified: false, message: "no code" }],
        } as never,
      }),
    );
    await settle();
    expect(breakpoints.lines(PATH).map((b) => b.line)).toEqual([3]);
    expect(gutterLines(view.state)).toEqual([3]);
    expect(breakpoints.store.getState().unverified).toEqual({ [`${REL}:3`]: "no code" });
    expect(view.dom.querySelector(".cm-breakpoint-unverified")).not.toBeNull();
  });
});

describe("a conditional breakpoint", () => {
  it("is edited from the gutter's context menu", async () => {
    const { host, breakpoints, view, edits } = await setup("a\nb\nc\n", [{ line: 2 }]);
    const marker = view.dom.querySelector<HTMLElement>(".cm-debug-gutter .cm-breakpoint")!;
    expect(marker).not.toBeNull();
    marker
      .closest(".cm-gutterElement")!
      .dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    // 줄은 CodeMirror 가 누른 높이로 찾는다 — happy-dom 에는 배치가 없어 줄을 가르지 않는다(팝업을 불렀는지만).
    expect(edits.map((e) => e.path)).toEqual([PATH]);

    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    let closed = false;
    await act(async () =>
      root!.render(
        <LocalizationProvider l10n={bundledLocalization("ko")}>
          <BreakpointEditor
            breakpoints={breakpoints}
            path={PATH}
            line={2}
            x={10}
            y={10}
            onClose={() => (closed = true)}
          />
        </LocalizationProvider>,
      ),
    );
    const dialog = document.querySelector<HTMLElement>("[data-testid=breakpoint-editor]")!;
    expect(dialog.textContent).toContain("브레이크포인트 · main.rs:2");
    const input = (name: string) => dialog.querySelector<HTMLInputElement>(`[data-field=${name}]`)!;
    const type = (el: HTMLInputElement, value: string) => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(el, value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    };
    await act(async () => {
      type(input("condition"), "i > 3");
      type(input("hitCondition"), "5");
    });
    await act(async () => [...dialog.querySelectorAll("button")].at(-1)!.click());
    await settle();
    expect(closed).toBe(true);
    expect(breakpoints.lines(PATH)).toEqual([
      { line: 2, enabled: true, condition: "i > 3", logMessage: null, hitCondition: "5" },
    ]);
    expect(host.sets().at(-1)?.breakpoints).toEqual([
      { line: 2, enabled: true, condition: "i > 3", logMessage: null, hitCondition: "5" },
    ]);
    expect(view.dom.querySelector(".cm-breakpoint-condition")).not.toBeNull();
  });
});
