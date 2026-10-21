// @vitest-environment happy-dom
// 대용량 · 바이너리 · hex · 인코딩(12-m1-plan M1-23 완료 기준)을 프로젝트 창(App kind="project", `?project=p1`)과 가짜 Host 로 본다:
// 1 GB 파일은 「열지 않음」 화면이 1초 안에 뜨고 읽은 바이트가 0 이다, hex 에서 그 파일 끝으로 가면 끝 페이지만 읽는다, 50 MiB 를 넘는
// 파일의 앞부분 1 MiB 를 읽기 전용 텍스트로 본다, FIFO 는 읽지 않는다, 탭 메뉴로 텍스트 · hex 를 오간다, EUC-KR 파일이 한글로 읽기
// 전용이고 어떤 길로도 디스크에 쓰지 않는다, 기준값 경계 ±1 바이트가 정한 동작으로 열린다, 1만 자 넘는 줄은 강조 · 줄바꿈을 끈다.
//
// 가짜 Host 의 판정은 crates/pw-fs 와 같은 규칙이다(앞 8 KiB 의 NUL → 바이너리, UTF-8 이 아니면 `encodings` 의 인코딩으로 풀고 편집 끔,
// 그 인코딩이 `UTF-8` 이면 깨진 바이트를 U+FFFD 로 푼 `lossy`, 크기 등급은 limits.ts 의 기준값). 큰 파일은 바이트를 만들지 않는 가상
// 파일(`virtual`)이다.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  EntryKind,
  FsBytes,
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
import { EDITABLE_MAX, HEAD_PREVIEW_BYTES, HEX_PAGE_BYTES, READ_ONLY_MAX } from "./limits";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** 경로 → 바이트. */
  files: new Map<string, Uint8Array>(),
  /** 경로 → 가상 파일: `size` 바이트를 `pattern` 으로 채우고 끝에 `tail` 을 둔다(바이트를 만들지 않는다). */
  virtual: new Map<string, { size: number; pattern: number[]; tail?: number[] }>(),
  /** 경로 → 알려 줄 크기(내용 길이 대신, 기준값 경계 시험). */
  sizes: new Map<string, number>(),
  /** 경로 → UTF-8 이 아닌 텍스트의 인코딩. */
  encodings: new Map<string, string>(),
  /** 경로 → stat 의 종류(FIFO 는 `other`). */
  kinds: new Map<string, EntryKind>(),
  /** `fs.readBytes` 응답을 이만큼 늦춘다(ms, 느린 Host · 원격). */
  readBytesDelayMs: 0,
  /** 경로 → mtime(바꾸면 `version` 도 바뀐다). 없으면 1. */
  mtimes: new Map<string, number>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
  subscribers: [] as { send(m: object): void; id: number; topics: string[] }[],
}));

