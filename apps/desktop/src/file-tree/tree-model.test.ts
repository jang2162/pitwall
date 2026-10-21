// 파일 트리 모델: 펼칠 때 읽기, 폴더 먼저, 루트 밖 링크, ignore 숨김/흐림, `fs.changed` 반영(펼친 폴더만 다시 읽기), 다시 뿌리내리기,
// 「프로젝트 뷰에서 선택」(조상 펼치기).

import type { FsEntry, FsList } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { FileTreeModel, type FsChanged, isWithin, joinPath } from "./tree-model";

type Node = { [name: string]: Node | string };

/** 가짜 디스크. 값이 문자열이면 파일, 객체면 폴더. `links` 는 심링크(논리 경로 → 실제 경로 또는 `outside`). */
function fakeDisk(tree: Node, opts: { ignored?: string[]; links?: Record<string, string> } = {}) {
  const calls: string[] = [];
  const at = (path: string): Node | string | undefined => {
    const real = opts.links?.[path] ?? path;
    if (real === "outside") return undefined;
    let node: Node | string | undefined = { "": tree };
    for (const part of ["", ...real.split("/").filter(Boolean)]) {
      if (typeof node !== "object") return undefined;
      node = node[part];
    }
    return node;
  };
  const list = vi.fn(async (path: string): Promise<FsList> => {
    calls.push(path);
    const node = at(path);
    if (typeof node !== "object") throw new Error(`not a folder: ${path}`);
    const entries: FsEntry[] = Object.keys(node)
      .sort()
      .map((name) => {
        const child = joinPath(path, name);
        const link = opts.links?.[child];
        const target = link === "outside" ? undefined : at(child);
        return {
          name,
          kind: link ? "symlink" : typeof node[name] === "object" ? "dir" : "file",
          target:
            link && target !== undefined ? (typeof target === "object" ? "dir" : "file") : null,
          outside: link === "outside",
          size: 0,
          mtimeMs: null,
          ignored: (opts.ignored ?? []).some((i) => isWithin(child, i)),
        };
      });
    return { path: opts.links?.[path] ?? path, entries };
  });
  return { tree, calls, list };
}

function names(model: FileTreeModel): string[] {
  return model.rows().map((r) => `${"  ".repeat(r.depth)}${r.entry.name}${r.expanded ? "/" : ""}`);
}

const STALE_ENTRY: FsEntry = {
  name: "",
  kind: "file",
  target: null,
  outside: false,
  size: 0,
  mtimeMs: null,
  ignored: false,
};

const change = (paths: [string, FsChanged["kinds"][number]][], rescan = false): FsChanged => ({
  root: "/r",
  worktree: null,
  paths: paths.map((p) => p[0]),
  kinds: paths.map((p) => p[1]),
  rescan,
});

