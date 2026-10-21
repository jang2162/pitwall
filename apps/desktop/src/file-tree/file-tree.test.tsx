// @vitest-environment happy-dom
// 파일 트리 화면: 펼친 행이 많아도 보이는 것만 그리는 것, 클릭 → 미리보기 · 더블클릭 · Enter → 고정, IntelliJ 식 키보드 이동,
// speed search, 루트 밖 링크 표시, ignore · 벤더 흐림 표시, 우클릭 메뉴의 경로 복사, 「프로젝트 뷰에서 선택」 의 굴림 · 포커스.

import { LocalizationProvider } from "@fluent/react";
import type { FsEntry } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { bundledLocalization } from "../i18n/testing";
import { FileTree, type FileTreeHandlers } from "./file-tree";
import { fakeTreeLayout } from "./testing";
import { FileTreeModel } from "./tree-model";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const file = (name: string, extra: Partial<FsEntry> = {}): FsEntry => ({
  name,
  kind: "file",
  target: null,
  outside: false,
  size: 0,
  mtimeMs: null,
  ignored: false,
  ...extra,
});
const dir = (name: string, extra: Partial<FsEntry> = {}): FsEntry => ({
  ...file(name),
  kind: "dir",
  ...extra,
});

/** 폴더 경로 → 항목. 없는 폴더는 비었다. */
function modelOf(folders: Record<string, FsEntry[]>, dimmed?: (paths: string[]) => boolean[]) {
  return new FileTreeModel({
    list: async (path) => ({ path, entries: folders[path] ?? [] }),
    dimmed: dimmed && (async (paths) => dimmed(paths)),
  });
}

let root: Root | undefined;
let handlers: { [K in keyof Required<FileTreeHandlers>]: ReturnType<typeof vi.fn> };

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  handlers = {
    open: vi.fn(),
    copy: vi.fn(),
    reveal: vi.fn(),
    openWith: vi.fn(),
    findInFolder: vi.fn(),
  };
  fakeTreeLayout(400);
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 10)));

async function render(model: FileTreeModel, focusRequest = 0) {
  if (!root) {
    const element = document.createElement("div");
    element.style.height = "400px";
    document.body.append(element);
    root = createRoot(element);
  }
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <FileTree
          model={model}
          handlers={handlers as unknown as FileTreeHandlers}
          platform="mac"
          focusRequest={focusRequest}
        />
      </LocalizationProvider>,
    ),
  );
  await settle();
}

const tree = () => document.querySelector<HTMLElement>("[role=tree]")!;
const rowEls = () => [...document.querySelectorAll<HTMLElement>("[role=treeitem]")];
const row = (path: string) => document.querySelector<HTMLElement>(`[data-path="${path}"]`);
const selectedPath = () =>
  document.querySelector<HTMLElement>("[aria-selected=true]")?.getAttribute("data-path");

async function key(target: HTMLElement, k: string) {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  });
  await settle();
}

async function mouse(target: HTMLElement, type: "click" | "dblclick" | "contextmenu") {
  await act(async () => {
    target.dispatchEvent(
      new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        button: type === "contextmenu" ? 2 : 0,
      }),
    );
  });
  await settle();
}

it("draws only the rows near the view, even with 50 000 expanded rows", async () => {
  const entries = Array.from({ length: 50_000 }, (_, i) =>
    file(`f${String(i).padStart(5, "0")}.txt`),
  );
  const model = modelOf({ "/r": entries });
  await model.setRoot("/r");
  expect(model.rows()).toHaveLength(50_001);
  await render(model);
  expect(rowEls().length).toBeGreaterThan(0);
  expect(rowEls().length).toBeLessThan(100);
  // 스크롤 높이는 모든 행을 담는다.
  expect(tree().firstElementChild!.getAttribute("style")).toContain(`height: ${50_001 * 22}px`);
});