/** 감시 이벤트 `fs.changed` 를 구독한 연결에 보낸다. */
async function changed(paths: string[]) {
  await act(async () => {
    for (const sub of host.subscribers)
      if (sub.topics.includes("fs.changed"))
        sub.send({
          method: "events.event",
          params: {
            subscriptionId: sub.id,
            projectId: "p1",
            topic: "fs.changed",
            payload: {
              root: "/work/demo",
              worktree: null,
              paths,
              kinds: paths.map(() => "modified"),
              rescan: false,
            },
          },
        });
  });
}

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  const limits = await import("./limits");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32602, message, data: null } });
        if (["fs.", "syntax.", "lang."].some((p) => m.method.startsWith(p)))
          host.calls.push({ method: m.method, params: m.params ?? {} });
        const params = (m.params ?? {}) as {
          path?: string;
          offset?: number;
          length?: number;
          bytes?: boolean;
        };
        const path = params.path ?? "";
        const exists = () => host.files.has(path) || host.virtual.has(path) || host.kinds.has(path);
        const realSize = () => host.virtual.get(path)?.size ?? host.files.get(path)?.length ?? 0;
        const size = () => host.sizes.get(path) ?? realSize();
        const mtime = () => host.mtimes.get(path) ?? 1;
        /** `[offset, offset + length)` 의 바이트(파일 끝까지). */
        const bytes = (offset: number, length: number): Uint8Array => {
          const end = Math.min(realSize(), offset + length);
          const v = host.virtual.get(path);
          if (!v) return host.files.get(path)!.slice(offset, end);
          const out = new Uint8Array(Math.max(0, end - offset));
          const tail = v.tail ?? [];
          for (let i = 0; i < out.length; i++) {
            const at = offset + i;
            const fromTail = at - (v.size - tail.length);
            out[i] = fromTail >= 0 ? tail[fromTail]! : v.pattern[at % v.pattern.length]!;
          }
          return out;
        };
        const sizeClass = () =>
          size() > limits.READ_ONLY_MAX
            ? "tooLarge"
            : size() > limits.EDITABLE_MAX
              ? "readOnly"
              : "editable";
        const base64 = (b: Uint8Array) => Buffer.from(b).toString("base64");
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
          case "fs.stat": {
            if (!exists()) return fail(`not found: ${path}`);
            const kind = host.kinds.get(path) ?? "file";
            return reply({
              path,
              kind,
              target: null,
              size: kind === "file" ? size() : 0,
              mtimeMs: mtime(),
              readonly: false,
              sizeClass: kind === "file" ? sizeClass() : null,
            } satisfies FsStat);
          }
          case "fs.read": {
            if (!exists()) return fail(`not found: ${path}`);
            if (host.kinds.has(path)) return fail(`\`${path}\` is not a regular file`);
            const whole = params.offset == null && params.length == null;
            if (whole && size() > limits.READ_ONLY_MAX) return fail("read a range");
            const offset = params.offset ?? 0;
            const part = bytes(offset, params.length ?? limits.READ_ONLY_MAX);
            const binary = bytes(0, limits.SNIFF_BYTES).includes(0);
            let text: string | undefined;
            let encoding: string | null = null;
            let lossy = false;
            if (!binary) {
              try {
                text = new TextDecoder("utf-8", { fatal: true }).decode(part);
                encoding = "UTF-8";
              } catch {
                encoding = host.encodings.get(path) ?? "windows-1252";
                lossy = encoding === "UTF-8";
                text = new TextDecoder(encoding).decode(part);
              }
            }
            const editableClass = sizeClass() === "editable";
            const longLines = text?.split("\n").some((l) => l.length > limits.LONG_LINE_CHARS);
            return reply({
              path,
              size: size(),
              mtimeMs: mtime(),
              offset,
              eof: offset + part.length >= realSize(),
              version: whole ? `${mtime()}:${path}` : null,
              content:
                text !== undefined && !params.bytes
                  ? { kind: "text", text }
                  : { kind: "base64", data: base64(part) },
              judgment: {
                binary,
                encoding,
                lossy,
                bom: null,
                lineEnding: "lf",
                longLines: longLines === true,
                sizeClass: sizeClass(),
                editable: !binary && encoding === "UTF-8" && !lossy && editableClass,
                highlight: !binary && editableClass && longLines !== true,
              },
            } satisfies FsRead);
          }
          case "fs.readBytes": {
            const p = m.params as { offset: number; length: number };
            if (p.length > limits.HEX_PAGE_BYTES) return fail("a byte page is at most 64 KiB");
            if (host.kinds.has(path)) return fail(`\`${path}\` is not a regular file`);
            const part = bytes(p.offset, p.length);
            const answer = () =>
              reply({
                path,
                size: realSize(),
                mtimeMs: 1,
                offset: p.offset,
                data: base64(part),
                eof: p.offset + part.length >= realSize(),
              } satisfies FsBytes);
            if (host.readBytesDelayMs > 0) return void setTimeout(answer, host.readBytesDelayMs);
            return answer();
          }
          case "fs.write":
            return fail("this test never writes");
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map((f) => ({
                language: f.path.endsWith(".js") ? "JavaScript" : null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "events.subscribe": {
            const id = next++;
            host.subscribers.push({ send, id, topics: (m.params as { topics: string[] }).topics });
            return reply({ subscriptionId: id });
          }
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

/** 창의 닫기 요청을 받으면 묻는 함수(`guardClose`). */
let closeGuard: (() => Promise<boolean>) | undefined;

const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: (shouldClose: () => Promise<boolean>) => {
    closeGuard = shouldClose;
    return () => {
      if (closeGuard === shouldClose) closeGuard = undefined;
    };
  },
} satisfies KeymapNative;

const GIB = 1024 * 1024 * 1024;
const HUGE = "/work/demo/huge.bin";
const LOG = "/work/demo/big.log";
const FIFO = "/work/demo/pipe";
const BLOB = "/work/demo/blob.bin";
const EUC_KR = "/work/demo/euc-kr.txt";
/** 「// 한글 주석\nlet 이름 = "가나다";\n」 의 EUC-KR 바이트. */
const EUC_KR_BYTES = [
  47, 47, 32, 199, 209, 177, 219, 32, 193, 214, 188, 174, 10, 108, 101, 116, 32, 192, 204, 184, 167,
  32, 61, 32, 34, 176, 161, 179, 170, 180, 217, 34, 59, 10,
];
const HANGUL = '// 한글 주석\nlet 이름 = "가나다";\n';

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  // happy-dom 은 배치를 하지 않는다 — hex 뷰어의 스크롤 영역만 높이 400px(22줄)로 둔다.
  const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.dataset.hexScroll !== undefined ? 400 : (clientHeight?.get?.call(this) ?? 0);
  });
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [BLOB, Uint8Array.from([0x7f, 0x45, 0x4c, 0x46, 0, 1, 2, 3, 0x41, 0x42, 0x43, 10, 0xff])],
    [EUC_KR, Uint8Array.from(EUC_KR_BYTES)],
  ]);
  host.virtual = new Map([
    // 희소 파일처럼 0 으로 차 있고 끝에 「END\n」.
    [HUGE, { size: GIB, pattern: [0], tail: [...Buffer.from("END\n")] }],
    // 60 MiB 텍스트 로그.
    [LOG, { size: 60 * 1024 * 1024, pattern: [...Buffer.from("log line\n")] }],
  ]);
  host.sizes = new Map();
  host.encodings = new Map([[EUC_KR, "EUC-KR"]]);
  host.kinds = new Map([[FIFO, "other"]]);
  host.readBytesDelayMs = 0;
  host.mtimes = new Map();
  host.calls = [];
  host.subscribers = [];
  closeGuard = undefined;
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

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

