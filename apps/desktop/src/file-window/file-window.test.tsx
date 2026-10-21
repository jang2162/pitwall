// @vitest-environment happy-dom
// 단일 파일 창(App kind="file", `?window=file&path=…`, 12-m1-plan M1-24)을 가짜 Host 에 붙여 본다: 그 파일이 탭으로 열리고 편집 ·
// 저장 · 외부 변경 · 미저장 닫기 확인이 프로젝트 창의 에디터와 같이 돌고, 요청에는 프로젝트가 실리지 않고, 파일 트리 · 검색 ·
// 터미널 액션이 없고, 「프로젝트로 열기」 가 네이티브를 부른다. 50 MiB 를 넘는 파일은 이 연결로 hex · 앞부분을 읽고, 외부 도구
// 버튼은 없다(이 창에는 외부 도구가 없다). 접근 범위 자체(다른 경로 거절)는 Host 가 지킨다 —
// crates/pw-fs/tests/file_scope.rs · src-tauri/src/ipc.rs 시험.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  FsBytes,
  FsRead,
  FsStat,
  FsWriteParams,
  FsWritten,
  HostInfo,
  KeymapFile,
  LangDetectResult,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { cursorOf } from "../editor/editor-workspace";
import { HEAD_PREVIEW_BYTES, HEX_PAGE_BYTES } from "../editor/limits";
import type { KeymapNative } from "../keymap/native";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import type { FileWindowNative } from "./file-window";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const FILE = "/tmp/x/a.txt";

const host = vi.hoisted(() => ({
  text: "",
  mtime: 1,
  /** 있으면 그 파일은 이 크기의 「열지 않음」(50 MiB 초과)이다 — 바이트는 `text` 뒤를 0 으로 채운 것이다. */
  hugeSize: undefined as number | undefined,
  calls: [] as { method: string; params: Record<string, unknown> }[],
  subscribers: [] as {
    send(m: object): void;
    id: number;
    projectId?: string;
    topics: string[];
  }[],
}));

const versionOf = () => `${host.mtime}:${host.text}`;

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        host.calls.push({ method: m.method, params: m.params ?? {} });
        const path = (m.params as { path?: string } | undefined)?.path;
        // 이 연결의 범위 밖은 Host 가 거절한다(pw-fs `Roots::file`).
        const outside = () =>
          send({ id: m.id, error: { code: -32602, message: `${path} is outside`, data: null } });
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
          case "fs.stat":
            if (path !== FILE) return outside();
            return reply({
              path: FILE,
              kind: "file",
              target: null,
              size: host.hugeSize ?? host.text.length,
              mtimeMs: host.mtime,
              readonly: false,
              sizeClass: host.hugeSize === undefined ? "editable" : "tooLarge",
            } satisfies FsStat);
          case "fs.readBytes": {
            if (path !== FILE) return outside();
            const p = m.params as { offset: number; length: number };
            const size = host.hugeSize ?? host.text.length;
            const page = new Uint8Array(Math.max(0, Math.min(p.length, size - p.offset)));
            page.set(new TextEncoder().encode(host.text).slice(p.offset, p.offset + page.length));
            return reply({
              path: FILE,
              size,
              mtimeMs: host.mtime,
              offset: p.offset,
              data: Buffer.from(page).toString("base64"),
              eof: p.offset + page.length >= size,
            } satisfies FsBytes);
          }
          case "fs.read":
            if (path !== FILE) return outside();
            if (host.hugeSize !== undefined) {
              const p = m.params as { offset?: number; length?: number };
              if (p.offset === undefined || p.length === undefined) return outside();
              return reply({
                path: FILE,
                size: host.hugeSize,
                mtimeMs: host.mtime,
                offset: p.offset,
                eof: false,
                version: null,
                content: { kind: "text", text: host.text },
                judgment: {
                  binary: false,
                  encoding: "UTF-8",
                  lossy: false,
                  bom: null,
                  lineEnding: "lf",
                  longLines: false,
                  sizeClass: "tooLarge",
                  editable: false,
                  highlight: false,
                },
              } satisfies FsRead);
            }
            return reply({
              path: FILE,
              size: host.text.length,
              mtimeMs: host.mtime,
              offset: 0,
              eof: true,
              version: versionOf(),
              content: { kind: "text", text: host.text },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: "editable",
                editable: true,
                highlight: true,
              },
            } satisfies FsRead);
          case "fs.write": {
            const p = m.params as FsWriteParams;
            if (p.path !== FILE) return outside();
            if (p.expectedVersion !== versionOf())
              return send({ id: m.id, error: { code: -32004, message: "conflict", data: null } });
            if (p.content.kind !== "text") return outside();
            host.text = p.content.text;
            host.mtime += 1;
            return reply({
              path: FILE,
              size: host.text.length,
              mtimeMs: host.mtime,
              version: versionOf(),
            } satisfies FsWritten);
          }
          case "fs.watch":
            return reply({ streamId: next++ });
          case "lang.detect":
            return reply({
              files: [
                {
                  language: null,
                  source: "extension",
                  vendored: false,
                  generated: false,
                  documentation: false,
                },
              ],
            } satisfies LangDetectResult);
          case "lang.list":
            return reply({ languages: [] });
          case "events.subscribe": {
            const id = next++;
            const params = m.params as { projectId?: string; topics: string[] };
            host.subscribers.push({ send, id, ...params });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
          case "stream.credit":
          case "stream.cancel":
            return m.id === undefined ? undefined : reply(null);
          default:
            return send({ id: m.id, error: { code: -32001, message: "permission denied" } });
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

let closeGuard: (() => Promise<boolean>) | undefined;
const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: vi.fn<KeymapNative["closeWindow"]>(async () => {}),
  guardClose: (shouldClose: () => Promise<boolean>) => {
    closeGuard = shouldClose;
    return () => {
      if (closeGuard === shouldClose) closeGuard = undefined;
    };
  },
} satisfies KeymapNative;
const fileNative = {
  openAsProject: vi.fn<FileWindowNative["openAsProject"]>(async () => {}),
};