it("opens a preview tab on click and a pinned tab on double click or Enter", async () => {
  const model = modelOf({ "/r": [dir("src"), file("a.rs")], "/r/src": [file("lib.rs")] });
  await model.setRoot("/r");
  await render(model);

  await mouse(row("/r/a.rs")!, "click");
  expect(handlers.open).toHaveBeenLastCalledWith("/r/a.rs", true);
  expect(selectedPath()).toBe("/r/a.rs");

  await mouse(row("/r/a.rs")!, "dblclick");
  expect(handlers.open).toHaveBeenLastCalledWith("/r/a.rs", false);

  // 폴더 클릭은 고르기만, 더블클릭은 펼친다.
  handlers.open.mockClear();
  await mouse(row("/r/src")!, "click");
  expect(handlers.open).not.toHaveBeenCalled();
  expect(row("/r/src/lib.rs")).toBeNull();
  await mouse(row("/r/src")!, "dblclick");
  expect(row("/r/src/lib.rs")).not.toBeNull();

  await mouse(row("/r/src/lib.rs")!, "click");
  tree().focus();
  await key(tree(), "Enter");
  expect(handlers.open).toHaveBeenLastCalledWith("/r/src/lib.rs", false);
});

it("moves with the keyboard like IntelliJ: arrows, expand and collapse, parent", async () => {
  const model = modelOf({
    "/r": [dir("a"), file("z.txt")],
    "/r/a": [dir("b"), file("x.txt")],
    "/r/a/b": [file("deep.txt")],
  });
  await model.setRoot("/r");
  await render(model);
  tree().focus();

  await key(tree(), "ArrowDown");
  expect(selectedPath()).toBe("/r");
  await key(tree(), "ArrowDown");
  expect(selectedPath()).toBe("/r/a");
  await key(tree(), "ArrowRight"); // 펼친다
  expect(row("/r/a/b")).not.toBeNull();
  expect(selectedPath()).toBe("/r/a");
  await key(tree(), "ArrowRight"); // 첫 자식으로
  expect(selectedPath()).toBe("/r/a/b");
  await key(tree(), "ArrowRight");
  await key(tree(), "ArrowRight");
  expect(selectedPath()).toBe("/r/a/b/deep.txt");
  await key(tree(), "ArrowLeft"); // 부모로
  expect(selectedPath()).toBe("/r/a/b");
  await key(tree(), "ArrowLeft"); // 접는다
  expect(row("/r/a/b/deep.txt")).toBeNull();
  await key(tree(), "End");
  expect(selectedPath()).toBe("/r/z.txt");
  await key(tree(), "Home");
  expect(selectedPath()).toBe("/r");
  await key(tree(), "Enter"); // 폴더에서 Enter 는 접고 펼친다
  expect(rowEls()).toHaveLength(1);
});

it("speed-searches the visible rows by typing", async () => {
  const model = modelOf({
    "/r": [dir("src"), file("Cargo.toml"), file("main.rs"), file("readme.md")],
    "/r/src": [file("main.ts")],
  });
  await model.setRoot("/r");
  await model.expand("/r/src");
  await render(model);
  tree().focus();

  await key(tree(), "m");
  const input = document.querySelector<HTMLInputElement>("input[aria-label]")!;
  expect(input).not.toBeNull();
  expect(input.value).toBe("m");
  expect(document.activeElement).toBe(input);
  // 첫 일치(위에서부터)로 간다.
  expect(selectedPath()).toBe("/r/src/main.ts");

  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, "main.r");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  expect(selectedPath()).toBe("/r/main.rs");
  expect(row("/r/main.rs")!.querySelector("mark")!.textContent).toBe("main.r");

  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, "m");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  // 지금 행(main.rs)도 일치하므로 그대로다.
  expect(selectedPath()).toBe("/r/main.rs");
  await key(input, "ArrowDown"); // 다음 일치
  expect(selectedPath()).toBe("/r/readme.md");
  await key(input, "ArrowUp");
  expect(selectedPath()).toBe("/r/main.rs");

  await key(input, "Escape");
  expect(document.querySelector("input[aria-label]")).toBeNull();
  expect(document.activeElement).toBe(tree());
  expect(selectedPath()).toBe("/r/main.rs");
});

