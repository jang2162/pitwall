// Markdown 변환 규칙(markdown.ts, 12-m1-plan M1-22): raw HTML 끔 · GFM 표 · 체크박스, 이미지 주소를 웹뷰가 직접 읽지 않음, 링크 ·
// 이미지 경로의 규칙 A(루트 밖 · 절대 경로 · 위험한 스킴 막기).

import { describe, expect, it } from "vitest";

import { classifyLink, IMAGE_SOURCE_ATTR, renderMarkdown, resolveLocal } from "./markdown";
import { PreviewTooLarge, readAllBytes } from "./preview-host";

const ROOT = "/work/demo";
const README = "/work/demo/README.md";
const NESTED = "/work/demo/docs/guide/intro.md";

describe("renderMarkdown", () => {
  it("renders GFM tables and task-list checkboxes", () => {
    const html = renderMarkdown(
      "| a | b |\n| - | - |\n| 1 | 2 |\n\n- [ ] todo\n- [x] done\n- plain\n",
    );
    expect(html).toContain("<table>");
    expect(html).toContain("<td>1</td>");
    expect(html).toContain('<input type="checkbox" class="pw-task" disabled> todo');
    expect(html).toContain('<input type="checkbox" class="pw-task" disabled checked> done');
    expect(html).toContain("<li>plain</li>");
    expect(html).not.toContain("[ ]");
  });

  it("keeps raw HTML as text", () => {
    const html = renderMarkdown(
      '<script>window.x = 1</script>\n\nhi <img src="a.png" onerror="alert(1)"> <svg onload="alert(1)"></svg>\n',
    );
    expect(html).not.toMatch(/<script|<img|<svg/i);
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("&lt;img src=&quot;a.png&quot; onerror=");
  });

  it("does not make links out of dangerous schemes", () => {
    const html = renderMarkdown(
      "[a](javascript:alert(1)) [b](vbscript:x) [c](file:///etc/passwd) <javascript:alert(1)>\n",
    );
    expect(html).not.toContain("<a");
  });

  it("moves image addresses out of src so the webview never fetches them", () => {
    const html = renderMarkdown("![logo](docs/logo.png) ![remote](https://example.com/x.png)\n");
    expect(html).not.toMatch(/\ssrc=/);
    expect(html).toContain(`${IMAGE_SOURCE_ATTR}="docs/logo.png"`);
    expect(html).toContain(`${IMAGE_SOURCE_ATTR}="https://example.com/x.png"`);
  });
});