describe("FileTreeModel", () => {
  it("lists a folder when it is expanded, folders first", async () => {
    const disk = fakeDisk({ r: { "b.txt": "", a: { "x.rs": "" }, "c.md": "", z: {} } });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    expect(names(model)).toEqual(["r/", "  a", "  z", "  b.txt", "  c.md"]);
    expect(disk.calls).toEqual(["/r"]);

    await model.expand("/r/a");
    expect(names(model)).toEqual(["r/", "  a/", "    x.rs", "  z", "  b.txt", "  c.md"]);
    model.collapse("/r/a");
    expect(names(model)).toEqual(["r/", "  a", "  z", "  b.txt", "  c.md"]);
    // 다시 펼치면 기억한 목록을 쓴다(바뀌었다는 소식이 없었다).
    await model.expand("/r/a");
    expect(disk.calls).toEqual(["/r", "/r/a"]);
  });

  it("marks links that point outside the root and never expands them", async () => {
    const disk = fakeDisk(
      { r: { docs: {}, inner: { "f.txt": "" } } },
      { links: { "/r/docs": "outside", "/r/alias": "/r/inner" } },
    );
    disk.tree.r = { ...(disk.tree.r as Node), alias: {} };
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    const docs = model.rows().find((r) => r.entry.name === "docs")!.entry;
    expect(docs).toMatchObject({ outside: true, dir: false, symlink: true });
    await model.expand("/r/docs");
    expect(disk.calls).toEqual(["/r"]);
    expect(model.isExpanded("/r/docs")).toBe(false);

    // 루트 안을 가리키는 링크 폴더는 펼친다.
    const alias = model.rows().find((r) => r.entry.name === "alias")!.entry;
    expect(alias).toMatchObject({ outside: false, dir: true, symlink: true });
    await model.expand("/r/alias");
    expect(names(model)).toContain("    f.txt");
  });

  it("hides ignored entries in hide mode and dims them otherwise", async () => {
    const disk = fakeDisk(
      { r: { target: { debug: {} }, src: {}, "a.rs": "" } },
      { ignored: ["/r/target"] },
    );
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    expect(names(model)).toEqual(["r/", "  src", "  target", "  a.rs"]);
    expect(model.rows().find((r) => r.entry.name === "target")!.entry.ignored).toBe(true);

    model.setIgnoredMode("hide");
    expect(names(model)).toEqual(["r/", "  src", "  a.rs"]);
    model.setIgnoredMode("dim");
    expect(names(model)).toEqual(["r/", "  src", "  target", "  a.rs"]);
  });

  it("dims vendored, generated and documentation files from lang.detect", async () => {
    const disk = fakeDisk({ r: { "a.rs": "", "jquery.min.js": "", d: {} } });
    const dimmed = vi.fn(async (paths: string[]) => paths.map((p) => p.endsWith(".min.js")));
    const model = new FileTreeModel({ list: disk.list, dimmed });
    await model.setRoot("/r");
    await vi.waitFor(() =>
      expect(model.rows().find((r) => r.entry.name === "jquery.min.js")!.entry.dim).toBe(true),
    );
    expect(model.rows().find((r) => r.entry.name === "a.rs")!.entry.dim).toBe(false);
    // 폴더는 묻지 않는다.
    expect(dimmed).toHaveBeenCalledWith(["/r/a.rs", "/r/jquery.min.js"]);
  });

  it("re-reads an expanded folder when a file is created or removed in it", async () => {
    const disk = fakeDisk({ r: { a: { "x.rs": "" } } });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    await model.expand("/r/a");
    (disk.tree.r as Node).a = { "x.rs": "", "new.rs": "" };
    await model.applyChange(change([["/r/a/new.rs", "created"]]));
    expect(names(model)).toEqual(["r/", "  a/", "    new.rs", "    x.rs"]);

    (disk.tree.r as Node).a = { "new.rs": "" };
    await model.applyChange(change([["/r/a/x.rs", "removed"]]));
    expect(names(model)).toEqual(["r/", "  a/", "    new.rs"]);
  });

  it("only re-reads expanded folders; a collapsed folder is re-read when it is expanded again", async () => {
    const disk = fakeDisk({ r: { a: { "x.rs": "" }, b: {} } });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    await model.expand("/r/a");
    model.collapse("/r/a");
    disk.calls.length = 0;
    (disk.tree.r as Node).a = { "x.rs": "", "y.rs": "" };
    await model.applyChange(change([["/r/a/y.rs", "created"]]));
    expect(disk.calls).toEqual([]);
    await model.expand("/r/a");
    expect(disk.calls).toEqual(["/r/a"]);
    expect(names(model)).toContain("    y.rs");
    // 한 번도 읽지 않은 폴더 안의 변경은 아무것도 읽지 않는다.
    disk.calls.length = 0;
    await model.applyChange(change([["/r/b/q/z.rs", "created"]]));
    expect(disk.calls).toEqual([]);
  });

  it("re-reads everything below a folder that changed as a whole (modified folder)", async () => {
    const disk = fakeDisk({ r: { target: { debug: { "a.o": "" } } } }, { ignored: ["/r/target"] });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    await model.expand("/r/target");
    await model.expand("/r/target/debug");
    disk.calls.length = 0;
    ((disk.tree.r as Node).target as Node).debug = { "a.o": "", "b.o": "" };
    // ignore 된 폴더 안의 변경은 그 폴더 하나의 `modified` 로 접혀 온다(crates/pw-fs/src/watch/mod.rs FsChanged).
    await model.applyChange(change([["/r/target", "modified"]]));
    expect(disk.calls.sort()).toEqual(["/r/target", "/r/target/debug"]);
    expect(names(model)).toContain("      b.o");
  });

  it("re-reads every expanded folder on rescan and ignores paths outside the root", async () => {
    const disk = fakeDisk({ r: { a: {} } });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    await model.expand("/r/a");
    disk.calls.length = 0;
    await model.applyChange({ ...change([["/other/a/x", "created"]]) });
    expect(disk.calls).toEqual([]);
    await model.applyChange(change([], true));
    expect(disk.calls.sort()).toEqual(["/r", "/r/a"]);
  });

  it("maps changes under the real path of a linked folder to the folder shown in the tree", async () => {
    const disk = fakeDisk(
      { r: { node_modules: { ".pnpm": { pkg: { "i.js": "" } }, pkg: {} } } },
      { links: { "/r/node_modules/pkg": "/r/node_modules/.pnpm/pkg" } },
    );
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    await model.expand("/r/node_modules");
    await model.expand("/r/node_modules/pkg");
    disk.calls.length = 0;
    (((disk.tree.r as Node).node_modules as Node)[".pnpm"] as Node).pkg = {
      "i.js": "",
      "j.js": "",
    };
    await model.applyChange(change([["/r/node_modules/.pnpm/pkg/j.js", "created"]]));
    expect(disk.calls).toEqual(["/r/node_modules/pkg"]);
    expect(names(model)).toContain("      j.js");
  });

  it("forgets what was below a folder that disappeared", async () => {
    const disk = fakeDisk({ r: { a: { b: { "x.rs": "" } } } });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    await model.reveal("/r/a/b/x.rs");
    expect(model.selected()).toBe("/r/a/b/x.rs");
    disk.tree.r = {};
    await model.applyChange(change([["/r/a", "removed"]]));
    expect(names(model)).toEqual(["r/"]);
    expect(model.isExpanded("/r/a")).toBe(false);
    expect(model.selected()).toBe("/r");
  });

  it("re-roots when the root changes and drops answers that arrive late for an earlier root", async () => {
    // W1 → W2 → W1: 처음 W1 의 느린 응답(옛 목록)이 마지막에 와도 지금의 W1 목록을 덮지 않는다.
    let release: (() => void) | undefined;
    let calls = 0;
    const list = vi.fn(async (path: string): Promise<FsList> => {
      if (path === "/w1" && calls++ === 0) {
        await new Promise<void>((r) => (release = r));
        return { path, entries: [{ ...STALE_ENTRY, name: "stale" }] };
      }
      return { path, entries: [{ ...STALE_ENTRY, name: path === "/w1" ? "fresh" : "other" }] };
    });
    const model = new FileTreeModel({ list });
    const first = model.setRoot("/w1");
    await model.setRoot("/w2");
    expect(names(model)).toEqual(["w2/", "  other"]);
    await model.setRoot("/w1");
    expect(names(model)).toEqual(["w1/", "  fresh"]);
    release!();
    await first;
    expect(model.getRoot()).toBe("/w1");
    expect(names(model)).toEqual(["w1/", "  fresh"]);
  });

  it("reveals a path by expanding its ancestors, and refuses paths outside the root", async () => {
    const disk = fakeDisk({ r: { a: { b: { c: { "deep.rs": "" } } }, z: {} } });
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    expect(await model.reveal("/r/a/b/c/deep.rs")).toBe(true);
    expect(model.selected()).toBe("/r/a/b/c/deep.rs");
    expect(names(model)).toEqual(["r/", "  a/", "    b/", "      c/", "        deep.rs", "  z"]);
    expect(await model.reveal("/elsewhere/x")).toBe(false);
    expect(await model.reveal("/r/a/missing.rs")).toBe(false);
    expect(model.contains("/r/a")).toBe(true);
    expect(model.contains("/rr/a")).toBe(false);
  });

  it("keeps a revealed ignored file and its folders visible in hide mode", async () => {
    const disk = fakeDisk(
      { r: { target: { "out.txt": "" }, src: {} } },
      { ignored: ["/r/target"] },
    );
    const model = new FileTreeModel({ list: disk.list });
    await model.setRoot("/r");
    model.setIgnoredMode("hide");
    expect(names(model)).toEqual(["r/", "  src"]);
    await model.reveal("/r/target/out.txt");
    expect(names(model)).toEqual(["r/", "  src", "  target/", "    out.txt"]);
  });

  it("shows a read error on the folder instead of its children", async () => {
    const model = new FileTreeModel({
      list: async () => {
        throw new Error("permission denied");
      },
    });
    await model.setRoot("/r");
    expect(model.rows()[0]).toMatchObject({ loading: false, error: "permission denied" });
  });
});
