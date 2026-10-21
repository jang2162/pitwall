// @vitest-environment happy-dom
// 이미지 · Markdown 미리보기(12-m1-plan M1-22 완료 기준)를 프로젝트 창(App kind="project", `?project=p1`)과 가짜 Host 로 본다: png 와
// svg 가 `fs.read` 범위 읽기 → Blob URL 로 열리고 크기가 보인다, `<script>` 가 든 svg · md 를 열어도 스크립트가 돌지 않는다(전역
// 표식), README.md 미리보기에 표와 상대 경로 이미지가 보이고 루트 밖 이미지는 읽지 않는다(규칙 A), 링크를 누르면 앱 창은 그대로이고
// 외부 브라우저(가짜 opener)가 연다, 편집하면 미리보기가 디바운스 뒤에 바뀐다.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { PREVIEW_DEBOUNCE_MS } from "./markdown-viewer";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
  /** 스크립트가 돌았는지 보는 전역 표식. 시험의 svg · md 가 이것을 세우려 한다. */
  var __pwPwned: unknown;
}

const host = vi.hoisted(() => ({
  /** 정규화한 경로 → 내용. */
  files: new Map<string, string | Uint8Array>(),
  /** Host 의 접근 범위가 거절하는 경로(루트 밖을 가리키는 심링크). */
  denied: new Set<string>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
}));

