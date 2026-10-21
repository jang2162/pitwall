// @vitest-environment happy-dom
import { Terminal } from "@xterm/xterm";
import { describe, expect, it } from "vitest";

import { cellOffsets, fileLinkProvider, findFileLinks, resolvePath } from "./terminal-links";

describe("findFileLinks", () => {
  it("finds paths with an optional line and column", () => {
    expect(findFileLinks("error at src/main.rs:12 here")).toEqual([
      { start: 9, end: 23, path: "src/main.rs", line: 12 },
    ]);
    expect(findFileLinks("  --> crates/pw-core/src/lib.rs:40:5")).toEqual([
      { start: 6, end: 36, path: "crates/pw-core/src/lib.rs", line: 40, column: 5 },
    ]);
    expect(findFileLinks("/abs/a.ts and ./b/c.tsx:3")).toEqual([
      { start: 0, end: 9, path: "/abs/a.ts" },
      { start: 14, end: 25, path: "./b/c.tsx", line: 3 },
    ]);
  });

  it("drops a sentence's full stop and ignores words, numbers and URLs", () => {
    expect(findFileLinks("see README.md.")).toEqual([{ start: 4, end: 13, path: "README.md" }]);
    expect(findFileLinks("hello world 42 ... 3.14")).toEqual([]);
    expect(findFileLinks("open https://example.com/a/b.html now")).toEqual([]);
  });

  it("keeps Windows drive paths", () => {
    expect(findFileLinks(String.raw`C:\work\a.rs:7`)).toEqual([
      { start: 0, end: 14, path: String.raw`C:\work\a.rs`, line: 7 },
    ]);
  });
});

describe("resolvePath", () => {
  it("joins relative paths to the shell's directory and tidies dots", () => {
    expect(resolvePath("/p/sub", "../src/main.rs")).toBe("/p/src/main.rs");
    expect(resolvePath("/p", "./a/./b.ts")).toBe("/p/a/b.ts");
    expect(resolvePath("/p", "/abs/x.ts")).toBe("/abs/x.ts");
    expect(resolvePath(String.raw`C:\p`, String.raw`src\a.rs`)).toBe(String.raw`C:\p\src\a.rs`);
    expect(resolvePath("/p", "~/x.ts")).toBeUndefined();
  });
});

function terminalWith(line: string) {
  const el = document.createElement("div");
  document.body.append(el);
  const term = new Terminal({ cols: 60, rows: 5 });
  term.open(el);
  return new Promise<Terminal>((resolve) => term.write(line, () => resolve(term)));
}

describe("fileLinkProvider", () => {
  it("links only existing files, counts wide characters as two cells, and opens on ⌘+click", async () => {
    const term = await terminalWith("오류: src/main.rs:12 missing.rs:1");
    const offsets = cellOffsets(term.buffer.active.getLine(0)!);
    // 「오」 「류」 는 두 칸씩이다.
    expect(offsets.slice(0, 4)).toEqual([0, 2, 4, 5]);
    const opened: unknown[] = [];
    const provider = fileLinkProvider(term, {
      cwd: () => "/p",
      isFile: async (path) => path === "/p/src/main.rs",
      opens: (e) => e.metaKey,
      open: (target) => opened.push(target),
    });
    const links = await new Promise<Parameters<Parameters<typeof provider.provideLinks>[1]>[0]>(
      (resolve) => provider.provideLinks(1, resolve),
    );
    expect(links).toHaveLength(1);
    const link = links![0]!;
    expect(link.text).toBe("src/main.rs:12");
    // 「오류: 」 = 6칸, 1부터 세어 7번째 칸에서 시작해 14글자.
    expect(link.range).toEqual({ start: { x: 7, y: 1 }, end: { x: 20, y: 1 } });
    link.activate(new MouseEvent("click"), link.text);
    expect(opened).toEqual([]);
    link.activate(new MouseEvent("click", { metaKey: true }), link.text);
    expect(opened).toEqual([{ path: "/p/src/main.rs", line: 12, column: undefined }]);
  });

  it("offers nothing before the shell reports its directory", async () => {
    const term = await terminalWith("src/main.rs:12");
    const provider = fileLinkProvider(term, {
      cwd: () => undefined,
      isFile: async () => true,
      opens: () => true,
      open() {},
    });
    const links = await new Promise((resolve) => provider.provideLinks(1, resolve));
    expect(links).toBeUndefined();
  });
});