let root: Root | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", `/?window=file&path=${encodeURIComponent(FILE)}`);
  host.text = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
  host.mtime = 1;
  host.hugeSize = undefined;
  host.calls = [];
  host.subscribers = [];
  native.closeWindow.mockClear();
  fileNative.openAsProject.mockClear();
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  dock = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "960px";
  element.style.height = "720px";
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
          <App kind="file" native={native} fileNative={fileNative} onDockApi={(a) => (dock = a)} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
  await settle();
}

async function press(code: string, mods: KeyboardEventInit = {}, key = code) {
  for (const type of ["keydown", "keyup"])
    await act(async () => {
      (document.activeElement ?? document.body).dispatchEvent(
        new KeyboardEvent(type, { bubbles: true, cancelable: true, code, key, ...mods }),
      );
    });
  await settle();
}

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((e) => EditorView.findFromDOM(e)!);
const tabs = () =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].map((e) => e.title);
const calls = (method: string) =>
  host.calls.filter((c) => c.method === method).map((c) => c.params);
const dialog = () => document.querySelector<HTMLElement>("[data-testid=unsaved-dialog]");

async function typeInto(view: EditorView, at: number, insert: string) {
  (document.activeElement as HTMLElement | null)?.blur();
  await act(async () =>
    view.dispatch({
      changes: { from: at, insert },
      selection: { anchor: at + insert.length },
      userEvent: "input.type",
    }),
  );
  await settle();
}

async function clickButton(scope: ParentNode, text: string) {
  const button = [...scope.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent === text,
  );
  if (!button) throw new Error(`단추 ${text} 가 없다`);
  await act(async () => button.click());
  await settle();
}

/** 액션 찾기(⌘⇧A)에 `label` 로 찾은 항목들. */
async function findActions(label: string): Promise<string[]> {
  await press("KeyA", { metaKey: true, shiftKey: true }, "a");
  const input = document.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, label);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  const found = [...document.querySelectorAll<HTMLElement>("[cmdk-item]")].map(
    (e) => e.textContent ?? "",
  );
  await press("Escape", {}, "Escape");
  return found;
}

it("opens its file as a tab, titled by the file name, with no project in any request", async () => {
  await mount();
  expect(tabs()).toEqual([FILE]);
  const [view] = views();
  expect(view!.state.doc.line(3).text).toBe("line 3");
  expect(document.querySelector(".titlebar")?.textContent).toBe("a.txt");
  for (const method of ["fs.stat", "fs.read"])
    for (const params of calls(method)) expect(params).toMatchObject({ projectId: "", path: FILE });
  expect(calls("lang.detect")[0]).not.toHaveProperty("projectId");
  // 감시는 그 파일 하나, 변경은 이 연결의 범위로 듣는다.
  expect(calls("fs.watch")).toEqual([{ projectId: "", worktree: null }]);
  expect(host.subscribers.map((s) => s.projectId)).toContain(`file:${FILE}`);
  // 프로젝트를 대는 요청은 하나도 없다.
  expect(host.calls.filter((c) => c.method.startsWith("project."))).toEqual([]);
});

it("edits and saves with the version it read, and takes an external change", async () => {
  await mount();
  const [view] = views();
  const read = versionOf();
  await typeInto(view!, 0, "// ");
  await press("KeyS", { metaKey: true }, "s");
  expect(calls("fs.write")).toEqual([
    {
      projectId: "",
      worktree: null,
      path: FILE,
      content: { kind: "text", text: `// ${view!.state.doc.toString().slice(3)}` },
      expectedVersion: read,
      bom: false,
    },
  ]);
  expect(host.text.startsWith("// line 1")).toBe(true);

  host.text = "changed by an agent";
  host.mtime += 1;
  await act(async () => {
    for (const s of host.subscribers.filter((s) => s.projectId === `file:${FILE}`))
      s.send({
        method: "events.event",
        params: {
          subscriptionId: s.id,
          projectId: `file:${FILE}`,
          topic: "fs.changed",
          payload: {
            root: FILE,
            worktree: null,
            paths: [FILE],
            kinds: ["modified"],
            rescan: false,
          },
        },
      });
  });
  await vi.waitFor(() => expect(views()[0]!.state.doc.toString()).toBe("changed by an agent"));
});