const opener = vi.hoisted(() => ({
  openUrl: vi.fn(async (_url: string) => {}),
  revealItemInDir: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/plugin-opener", () => opener);

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32005, message, data: null } });
        if (m.method.startsWith("fs.")) host.calls.push({ method: m.method, params: m.params });
        const params = (m.params ?? {}) as {
          path?: string;
          offset?: number;
          length?: number;
          bytes?: boolean;
        };
        const path = params.path ?? "";
        const raw = () => {
          const content = host.files.get(path)!;
          return typeof content === "string" ? new TextEncoder().encode(content) : content;
        };
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: { files: [], notices: [] },
              error: null,
            } satisfies ConfigView);
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            } satisfies KeymapFile);
          case "host.info":
            return reply({
              hostId: "local",
              kind: "local",
              version: "0",
              os: "t",
              arch: "t",
            } satisfies HostInfo);
          case "task.list":
            return reply([]);
          case "project.get":
            return reply({
              id: "p1",
              name: "demo",
              host: "local",
              roots: ["/work/demo"],
              favorite: false,
              tags: [],
              configMode: "repository",
              slug: "demo",
              createdAt: 1,
              openedAt: null,
            } satisfies Project);
          case "project.trust":
            return reply({
              projectId: "p1",
              ownerId: "p1",
              state: "trusted",
            } satisfies ProjectTrust);
          case "project.storage":
            return reply({ kind: "ok", path: "/data/state.db" });
          case "project.claimTrustPrompt":
            return reply(false);
          case "fs.stat":
            if (!host.files.has(path)) return fail(`not found: ${path}`);
            return reply({
              path,
              kind: "file",
              target: null,
              size: raw().length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: "editable",
            } satisfies FsStat);
          case "fs.read": {
            if (host.denied.has(path)) return fail(`\`${path}\` is outside the project roots`);
            if (!host.files.has(path)) return fail(`not found: ${path}`);
            const all = raw();
            const offset = params.offset ?? 0;
            const part = all.slice(offset, offset + (params.length ?? all.length));
            const text = typeof host.files.get(path) === "string";
            return reply({
              path,
              size: all.length,
              mtimeMs: 1,
              offset,
              eof: offset + part.length >= all.length,
              version: offset === 0 && part.length === all.length ? `1:${path}` : null,
              content:
                text && !params.bytes
                  ? { kind: "text", text: new TextDecoder().decode(part) }
                  : { kind: "base64", data: btoa(String.fromCharCode(...part)) },
              judgment: {
                binary: !text,
                encoding: text ? "UTF-8" : null,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: "editable",
                editable: text,
                highlight: text,
              },
            } satisfies FsRead);
          }
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map(() => ({
                language: "Markdown",
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "events.subscribe":
            return reply({ subscriptionId: 1 });
          case "events.unsubscribe":
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: () => () => {},
} satisfies KeymapNative;

/** 1×1 투명 PNG. */
const PNG = Uint8Array.from(
  atob(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  ),
  (c) => c.charCodeAt(0),
);

const EVIL_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" onload="window.__pwPwned = 'onload'">
<script>window.__pwPwned = 'script'</script>
<rect width="10" height="10" fill="red"/>
</svg>`;

const EVIL_MD = `# Evil

<script>window.__pwPwned = 'md-script'</script>

<img src="x.png" onerror="window.__pwPwned = 'md-onerror'">

<svg><script>window.__pwPwned = 'md-svg'</script></svg>

[click](javascript:window.__pwPwned='md-link')
`;

const README_MD = `# Demo

See [the site](https://example.com/docs?a=1) and [contributing](CONTRIBUTING.md).

| Name | Value |
| ---- | ----- |
| one  | 1     |

- [x] done
- [ ] todo

![logo](docs/logo.png)
![outside](../outside.png)
![absolute](/etc/logo.png)
![remote](https://example.com/badge.png)
![link out](docs/escape.png)
`;

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  globalThis.__pwPwned = undefined;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map<string, string | Uint8Array>([
    ["/work/demo/pixel.png", PNG],
    ["/work/demo/evil.svg", EVIL_SVG],
    ["/work/demo/evil.md", EVIL_MD],
    ["/work/demo/README.md", README_MD],
    ["/work/demo/CONTRIBUTING.md", "# Contributing\n"],
    ["/work/demo/docs/logo.png", PNG],
    ["/work/outside.png", PNG],
    ["/etc/logo.png", PNG],
    ["/work/demo/docs/escape.png", PNG],
  ]);
  host.denied = new Set(["/work/demo/docs/escape.png"]);
  host.calls = [];
  opener.openUrl.mockClear();
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <StrictMode>
        <ThemeStoreProvider store={store}>
          <App kind="project" native={native} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

async function open(file: string) {
  await act(async () => projectOpenInbox.push({ file }));
  await settle();
  await settle();
}

const reads = () =>
  host.calls
    .filter((c) => c.method === "fs.read")
    .map((c) => c.params as { path: string; offset?: number; length?: number; bytes?: boolean });

const preview = () => document.querySelector<HTMLElement>("[data-testid=markdown-preview]");

/** 스크립트가 돌 수 있는 것: `<script>` · 인라인 svg · 끼워 넣는 요소 · `on*` 속성 · `javascript:` 주소. */
function scriptable(scope: ParentNode): string[] {
  const found: string[] = [];
  for (const el of scope.querySelectorAll("*")) {
    const tag = el.tagName.toLowerCase();
    if (["script", "svg", "iframe", "object", "embed"].includes(tag)) found.push(tag);
    for (const attr of el.attributes) {
      if (attr.name.startsWith("on")) found.push(`${tag}[${attr.name}]`);
      if (/^\s*javascript:/i.test(attr.value)) found.push(`${tag}[${attr.name}=javascript:]`);
    }
  }
  return found;
}

it("opens a png through ranged byte reads into a Blob URL and shows its size", async () => {
  await mount();
  await open("/work/demo/pixel.png");
  const img = document.querySelector<HTMLImageElement>("[data-testid=image-viewer] img")!;
  expect(img.getAttribute("src")).toMatch(/^blob:/);
  // StrictMode 가 효과를 두 번 돌려 읽기가 두 번일 수 있다. 모두 바이트 범위 읽기다(텍스트로 읽지 않는다).
  expect(reads().length).toBeGreaterThan(0);
  for (const read of reads())
    expect(read).toMatchObject({ path: "/work/demo/pixel.png", offset: 0, bytes: true });
  expect(reads().every((r) => typeof r.length === "number")).toBe(true);
  const info = () => document.querySelector("[data-testid=image-info]")!.textContent;
  expect(info()).toBe(`${PNG.length} B`);

  // 그려지면 해상도를 더한다.
  Object.defineProperty(img, "naturalWidth", { value: 640 });
  Object.defineProperty(img, "naturalHeight", { value: 480 });
  await act(async () => img.dispatchEvent(new Event("load")));
  expect(info()).toContain("640");
  expect(info()).toContain("480");
  expect(info()).toContain(`${PNG.length} B`);

  // 맞춤 ↔ 실제 크기.
  expect(img.className).toContain("max-w-full");
  const actual = document.querySelector<HTMLButtonElement>("[data-mode=actual]")!;
  await act(async () => actual.click());
  expect(actual.getAttribute("aria-pressed")).toBe("true");
  expect(img.className).not.toContain("max-w-full");
});

it("draws an svg only as an <img>, so a script inside it does not run", async () => {
  const created = vi.spyOn(URL, "createObjectURL");
  await mount();
  await open("/work/demo/evil.svg");
  // 형식이 없는 Blob 이면 웹뷰가 svg 를 그리지 않는다.
  expect(created.mock.calls.map(([blob]) => (blob as Blob).type)).toContain("image/svg+xml");
  const img = document.querySelector<HTMLImageElement>("[data-testid=image-viewer] img")!;
  expect(img.getAttribute("src")).toMatch(/^blob:/);
  expect(document.querySelector("[data-testid=image-info]")!.textContent).toBe(
    `${new TextEncoder().encode(EVIL_SVG).length} B`,
  );
  expect(scriptable(document.querySelector("[data-testid=image-viewer]")!)).toEqual([]);
  // 텍스트로 읽지 않았다 — 바이트로만 받았다.
  expect(reads().every((r) => r.bytes === true)).toBe(true);
  await settle();
  expect(globalThis.__pwPwned).toBeUndefined();
});

it("does not run scripts or handlers from a Markdown file", async () => {
  await mount();
  await open("/work/demo/evil.md");
  const el = preview()!;
  expect(el.querySelector("h1")?.textContent).toBe("Evil");
  expect(el.textContent).toContain("<script>window.__pwPwned = 'md-script'</script>");
  expect(scriptable(el)).toEqual([]);
  expect(el.querySelector("a")).toBeNull();
  await settle();
  expect(globalThis.__pwPwned).toBeUndefined();
});

it("previews README.md with a table, checkboxes and a relative image, and keeps out-of-root images unread", async () => {
  await mount();
  await open("/work/demo/README.md");
  await settle();
  const el = preview()!;
  expect(el.querySelector("table td")?.textContent?.trim()).toBe("one");
  expect([...el.querySelectorAll<HTMLInputElement>("input.pw-task")].map((c) => c.checked)).toEqual(
    [true, false],
  );

  const img = el.querySelector<HTMLImageElement>('img[alt="logo"]')!;
  expect(img.getAttribute("src")).toMatch(/^blob:/);
  expect(reads()).toContainEqual(
    expect.objectContaining({ path: "/work/demo/docs/logo.png", bytes: true }),
  );

  // 루트 밖 · 절대 경로 · 원격은 읽지도 그리지도 않는다. 루트 안이지만 Host 가 거절한 것(루트 밖 링크)도 그리지 않는다.
  const readPaths = reads().map((r) => r.path);
  for (const p of ["/work/outside.png", "/etc/logo.png"]) expect(readPaths).not.toContain(p);
  expect(readPaths.some((p) => p.includes("example.com"))).toBe(false);
  expect([...el.querySelectorAll(".pw-image-blocked")].map((s) => s.textContent)).toEqual([
    "outside",
    "absolute",
    "remote",
    "link out",
  ]);
  expect([...el.querySelectorAll("img")].map((i) => i.alt)).toEqual(["logo"]);
  expect(
    [...el.querySelectorAll("img")].every((i) => i.getAttribute("src")?.startsWith("blob:")),
  ).toBe(true);
});

it("opens a link in the external browser and leaves the app window where it is", async () => {
  await mount();
  await open("/work/demo/README.md");
  const before = window.location.href;
  const link = [...preview()!.querySelectorAll("a")].find((a) => a.textContent === "the site")!;
  const click = new MouseEvent("click", { bubbles: true, cancelable: true });
  await act(async () => link.dispatchEvent(click));
  expect(click.defaultPrevented).toBe(true);
  expect(opener.openUrl).toHaveBeenCalledExactlyOnceWith("https://example.com/docs?a=1");
  expect(window.location.href).toBe(before);

  // 가운데 클릭도 창을 옮기지 않고 아무것도 열지 않는다.
  const aux = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
  await act(async () => link.dispatchEvent(aux));
  expect(aux.defaultPrevented).toBe(true);
  expect(opener.openUrl).toHaveBeenCalledTimes(1);

  // 상대 경로 링크는 앱의 탭으로 연다(외부 브라우저가 아니다).
  const contributing = [...preview()!.querySelectorAll("a")].find(
    (a) => a.textContent === "contributing",
  )!;
  await act(async () => contributing.click());
  await settle();
  await settle();
  expect(opener.openUrl).toHaveBeenCalledTimes(1);
  expect(
    [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].map((e) => e.title),
  ).toContain("/work/demo/CONTRIBUTING.md");
  expect(window.location.href).toBe(before);
});

it("updates the preview after edits stop for the debounce time", async () => {
  await mount();
  await open("/work/demo/README.md");
  await act(async () => document.querySelector<HTMLButtonElement>("[data-mode=split]")!.click());
  await settle();
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;
  expect(preview()!.querySelector("h1")!.textContent).toBe("Demo");

  vi.useFakeTimers();
  await act(async () => view.dispatch({ changes: { from: 2, to: 6, insert: "Changed" } }));
  await act(async () => vi.advanceTimersByTime(PREVIEW_DEBOUNCE_MS - 10));
  // 그사이 또 고치면 다시 기다린다.
  await act(async () => view.dispatch({ changes: { from: 2, insert: "Re" } }));
  await act(async () => vi.advanceTimersByTime(PREVIEW_DEBOUNCE_MS - 10));
  expect(preview()!.querySelector("h1")!.textContent).toBe("Demo");
  await act(async () => vi.advanceTimersByTime(20));
  expect(preview()!.querySelector("h1")!.textContent).toBe("ReChanged");

  // 모드는 버튼으로 바뀐다: 에디터만 · 미리보기만.
  vi.useRealTimers();
  await act(async () => document.querySelector<HTMLButtonElement>("[data-mode=editor]")!.click());
  expect(preview()).toBeNull();
  expect(document.querySelector(".cm-editor")).not.toBeNull();
  await act(async () => document.querySelector<HTMLButtonElement>("[data-mode=preview]")!.click());
  expect(document.querySelector(".cm-editor")).toBeNull();
  expect(preview()!.querySelector("h1")!.textContent).toBe("ReChanged");
});
