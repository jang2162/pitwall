// @vitest-environment happy-dom
// 끌어 놓을 폴더(tree-drop.ts)와 이름 바꾸기의 처음 선택(file-dialogs.tsx `stemRange`).

import { afterEach, describe, expect, it, vi } from "vitest";

import { stemRange } from "./file-dialogs";
import { dropFolderAt, folderOf } from "./tree-drop";
import type { TreeEntry } from "./tree-model";

const entry = (path: string, over: Partial<TreeEntry> = {}): TreeEntry => ({
  path,
  name: path.slice(path.lastIndexOf("/") + 1),
  dir: false,
  symlink: false,
  outside: false,
  ignored: false,
  dim: false,
  ...over,
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("folderOf", () => {
  it("is the folder itself, or the parent of a file and of a link pointing outside", () => {
    expect(folderOf(entry("/r/src", { dir: true }), "/r")).toBe("/r/src");
    expect(folderOf(entry("/r/src/a.rs"), "/r")).toBe("/r/src");
    expect(folderOf(entry("/r/out", { dir: false, outside: true, symlink: true }), "/r")).toBe(
      "/r",
    );
  });
});

describe("dropFolderAt", () => {
  it("finds the row under the point, the root on empty space, and nothing outside the tree", () => {
    const tree = document.createElement("div");
    tree.setAttribute("role", "tree");
    const item = document.createElement("div");
    item.setAttribute("role", "treeitem");
    item.setAttribute("data-path", "/r/src/a.rs");
    tree.append(item);
    document.body.append(tree);
    vi.spyOn(tree, "getBoundingClientRect").mockReturnValue(new DOMRect(10, 10, 100, 100));
    const entries = new Map([["/r/src/a.rs", entry("/r/src/a.rs")]]);
    const at = (x: number, y: number, hit: Element | null) => {
      document.elementFromPoint = () => hit;
      return dropFolderAt(tree, { x, y }, "/r", (p) => entries.get(p));
    };
    expect(at(20, 20, item)).toBe("/r/src");
    expect(at(20, 20, tree)).toBe("/r");
    expect(at(200, 20, item)).toBeUndefined();
  });
});

describe("stemRange", () => {
  it("selects the name before the extension, all of a dot file", () => {
    expect(stemRange("lib.rs")).toEqual([0, 3]);
    expect(stemRange("archive.tar.gz")).toEqual([0, 11]);
    expect(stemRange(".gitignore")).toEqual([0, 10]);
    expect(stemRange("Makefile")).toEqual([0, 8]);
  });
});