it("moves the cursor when the same file is opened again with a line", async () => {
  await mount();
  await act(async () => projectOpenInbox.push({ file: FILE, line: 20, col: 3 }));
  await settle();
  expect(tabs()).toEqual([FILE]);
  expect(cursorOf(views()[0]!)).toEqual({ line: 20, column: 3 });
});

it("has no file tree, search or terminal: their keys do nothing and their actions are absent", async () => {
  await mount();
  const panels = () => dock!.panels.map((p) => p.id).sort();
  const before = panels();
  // ⌘1 프로젝트 툴 윈도우, ⌥F12 터미널, ⌘⇧F 전체에서 찾기.
  await press("Digit1", { metaKey: true }, "1");
  await press("F12", { altKey: true }, "F12");
  await press("KeyF", { metaKey: true, shiftKey: true }, "f");
  expect(panels()).toEqual(before);
  expect(document.querySelector("[cmdk-dialog]")).toBeNull();
  expect(calls("search.text")).toEqual([]);
  expect(await findActions("툴 윈도우")).toEqual([]);
  expect(await findActions("전체에서 찾기")).toEqual([]);
  // 에디터 액션은 있다.
  expect((await findActions("프로젝트로 열기")).length).toBe(1);
});

it("opens as a project through the native command from the status bar", async () => {
  await mount();
  const status = document.querySelector<HTMLElement>("[data-testid=status-file-window]")!;
  expect(status.textContent).toContain("단일 파일");
  await clickButton(status, "프로젝트로 열기");
  expect(fileNative.openAsProject).toHaveBeenCalledTimes(1);
});

it("asks before closing with unsaved edits and closes the window after the last tab", async () => {
  await mount();
  await typeInto(views()[0]!, 0, "> ");
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  expect(dialog()?.textContent).toContain("a.txt");
  await clickButton(dialog()!, "취소");
  expect(await answer).toBe(false);

  await press("KeyW", { metaKey: true }, "w");
  expect(dialog()).not.toBeNull();
  expect(native.closeWindow).not.toHaveBeenCalled();
  await clickButton(dialog()!, "저장 안 함");
  expect(tabs()).toEqual([]);
  expect(calls("fs.write")).toEqual([]);
  expect(native.closeWindow).toHaveBeenCalledTimes(1);
});

const notice = () => document.querySelector<HTMLElement>("[data-editor-tab] [role=status]");

/** 50 MiB 를 넘는 그 파일을 열어 「열지 않음」 화면을 기다린다. */
async function mountHuge() {
  host.hugeSize = 60 * 1024 * 1024;
  host.text = "head of a big log\n";
  // happy-dom 은 배치를 하지 않는다 — hex 뷰어의 스크롤 영역만 높이를 준다.
  const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.dataset.hexScroll !== undefined ? 400 : (clientHeight?.get?.call(this) ?? 0);
  });
  await mount();
  await vi.waitFor(() => expect(notice()?.textContent).toContain("50 MiB 를 넘는 파일"));
  expect(calls("fs.read")).toEqual([]);
}

it("offers the head and hex of a file too large to open, but no external tool", async () => {
  await mountHuge();
  expect([...notice()!.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
    "앞부분 1.0 MiB 보기",
    "hex 로 보기",
  ]);
});

it("reads a hex page of a file too large to open through this connection", async () => {
  await mountHuge();
  await clickButton(notice()!, "hex 로 보기");
  await vi.waitFor(() =>
    expect(document.querySelector("[data-hex-row]")?.textContent).toContain(
      "68 65 61 64 20 6F 66 20",
    ),
  );
  expect(calls("fs.readBytes")).toEqual([
    { projectId: "", worktree: null, path: FILE, offset: 0, length: HEX_PAGE_BYTES },
  ]);
});

it("shows the head of a file too large to open through this connection", async () => {
  await mountHuge();
  await clickButton(notice()!, "앞부분 1.0 MiB 보기");
  await vi.waitFor(() => expect(views()).toHaveLength(1));
  expect(views()[0]!.state.doc.line(1).text).toBe("head of a big log");
  expect(views()[0]!.state.readOnly).toBe(true);
  // StrictMode 의 개발 모드는 효과를 두 번 돌려 같은 읽기가 두 번일 수 있다.
  expect(new Set(calls("fs.read").map((c) => JSON.stringify(c)))).toEqual(
    new Set([
      JSON.stringify({
        projectId: "",
        worktree: null,
        path: FILE,
        offset: 0,
        length: HEAD_PREVIEW_BYTES,
      }),
    ]),
  );
});
