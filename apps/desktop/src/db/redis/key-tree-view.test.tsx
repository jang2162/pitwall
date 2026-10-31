// @vitest-environment happy-dom
// 키 트리 화면(key-tree-view.tsx): 스캔 중에는 줄을 다시 펴는 것을 `RELAYOUT_MS` 에 한 번으로 묶되, 첫 묶음은 기다리지 않고 곧바로
// 그린다(첫 화면 500 ms — 14-m3-plan M3-18). 스캔이 끝나면 곧바로 다시 편다.

import { LocalizationProvider } from "@fluent/react";
import type { KvKeyInfo } from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import { bundledLocalization } from "../../i18n/testing";
import { act, endTest, it } from "../../test-support/ended-test";
import { KeyTree } from "./key-tree";
import { KeyTreeView, RELAYOUT_MS } from "./key-tree-view";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.mock("../../keymap/keymap-provider", () => ({ useKeymapPopup: () => {} }));

const noop = () => {};
const ACTIONS = { open: noop, editTtl: noop, remove: noop, rename: noop, copyName: noop };
const info = (key: string): KvKeyInfo => ({ key, kind: "string", ttlMs: null, size: 1 });

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "tree" ? 600 : 0;
    });
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

function render(tree: KeyTree, loading: boolean) {
  act(() =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <KeyTreeView
          tree={tree}
          loading={loading}
          selected={undefined}
          onSelect={noop}
          actions={ACTIONS}
          readOnly={false}
        />
      </LocalizationProvider>,
    ),
  );
}

const shown = () =>
  [...document.querySelectorAll<HTMLElement>("[data-key]")].map((e) => e.dataset.key);

describe("KeyTreeView while scanning", () => {
  it("draws the first chunk at once and later chunks at most every RELAYOUT_MS", async () => {
    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    const tree = new KeyTree(":", (run) => run());
    render(tree, true);
    act(() => void tree.add([info("a")]));
    expect(shown()).toEqual(["a"]);
    act(() => void tree.add([info("b")]));
    expect(shown()).toEqual(["a"]);
    await act(async () => new Promise((r) => setTimeout(r, RELAYOUT_MS + 50)));
    expect(shown()).toEqual(["a", "b"]);
    // 스캔이 끝나면 곧바로.
    act(() => void tree.add([info("c")]));
    render(tree, false);
    expect(shown()).toEqual(["a", "b", "c"]);
  });
});