it("shows a link that points outside the root as such, and never expands or opens it", async () => {
  const model = modelOf({
    "/r": [
      dir("docs", { kind: "symlink", outside: true }),
      file("secret", { kind: "symlink", outside: true }),
    ],
  });
  await model.setRoot("/r");
  await render(model);
  const docs = row("/r/docs")!;
  expect(docs.hasAttribute("data-outside")).toBe(true);
  expect(docs.textContent).toContain("루트 밖을 가리킴");
  expect(docs.getAttribute("aria-expanded")).toBeNull();
  await mouse(docs, "dblclick");
  await mouse(row("/r/secret")!, "click");
  await mouse(row("/r/secret")!, "dblclick");
  expect(handlers.open).not.toHaveBeenCalled();
  expect(rowEls()).toHaveLength(3);
});

it("marks ignored and vendored rows", async () => {
  const model = modelOf(
    { "/r": [dir("target", { ignored: true }), file("a.rs"), file("jquery.min.js")] },
    (paths) => paths.map((p) => p.endsWith(".min.js")),
  );
  await model.setRoot("/r");
  await render(model);
  await vi.waitFor(() => expect(row("/r/jquery.min.js")!.hasAttribute("data-dim")).toBe(true));
  expect(row("/r/target")!.hasAttribute("data-ignored")).toBe(true);
  expect(row("/r/a.rs")!.hasAttribute("data-dim")).toBe(false);
  expect(row("/r/a.rs")!.hasAttribute("data-ignored")).toBe(false);
});

it("copies the absolute and the relative path and finds in the folder from the context menu", async () => {
  const model = modelOf({ "/r": [dir("src")], "/r/src": [file("lib.rs")] });
  await model.setRoot("/r");
  await model.expand("/r/src");
  await render(model);

  const pick = async (label: string) => {
    await mouse(row("/r/src/lib.rs")!, "contextmenu");
    const item = [...document.querySelectorAll<HTMLElement>("[role=menuitem]")].find(
      (e) => e.textContent === label,
    );
    if (!item) throw new Error(`메뉴 항목 ${label} 이 없다`);
    await mouse(item, "click");
  };
  await pick("경로 복사");
  expect(handlers.copy).toHaveBeenLastCalledWith("/r/src/lib.rs");
  await pick("상대 경로 복사");
  expect(handlers.copy).toHaveBeenLastCalledWith("src/lib.rs");
  await pick("Finder 에서 보기");
  expect(handlers.reveal).toHaveBeenLastCalledWith("/r/src/lib.rs");
  await pick("다음에서 열기…");
  expect(handlers.openWith).toHaveBeenLastCalledWith("/r/src/lib.rs");
  await pick("이 폴더에서 찾기");
  expect(handlers.findInFolder).toHaveBeenLastCalledWith(
    expect.objectContaining({ path: "/r/src/lib.rs", dir: false }),
  );
});

it("scrolls a revealed row into view and focuses the tree", async () => {
  const many = Array.from({ length: 3000 }, (_, i) => file(`f${String(i).padStart(4, "0")}.txt`));
  const model = modelOf({ "/r": [dir("a"), ...many], "/r/a": many });
  await model.setRoot("/r");
  await render(model);
  expect(row("/r/a/f2500.txt")).toBeNull();

  await model.reveal("/r/a/f2500.txt");
  await render(model, 1);
  expect(row("/r/a/f2500.txt")).not.toBeNull();
  expect(selectedPath()).toBe("/r/a/f2500.txt");
  expect(document.activeElement).toBe(tree());
  expect(tree().scrollTop).toBeGreaterThan(2000 * 22);
});