async function open(file: string, line?: number, col?: number) {
  await act(async () => projectOpenInbox.push({ file, line, col }));
  await settle();
  await settle();
}

const calls = (method: string) =>
  host.calls.filter((c) => c.method === method).map((c) => c.params);
/** `fs.read` · `fs.readBytes` 로 읽은 바이트 수(요청한 길이, 파일 끝에서 자름). */
const bytesRead = (path: string) =>
  host.calls
    .filter((c) => c.params.path === path && ["fs.read", "fs.readBytes"].includes(c.method))
    .reduce((sum, c) => {
      const size = host.virtual.get(path)?.size ?? host.files.get(path)?.length ?? 0;
      const offset = (c.params.offset as number | undefined) ?? 0;
      const length = (c.params.length as number | undefined) ?? size;
      return sum + Math.max(0, Math.min(size, offset + length) - offset);
    }, 0);

async function keydown(init: KeyboardEventInit, target?: Element) {
  await act(async () => {
    (target ?? document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

async function click(el: Element) {
  await act(async () => (el as HTMLElement).click());
  await settle();
}

async function typeInto(input: HTMLInputElement, value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const toasts = () =>
  [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent ?? "");
const notice = () => document.querySelector<HTMLElement>("[data-editor-tab] [role=status]");
const hexRows = () =>
  [...document.querySelectorAll<HTMLElement>("[data-hex-row]")].map((r) => ({
    offset: Number(r.dataset.hexRow),
    text: r.textContent ?? "",
  }));
const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((e) => EditorView.findFromDOM(e)!);
const menuItems = () =>
  [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].map((i) => i.textContent);

async function openTabMenu() {
  const header = document.querySelector<HTMLElement>("[data-editor-tab-header]")!;
  await act(async () =>
    header.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 5, clientY: 5 }),
    ),
  );
  await settle();
}

async function pickMenu(label: string) {
  const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (i) => i.textContent === label,
  );
  if (!item) throw new Error(`메뉴 ${label} 이 없다: ${menuItems().join(", ")}`);
  await act(async () => item.click());
  await settle();
  await settle();
}

it("shows a 1 GB file as not opened within a second without reading a byte, and hex reads only the last page at the end", async () => {
  await mount();
  const started = performance.now();
  await act(async () => projectOpenInbox.push({ file: HUGE }));
  await vi.waitFor(() => expect(notice()?.textContent).toContain("50 MiB 를 넘는 파일"), {
    timeout: 1000,
    interval: 5,
  });
  expect(performance.now() - started).toBeLessThan(1000);
  expect(notice()?.textContent).toContain("1024.0 MiB");
  expect(bytesRead(HUGE)).toBe(0);
  expect(calls("fs.read")).toEqual([]);
  // [앞부분 1 MiB 보기] [hex 로 보기] [외부 도구로 열기].
  expect([...notice()!.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
    "앞부분 1.0 MiB 보기",
    "hex 로 보기",
    "다음에서 열기…",
  ]);

  await click(document.querySelector("[data-notice-action=hex]")!);
  await vi.waitFor(() => expect(hexRows()[0]?.text).toContain("00000000"));
  // 첫 화면: 첫 페이지만.
  expect(calls("fs.readBytes")).toEqual([
    { projectId: "p1", worktree: null, path: HUGE, offset: 0, length: HEX_PAGE_BYTES },
  ]);
  // 400px 를 채우는 23줄 + 아래에 걸친 한 줄만 그린다(가상 스크롤).
  expect(hexRows()).toHaveLength(Math.ceil(400 / 18) + 1);
  expect(hexRows()[0]!.text).toBe(
    `00000000${"00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00"}................`,
  );

  // ⌘L(hex 포커스) → 오프셋 칸. 줄로 이동 대화상자가 아니다.
  host.calls = [];
  const scroller = document.querySelector<HTMLElement>("[data-hex-scroll]")!;
  await act(async () => scroller.focus());
  await keydown({ code: "KeyL", key: "l", metaKey: true });
  expect(document.querySelector("[data-testid=goto-line-dialog]")).toBeNull();
  const input = document.querySelector<HTMLInputElement>("[data-testid=goto-offset-dialog] input")!;
  await typeInto(input, "0x3FFFFFFF");
  await keydown({ key: "Enter", code: "Enter" }, input);
  await vi.waitFor(() => expect(hexRows().at(-1)?.text).toContain("END."));
  // 끝으로 가면 끝 페이지 하나만 읽는다 — 그 사이 페이지는 읽지 않는다.
  expect(calls("fs.readBytes")).toEqual([
    {
      projectId: "p1",
      worktree: null,
      path: HUGE,
      offset: GIB - HEX_PAGE_BYTES,
      length: HEX_PAGE_BYTES,
    },
  ]);
  expect(bytesRead(HUGE)).toBe(HEX_PAGE_BYTES);
  expect(hexRows().at(-1)).toEqual({
    offset: GIB - 16,
    text: `3FFFFFF0${"00 00 00 00 00 00 00 00  00 00 00 00 45 4E 44 0A"}............END.`,
  });
  // 고른 바이트(마지막 \n)가 hex · ASCII 두 칸에서 강조된다.
  expect(
    [...document.querySelectorAll("[data-hex-row] [data-selected]")].map((e) => e.textContent),
  ).toEqual(["0A", "."]);
  expect(document.activeElement).toBe(scroller);
});

it("asks a slow Host for a hex page once, however often the view scrolls within that page", async () => {
  // 응답이 오기 전에 같은 페이지 안에서 스크롤해도 그 페이지를 다시 요청하지 않고, 도착한 응답을 버리지 않는다.
  host.readBytesDelayMs = 100;
  await mount();
  await open(HUGE);
  await click(document.querySelector("[data-notice-action=hex]")!);
  const scroller = document.querySelector<HTMLElement>("[data-hex-scroll]")!;
  for (let i = 1; i <= 12; i++) {
    await act(async () => {
      scroller.scrollTop = i * 4;
      scroller.dispatchEvent(new Event("scroll"));
    });
    await act(async () => new Promise((r) => setTimeout(r, 40)));
  }
  // 1 GB 의 스크롤 축척에서 48px 은 수백 줄이다 — 아직 첫 페이지(4096줄) 안이다.
  expect(hexRows()[0]!.offset).toBeGreaterThan(0);
  expect(hexRows()[0]!.offset).toBeLessThan(HEX_PAGE_BYTES);
  await vi.waitFor(() => expect(hexRows()[0]!.text).toContain("00 00 00 00"));
  expect(calls("fs.readBytes").map((c) => c.offset)).toEqual([0]);
});

it("shows the first 1 MiB of a file over 50 MiB as read-only text", async () => {
  await mount();
  await open(LOG);
  expect(calls("fs.read")).toEqual([]);
  await click(document.querySelector("[data-notice-action=head]")!);
  await vi.waitFor(() => expect(views()).toHaveLength(1));
  // 앞 1 MiB 만 읽는다(StrictMode 의 개발 모드는 효과를 두 번 돌려 같은 읽기가 두 번일 수 있다).
  expect(new Set(calls("fs.read").map((c) => JSON.stringify(c)))).toEqual(
    new Set([
      JSON.stringify({
        projectId: "p1",
        worktree: null,
        path: LOG,
        offset: 0,
        length: HEAD_PREVIEW_BYTES,
      }),
    ]),
  );
  expect(document.querySelector("[data-testid=raw-text-viewer]")?.textContent).toContain(
    "앞부분 1.0 MiB 만 보인다(전체 60.0 MiB) · 읽기 전용",
  );
  const view = views()[0]!;
  expect(view.state.readOnly).toBe(true);
  expect(view.state.doc.line(1).text).toBe("log line");
  expect(view.state.doc.length).toBe(HEAD_PREVIEW_BYTES);
});

it("does not read a FIFO and offers only an external tool", async () => {
  await mount();
  await open(FIFO);
  expect(notice()?.textContent).toContain("일반 파일이 아니어서(FIFO · 장치 · 소켓) 열지 않는다");
  expect([...notice()!.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
    "다음에서 열기…",
  ]);
  expect(calls("fs.read")).toEqual([]);
  expect(calls("fs.readBytes")).toEqual([]);
  await openTabMenu();
  expect(menuItems()).not.toContain("텍스트로 열기");
  expect(menuItems()).not.toContain("hex 로 열기");
});

it("opens a binary file in hex, and as text or back from the tab menu", async () => {
  await mount();
  await open(BLOB);
  await vi.waitFor(() => expect(hexRows()).toHaveLength(1));
  expect(hexRows()[0]).toEqual({
    offset: 0,
    text: `00000000${"7F 45 4C 46 00 01 02 03  41 42 43 0A FF"}${" ".repeat(9)}.ELF....ABC..`,
  });
  // 13바이트 파일: 페이지 하나, 파일 크기만큼.
  expect(calls("fs.readBytes")).toMatchObject([{ offset: 0, length: 13 }]);

  await openTabMenu();
  expect(menuItems()).toContain("텍스트로 열기");
  expect(menuItems()).not.toContain("hex 로 열기");
  expect(menuItems()).not.toContain("기본 뷰어로 열기");
  await pickMenu("텍스트로 열기");
  await vi.waitFor(() => expect(views()).toHaveLength(1));
  expect(views()[0]!.state.readOnly).toBe(true);
  expect(views()[0]!.state.doc.toString()).toBe("\x7fELF\0\x01\x02\x03ABC\n�");
  expect(document.querySelector("[data-testid=raw-text-viewer]")?.textContent).toContain(
    "바이너리 파일을 텍스트로 보인다",
  );
  expect(calls("fs.read").at(-1)).toMatchObject({ offset: 0, length: READ_ONLY_MAX });

  await openTabMenu();
  expect(menuItems()).toContain("hex 로 열기");
  expect(menuItems()).toContain("기본 뷰어로 열기");
  await pickMenu("기본 뷰어로 열기");
  await vi.waitFor(() => expect(hexRows()).toHaveLength(1));
  expect(views()).toEqual([]);
});

it("opens a text file in hex from the tab menu", async () => {
  host.files.set("/work/demo/a.txt", new TextEncoder().encode("hi\n"));
  await mount();
  await open("/work/demo/a.txt");
  expect(views()[0]!.state.doc.toString()).toBe("hi\n");
  await openTabMenu();
  expect(menuItems()).not.toContain("텍스트로 열기");
  await pickMenu("hex 로 열기");
  await vi.waitFor(() => expect(hexRows()).toHaveLength(1));
  expect(hexRows()[0]!.text).toContain("68 69 0A");
});

it("shows EUC-KR text in Hangul, read-only, and no key, save or close writes it back", async () => {
  await mount();
  await open(EUC_KR, 1, 6);
  const view = views()[0]!;
  expect(view.state.doc.toString()).toBe(HANGUL);
  expect(view.state.readOnly).toBe(true);
  expect(document.querySelector("[data-read-only-notice]")?.textContent).toBe(
    "EUC-KR 로 읽었다 · UTF-8 이 아니라 읽기 전용으로 연다",
  );
  expect(document.querySelector("[data-testid=status-encoding]")?.textContent).toBe("EUC-KR");

  expect(view.hasFocus).toBe(true);
  for (const key of [
    { code: "Backspace", key: "Backspace" },
    { code: "Delete", key: "Delete" },
    { code: "Enter", key: "Enter" },
    { code: "KeyD", key: "d", metaKey: true },
    { code: "Backspace", key: "Backspace", metaKey: true },
    { code: "ArrowUp", key: "ArrowUp", altKey: true, shiftKey: true },
    { code: "Slash", key: "/", metaKey: true },
  ])
    await keydown(key, view.contentDOM);
  expect(view.state.doc.toString()).toBe(HANGUL);
  await keydown({ code: "KeyS", key: "s", metaKey: true }, view.contentDOM);
  // 창을 닫아도 묻지 않고(미저장이 없다) 쓰지 않는다.
  expect(await act(() => closeGuard!())).toBe(true);
  expect(document.querySelector("[role=alertdialog]")).toBeNull();
  expect(calls("fs.write")).toEqual([]);
});

it("shows UTF-8 with a broken byte in Hangul as UTF-8, read-only, and no key, save or close writes it back", async () => {
  // 쓰는 중인 한글 로그: 끝에 깨진 바이트 하나. 파일 전체를 다른 인코딩(windows-1252)으로 깨뜨려 보이지 않는다.
  const LOSSY = "/work/demo/app.log";
  host.files.set(LOSSY, Uint8Array.from([...Buffer.from(HANGUL), 0xff]));
  host.encodings.set(LOSSY, "UTF-8");
  await mount();
  await open(LOSSY, 1, 1);
  const view = views()[0]!;
  expect(view.state.doc.toString()).toBe(`${HANGUL}\uFFFD`);
  expect(view.state.readOnly).toBe(true);
  expect(document.querySelector("[data-read-only-notice]")?.textContent).toBe(
    "UTF-8 로 읽었다 · 깨진 바이트가 있어(�) 읽기 전용으로 연다",
  );
  expect(document.querySelector("[data-testid=status-encoding]")?.textContent).toBe(
    "UTF-8(깨진 바이트 있음)",
  );
  for (const key of [
    { code: "Backspace", key: "Backspace" },
    { code: "Enter", key: "Enter" },
    { code: "KeyD", key: "d", metaKey: true },
  ])
    await keydown(key, view.contentDOM);
  expect(view.state.doc.toString()).toBe(`${HANGUL}\uFFFD`);
  await keydown({ code: "KeyS", key: "s", metaKey: true }, view.contentDOM);
  expect(await act(() => closeGuard!())).toBe(true);
  expect(calls("fs.write")).toEqual([]);
});

it("says the save was blocked when the file turns read-only under unsaved edits, and the close dialog's Save keeps the window", async () => {
  // 편집하는 사이 디스크의 파일이 같은 글자의 EUC-KR 로 다시 저장됐다(외부 도구). 글자가 같아 배너 없이 판정만 바뀌어 읽기 전용이
  // 된다 — 그때 ⌘S · 닫기의 「저장」 이 말없이 성공한 것처럼 끝나면 편집이 사라진다.
  const NOTE = "/work/demo/note.txt";
  host.files.set(NOTE, new TextEncoder().encode(HANGUL));
  await mount();
  await open(NOTE);
  const view = views()[0]!;
  expect(view.state.readOnly).toBe(false);
  await act(async () =>
    view.dispatch({
      changes: { from: view.state.doc.length, insert: "// 내 편집\n" },
      userEvent: "input.type",
    }),
  );
  host.files.set(NOTE, Uint8Array.from(EUC_KR_BYTES));
  host.encodings.set(NOTE, "EUC-KR");
  host.mtimes.set(NOTE, 2);
  await changed([NOTE]);
  await vi.waitFor(() => expect(view.state.readOnly).toBe(true));
  expect(view.state.doc.toString()).toBe(`${HANGUL}// 내 편집\n`);

  await keydown({ code: "KeyS", key: "s", metaKey: true }, view.contentDOM);
  await vi.waitFor(() =>
    expect(toasts()).toContainEqual(
      expect.stringContaining("이 파일은 이제 읽기 전용이라 쓰지 않았다. 편집은 탭에 남아 있다."),
    ),
  );
  expect(toasts()).toContainEqual(expect.stringContaining(`저장하지 못했다: ${NOTE}`));

  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  const dialog = document.querySelector<HTMLElement>("[data-testid=unsaved-dialog]")!;
  expect(dialog.textContent).toContain("note.txt");
  const save = [...dialog.querySelectorAll("button")].find((b) => b.textContent === "저장")!;
  await click(save);
  expect(await answer).toBe(false);
  expect(toasts()).toContainEqual(expect.stringContaining("저장하지 못한 파일이 있어 닫지 않았다"));
  expect(view.state.doc.toString()).toBe(`${HANGUL}// 내 편집\n`);
  expect(calls("fs.write")).toEqual([]);
});

it("opens files one byte either side of the thresholds as decided", async () => {
  const text = new TextEncoder().encode("x\n");
  for (const [name, size] of [
    ["at-5m", EDITABLE_MAX],
    ["over-5m", EDITABLE_MAX + 1],
    ["at-50m", READ_ONLY_MAX],
    ["over-50m", READ_ONLY_MAX + 1],
  ] as const) {
    host.files.set(`/work/demo/${name}.txt`, text);
    host.sizes.set(`/work/demo/${name}.txt`, size);
  }
  await mount();
  await open("/work/demo/at-5m.txt");
  expect(views()[0]!.state.readOnly).toBe(false);
  expect(document.querySelector("[data-read-only-notice]")).toBeNull();

  for (const name of ["over-5m", "at-50m"]) {
    await open(`/work/demo/${name}.txt`);
    const view = views().at(-1)!;
    expect(view.state.readOnly, name).toBe(true);
    expect(document.querySelector("[data-read-only-notice]")?.textContent).toContain(
      "5 MiB 를 넘는 파일이라 읽기 전용으로 연다",
    );
  }
  const before = calls("fs.read").length;
  await open("/work/demo/over-50m.txt");
  expect(notice()?.textContent).toContain("50 MiB 를 넘는 파일이라 열지 않는다");
  expect(calls("fs.read")).toHaveLength(before);
});

it("turns highlighting and wrapping off for a line over 10,000 characters", async () => {
  host.files.set("/work/demo/min.js", new TextEncoder().encode(`var a=${"1+".repeat(5001)}1;`));
  host.files.set("/work/demo/ok.js", new TextEncoder().encode("var a = 1;\n"));
  await mount();
  await open("/work/demo/ok.js");
  await vi.waitFor(() => expect(calls("syntax.open")).toHaveLength(1));
  await open("/work/demo/min.js");
  const view = views().at(-1)!;
  expect(view.state.doc.line(1).length).toBeGreaterThan(10_000);
  await settle();
  // 강조 세션을 열지 않는다(ok.js 의 것 하나뿐), 줄바꿈도 없다.
  expect(calls("syntax.open")).toHaveLength(1);
  expect(view.contentDOM.classList.contains("cm-lineWrapping")).toBe(false);
});

it("turns highlighting off when the judgment says so, even for a short text", async () => {
  // 판정이 크기 등급으로 강조를 끄는 경우(5 MiB + 1 로 알려진 파일). 내용은 짧아 에디터 쪽 길이 검사(limits.ts 의
  // `overHighlightLimit`)는 걸리지 않는다 — 판정의 `highlight` 만 보고 꺼야 한다.
  host.files.set("/work/demo/big.js", new TextEncoder().encode("var a = 1;\n"));
  host.sizes.set("/work/demo/big.js", EDITABLE_MAX + 1);
  host.files.set("/work/demo/ok.js", new TextEncoder().encode("var a = 1;\n"));
  await mount();
  await open("/work/demo/ok.js");
  await vi.waitFor(() => expect(calls("syntax.open")).toHaveLength(1));
  await open("/work/demo/big.js");
  expect(views().at(-1)!.state.readOnly).toBe(true);
  await settle();
  expect(calls("syntax.open")).toHaveLength(1);
});