describe("resolveLocal", () => {
  it("resolves relative paths from the document's folder", () => {
    expect(resolveLocal("docs/logo.png", README, ROOT)).toBe("/work/demo/docs/logo.png");
    expect(resolveLocal("./img/a.png", NESTED, ROOT)).toBe("/work/demo/docs/guide/img/a.png");
    expect(resolveLocal("../../a.png", NESTED, ROOT)).toBe("/work/demo/a.png");
    expect(resolveLocal("my%20logo.png?raw=1#x", README, ROOT)).toBe("/work/demo/my logo.png");
  });

  it("refuses paths outside the root, absolute paths and URLs", () => {
    expect(resolveLocal("../outside.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("../../../a.png", NESTED, ROOT)).toBeUndefined();
    expect(resolveLocal("docs/../../demo2/a.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("/work/demo/a.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("%2Fetc%2Fpasswd", README, ROOT)).toBeUndefined();
    expect(resolveLocal("\\\\server\\share\\a.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("C:\\a.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("https://example.com/a.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("file:///work/demo/a.png", README, ROOT)).toBeUndefined();
    expect(resolveLocal("data:image/png;base64,AAAA", README, ROOT)).toBeUndefined();
    expect(resolveLocal("", README, ROOT)).toBeUndefined();
  });

  it("does not take a sibling folder with the same prefix as inside", () => {
    expect(resolveLocal("../demo-other/a.png", README, ROOT)).toBeUndefined();
  });

  it("works with Windows paths", () => {
    expect(resolveLocal("img\\a.png", "C:\\work\\demo\\README.md", "C:\\work\\demo")).toBe(
      "C:\\work\\demo\\img\\a.png",
    );
    expect(resolveLocal("..\\a.png", "C:\\work\\demo\\README.md", "C:\\work\\demo")).toBe(
      undefined,
    );
  });
});

describe("classifyLink", () => {
  it("sends http(s) and mailto to the external browser", () => {
    expect(classifyLink("https://example.com/a?b=1", README, ROOT)).toEqual({
      kind: "external",
      url: "https://example.com/a?b=1",
    });
    expect(classifyLink("HTTP://example.com", README, ROOT)).toEqual({
      kind: "external",
      url: "http://example.com/",
    });
    expect(classifyLink("mailto:a@example.com", README, ROOT).kind).toBe("external");
  });

  it("never opens other schemes", () => {
    for (const href of [
      "javascript:alert(1)",
      " JavaScript:alert(1)",
      "vbscript:x",
      "file:///etc/passwd",
      "data:text/html,<script>alert(1)</script>",
      "tauri://localhost/",
      "ftp://example.com/",
      "//example.com/a",
    ])
      expect(classifyLink(href, README, ROOT), href).toEqual({ kind: "blocked" });
  });

  it("opens relative files inside the root as tabs and blocks the rest", () => {
    expect(classifyLink("CONTRIBUTING.md#setup", README, ROOT)).toEqual({
      kind: "file",
      path: "/work/demo/CONTRIBUTING.md",
    });
    expect(classifyLink("../secret.md", README, ROOT)).toEqual({ kind: "blocked" });
    expect(classifyLink("/etc/hosts", README, ROOT)).toEqual({ kind: "blocked" });
    expect(classifyLink("CONTRIBUTING.md", README, undefined)).toEqual({ kind: "blocked" });
  });

  it("keeps in-page anchors in the preview", () => {
    expect(classifyLink("#getting-started", README, ROOT)).toEqual({
      kind: "anchor",
      id: "getting-started",
    });
  });
});

describe("readAllBytes", () => {
  it("joins range reads until the end of the file", async () => {
    const file = Uint8Array.from({ length: 10 }, (_, i) => i);
    const calls: { offset?: number | null; length?: number | null; bytes?: boolean }[] = [];
    const blob = await readAllBytes(async (params) => {
      calls.push({ offset: params.offset, length: params.length, bytes: params.bytes });
      const start = params.offset ?? 0;
      // Host 가 요청보다 적게 줄 수 있다 — 4바이트씩만 준다.
      const part = file.slice(start, start + 4);
      return {
        path: params.path,
        size: file.length,
        mtimeMs: 1,
        offset: start,
        eof: start + part.length >= file.length,
        version: null,
        content: { kind: "base64", data: btoa(String.fromCharCode(...part)) },
        judgment: {
          binary: true,
          encoding: null,
          bom: null,
          lineEnding: "lf",
          longLines: false,
          sizeClass: "editable",
          editable: false,
          highlight: false,
        },
      };
    }, "/work/demo/a.png");
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(file);
    expect(calls.map((c) => c.offset)).toEqual([0, 4, 8]);
    expect(calls.every((c) => c.bytes === true && typeof c.length === "number")).toBe(true);
  });

  const fake =
    (size: number, calls: { offset?: number | null }[]) =>
    async (params: { path: string; offset?: number | null }) => {
      calls.push({ offset: params.offset });
      const start = params.offset ?? 0;
      return {
        path: params.path,
        size,
        mtimeMs: 1,
        offset: start,
        eof: false,
        version: null,
        content: { kind: "base64" as const, data: btoa("abcd") },
        judgment: {
          binary: true,
          encoding: null,
          bom: null,
          lineEnding: "lf" as const,
          longLines: false,
          sizeClass: "editable" as const,
          editable: false,
          highlight: false,
        },
      };
    };

  it("refuses a file whose size is over the limit after the first chunk", async () => {
    const calls: { offset?: number | null }[] = [];
    await expect(
      readAllBytes(fake(10 * 1024 ** 3, calls), "/work/demo/big.png"),
    ).rejects.toMatchObject({ size: 10 * 1024 ** 3, limit: 50 * 1024 * 1024 });
    expect(calls).toHaveLength(1);
  });

  it("stops when the bytes read pass the limit although the reported size did not", async () => {
    const calls: { offset?: number | null }[] = [];
    // 크기는 작다고 하면서 끝(eof)을 영영 안 주는 Host.
    const error = await readAllBytes(fake(4, calls), "/work/demo/liar.png", 10).catch(
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(PreviewTooLarge);
    expect(calls).toHaveLength(3);
  });
});
