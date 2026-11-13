// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, showTooltip, type Tooltip } from "@codemirror/view";
import * as Dialog from "@radix-ui/react-dialog";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it } from "vitest";

import { contextLayers } from "../keymap/context";
import { targetView } from "./editor-ops";
import type { EditorWorkspace } from "./editor-workspace";
import {
  editorOfElement,
  editorTooltips,
  TOOLTIP_HOST_CLASS,
  TOOLTIP_LAYER_ID,
  TOOLTIP_LAYER_Z,
  tooltipHostOf,
} from "./tooltip-layer";

const here = dirname(fileURLToPath(import.meta.url));
const source = (path: string) => readFileSync(join(here, path), "utf8");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  document.body.replaceChildren();
});

/** 단추 하나가 든 툴팁(이름 바꾸기 칸 · 코드 액션 단추처럼 포커스를 받는 것). */
const buttonTip: Tooltip = {
  pos: 0,
  create: () => {
    const dom = document.createElement("div");
    dom.className = "pw-test-tip";
    const button = document.createElement("button");
    button.textContent = "act";
    dom.append(button);
    return { dom };
  },
};

/** `#root` 안의 문맥 요소에 에디터를 단다(앱의 에디터 패널처럼). */
function mountEditor(tips = new Compartment()) {
  const root = document.createElement("div");
  root.id = "root";
  document.body.append(root);
  const panel = document.createElement("div");
  panel.setAttribute("data-keymap-context", "editorFocus");
  panel.setAttribute("data-tool-window", "commit");
  root.append(panel);
  const view = new EditorView({
    parent: panel,
    state: EditorState.create({
      doc: "hello",
      extensions: [tips.of(editorTooltips()), showTooltip.of(buttonTip)],
    }),
  });
  return { view, root, tips };
}

it("draws editor tooltips in a layer right after the app root, under the z-50 in-window modals", () => {
  const { view, root } = mountEditor();
  const layer = document.getElementById(TOOLTIP_LAYER_ID)!;
  expect(root.nextElementSibling).toBe(layer);
  const tip = document.querySelector(".pw-test-tip")!;
  expect(view.dom.contains(tip)).toBe(false);
  expect(layer.contains(tip)).toBe(true);
  expect(tooltipHostOf(view)!.contains(tip)).toBe(true);
  // 패널 안 겹침(z-10 · z-20) 위, 창 안 모달(탭 전환기 · 탐색 팝업 z-50) 아래.
  expect(Number(layer.style.zIndex)).toBe(TOOLTIP_LAYER_Z);
  expect(TOOLTIP_LAYER_Z).toBeGreaterThan(20);
  expect(TOOLTIP_LAYER_Z).toBeLessThan(50);
  for (const modal of ["./tab-switcher.tsx", "./navigation-ui.tsx"])
    expect(source(modal)).toMatch(/fixed inset-0 z-50/);
  view.destroy();
});

it("hides the layer while a modal Radix dialog marks the rest aria-hidden, and shows it again after", async () => {
  const { view, root } = mountEditor();
  const layer = document.getElementById(TOOLTIP_LAYER_ID)!;
  // Radix 의 포털은 z-index 없이 body 끝에 붙는다 — 층(z 40)이 그 위로 오지 않게 숨기는 규칙.
  expect(source("../styles.css")).toMatch(
    /#pw-editor-tooltips\[aria-hidden="true"\]\s*\{\s*display: none;/,
  );
  const host = document.createElement("div");
  root.append(host);
  const app = createRoot(host);
  const render = (open: boolean) =>
    act(async () =>
      app.render(
        <Dialog.Root open={open}>
          <Dialog.Portal>
            <Dialog.Overlay className="fixed inset-0" />
            <Dialog.Content aria-describedby={undefined}>
              <Dialog.Title>t</Dialog.Title>
            </Dialog.Content>
          </Dialog.Portal>
        </Dialog.Root>,
      ),
    );
  await render(true);
  expect(layer.getAttribute("aria-hidden")).toBe("true");
  await render(false);
  expect(layer.getAttribute("aria-hidden")).toBeNull();
  await act(async () => app.unmount());
  view.destroy();
});

it("keeps the editor's keymap context and tool window for focus inside a tooltip", () => {
  const { view } = mountEditor();
  const button = document.querySelector<HTMLButtonElement>(".pw-test-tip button")!;
  // 에디터를 DOM 에 단 뒤의 갱신에서 문맥을 옮겨 적는다.
  view.dispatch({});
  button.focus();
  expect(document.activeElement).toBe(button);
  const [focus, toolWindow] = contextLayers(document.activeElement, {
    popupOpen: false,
    platform: "mac",
  });
  expect(focus).toMatchObject({ editorFocus: true });
  expect(toolWindow).toMatchObject({ toolWindowFocus: true, activeToolWindow: "commit" });
  expect(editorOfElement(button)).toBe(view);
  // 에디터 액션의 대상도 그 에디터다(활성 탭으로 물러나지 않는다 — 분할에서 다른 쪽이 활성일 수 있다).
  const none = { activeTab: () => undefined, view: () => undefined } as unknown as EditorWorkspace;
  expect(targetView(none)).toBe(view);
  expect(editorOfElement(view.contentDOM)).toBe(view);
  expect(editorOfElement(document.body)).toBeUndefined();
  view.destroy();
});

it("gives each configuration its own host and takes it away when the editor goes", () => {
  const { view, tips } = mountEditor();
  const layer = document.getElementById(TOOLTIP_LAYER_ID)!;
  const first = tooltipHostOf(view)!;
  view.dispatch({ effects: tips.reconfigure(editorTooltips()) });
  const second = tooltipHostOf(view)!;
  expect(second).not.toBe(first);
  expect(first.isConnected).toBe(false);
  expect(second.querySelector(".pw-test-tip")).not.toBeNull();
  expect(layer.querySelectorAll(`.${TOOLTIP_HOST_CLASS}`)).toHaveLength(1);
  view.destroy();
  expect(layer.querySelectorAll(`.${TOOLTIP_HOST_CLASS}`)).toHaveLength(0);
});
