// @vitest-environment happy-dom
// 멀티 커서(15-m4-plan M4-14 완료 기준)를 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 키는 창의 키맵 엔진(기본 키맵)을
// 지나고, 에디터는 실제 CodeMirror 다: ⌃G · ⌃⇧G · ⌃⌘G 의 선택 범위, ⌥⌥ 누른 채 ↓ 세 번 → 커서 넷(떼면 보통 키), 컬럼 모드(⌘⇧8)의
// ⇧+화살표 사각 선택이 짧은 줄을 지남, ⌥+드래그 · ⌥⇧+클릭의 마우스 판정, Esc(찾기 막대 · 팝업이 먼저), 줄 수 = 커서 수인 붙여넣기,
// 커서 셋에서 한글 조합, 상태바의 「N 커서」 · 「컬럼」. 창 틀(가짜 Host · 마운트)은 editor-ops.test.tsx 와 같다.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

import { EditorSelection, Prec, StateEffect, StateField } from "@codemirror/state";
import { EditorView, keymap, showTooltip } from "@codemirror/view";
import type {
  ConfigView,
  JsonValue,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  LangList,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { act, endTest, it } from "../test-support/ended-test";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { MAX_OCCURRENCES } from "./extensions/multi-cursor";
import type { fakeSyntaxServer } from "./syntax-testing";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
});

const host = vi.hoisted(() => ({
  files: new Map<string, string>(),
  /** 전역 settings.toml 의 값(`config.get`). */
  settings: {} as JsonValue,
  subscribers: [] as {
    send(m: object): void;
    id: number;
    topics: string[];
    projectId?: string;
  }[],
  /** 구문 분석 서비스(`syntax.*`). 없으면 그 메서드가 없는 Host 다(강조하지 않는다). */
  syntax: undefined as ReturnType<typeof fakeSyntaxServer> | undefined,
}));

const LANGUAGES: LangList = {
  languages: [
    { name: "Python", popular: true, lineComment: "#" },
    { name: "Rust", popular: true, lineComment: "//" },
    { name: "COBOL", popular: false, lineComment: null },
    { name: "Text", popular: false, lineComment: null },
  ],
};

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
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
        if (m.method.startsWith("syntax.") && host.syntax) {
          const { source } = host.syntax;
          const run = {
            "syntax.open": source.open,
            "syntax.edit": source.edit,
            "syntax.highlights": source.highlights,
            "syntax.close": source.close,
          }[m.method] as (p: unknown) => Promise<unknown>;
          return void run(m.params).then(reply, (e: { code?: number; message: string }) =>
            send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
          );
        }
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: {
                files: [{ id: "settings", value: host.settings, layers: [] }],
                notices: [],
              },
              error: null,
            } satisfies ConfigView);
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            } satisfies KeymapFile);
          // 상태바의 브랜치 항목(src/git/branch-state.ts)이 묻는다 — 이 시험은 저장소 없음으로 답한다.
          case "git.refs":
            return reply({
              workTree: null,
              head: null,
              branches: [],
              remoteBranches: [],
              tags: [],
              unsupported: null,
              broken: [],
            });
          case "git.operation":
            return reply({ operation: null });
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
            return reply({
              path,
              kind: "file",
              target: null,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: "editable",
            } satisfies FsStat);
          case "fs.read":
            return reply({
              path,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "1",
              content: { kind: "text", text: host.files.get(path)! },
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
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map((f) => ({
                language: f.path.endsWith(".rs")
                  ? "Rust"
                  : f.path.endsWith(".py")
                    ? "Python"
                    : null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "lang.list":
            return reply(LANGUAGES);
          case "events.subscribe": {
            const id = next++;
            const p = m.params as { topics: string[]; projectId?: string };
            host.subscribers.push({ send, id, topics: p.topics, projectId: p.projectId });
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

/** 네이티브 메뉴(src-tauri/src/menu.rs): 마지막으로 받은 accelerator 와 메뉴에서 고른 액션을 보내는 자리. */
const menu = {
  accelerators: {} as Record<string, string | null>,
  choose: undefined as ((action: string) => void) | undefined,
};

const native = {
  syncMenu: async (accelerators) => {
    menu.accelerators = accelerators;
  },
  onMenuAction: (listener) => {
    menu.choose = listener;
    return () => {
      if (menu.choose === listener) menu.choose = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: () => () => {},
} satisfies KeymapNative;

/** 낱말 `foo` 셋(셋째 줄의 `foobar` 는 낱말이 아니다). */
const WORDS = "/work/demo/words.py";
/** 가운데 줄이 짧다(사각 선택이 지나간다). */
const BLOCK = "/work/demo/block.py";
/** 모든 일치 선택의 상한(`MAX_OCCURRENCES`)을 넘는 낱말(400 줄). */
const MANY = "/work/demo/many.py";
/** 커서 셋을 둘 같은 줄 셋. */
const ROWS = "/work/demo/rows.py";

let root: Root | undefined;

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [WORDS, "foo = 1\nbar = foo\nfoobar = foo\n"],
    [BLOCK, "abcdef\nab\nabcdef\n"],
    [ROWS, "a\nb\nc\n"],
    [MANY, "foo\n".repeat(400)],
  ]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
  // happy-dom 은 DOM 선택을 바꾸는 그 자리에서 `selectionchange` 를 동기로 보낸다(브라우저는 나중에 보낸다). 줄 수가 바뀌는 편집이면
  // CodeMirror 가 자기 업데이트 도중에 자기가 둔 선택의 이벤트를 받아 「update in progress」 로 멈춘다. 브라우저에서는 그 이벤트가
  // 업데이트 뒤에 와 아무것도 바꾸지 않으므로, 업데이트 중에 온 것은 버린다(CodeMirror 의 `updateState`: 0 = 쉼).
  const dispatch = document.dispatchEvent.bind(document);
  vi.spyOn(document, "dispatchEvent").mockImplementation((e) => {
    const updating = [...document.querySelectorAll<HTMLElement>(".cm-editor")].some(
      (dom) =>
        (EditorView.findFromDOM(dom) as unknown as { updateState: number }).updateState !== 0,
    );
    return e.type === "selectionchange" && updating ? true : dispatch(e);
  });
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
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
      <ThemeStoreProvider store={store}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

async function open(file: string, line?: number, col?: number) {
  await act(async () => projectOpenInbox.push({ file, line, col }));
  await settle();
}

async function keydown(init: KeyboardEventInit, target?: Element | null) {
  await act(async () => {
    (target ?? document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

const view = () =>
  EditorView.findFromDOM(document.querySelector<HTMLElement>(".dv-active-group .cm-editor")!)!;
const statusText = (id: string) =>
  document.querySelector(`[data-status-item=${id}]`)?.textContent ?? "";

/** 선택 범위들(anchor, head). */
const ranges = () => view().state.selection.ranges.map((r) => [r.anchor, r.head]);
const ctrl = (code: string, more: KeyboardEventInit = {}) =>
  keydown({ code, key: code.replace(/^Key/, "").toLowerCase(), ctrlKey: true, ...more });

async function keyup(init: KeyboardEventInit) {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keyup", { bubbles: true, cancelable: true, ...init }),
    );
  });
}

/** ⌥ 를 한 번 탭하고 두 번째 누름을 누른 채 둔다(키맵 엔진의 `doubleHold`). */
async function holdAltTwice() {
  await keydown({ code: "AltLeft", key: "Alt", altKey: true });
  await keyup({ code: "AltLeft", key: "Alt" });
  await keydown({ code: "AltLeft", key: "Alt", altKey: true });
}

it("adds the next occurrence with ⌃G (the word first), drops only the last with ⌃⇧G, and counts the cursors in the status bar", async () => {
  await mount();
  await open(WORDS, 1, 2);
  expect(statusText("cursor")).toBe("1:2");
  await ctrl("KeyG");
  // 선택이 없으면 커서의 낱말.
  expect(ranges()).toEqual([[0, 3]]);
  await ctrl("KeyG");
  // 낱말 단위로 맞춘다 — `foobar` 의 앞은 건너뛴다.
  expect(ranges()).toEqual([
    [0, 3],
    [14, 17],
  ]);
  await ctrl("KeyG");
  expect(ranges()).toEqual([
    [0, 3],
    [14, 17],
    [27, 30],
  ]);
  expect(statusText("cursor")).toBe("3 커서 (9자)");
  // 한글 입력 상태에서도 물리 키 자리로 맞춘다.
  await keydown({ code: "KeyG", key: "ㅎ", ctrlKey: true, shiftKey: true });
  expect(ranges()).toEqual([
    [0, 3],
    [14, 17],
  ]);
  await ctrl("KeyG", { shiftKey: true });
  expect(ranges()).toEqual([[0, 3]]);
  // 첫 선택은 남는다.
  await ctrl("KeyG", { shiftKey: true });
  expect(ranges()).toEqual([[0, 3]]);
  expect(statusText("cursor")).toBe("1:4 (3자)");
});

it("takes back the main cursor with ⌃⇧G when the cursors came from ⌥⇧+click or ⌥⌥↓", async () => {
  await mount();
  await open(ROWS, 1, 1);
  await act(async () =>
    view().dispatch({
      selection: EditorSelection.create(
        [EditorSelection.cursor(0), EditorSelection.cursor(2), EditorSelection.cursor(4)],
        2,
      ),
    }),
  );
  await ctrl("KeyG", { shiftKey: true });
  expect(ranges()).toEqual([
    [0, 0],
    [2, 2],
  ]);
  expect(view().state.selection.mainIndex).toBe(1);
  await ctrl("KeyG", { shiftKey: true });
  expect(ranges()).toEqual([[0, 0]]);
});

it("selects every whole-word occurrence of the cursor's word with ⌃⌘G, and every text match of a selection", async () => {
  await mount();
  await open(WORDS, 2, 8);
  await ctrl("KeyG", { metaKey: true });
  expect(ranges()).toEqual([
    [0, 3],
    [14, 17],
    [27, 30],
  ]);
  // 주 범위는 커서가 있던 일치.
  expect(view().state.selection.main.from).toBe(14);
  await act(async () => view().dispatch({ selection: { anchor: 0, head: 3 } }));
  await ctrl("KeyG", { metaKey: true });
  expect(ranges()).toEqual([
    [0, 3],
    [14, 17],
    [18, 21],
    [27, 30],
  ]);
});

it("adds a cursor below for each ↓ while ⌥⌥ is held, and a plain ↓ after ⌥ goes up", async () => {
  await mount();
  await open(BLOCK, 1, 5);
  await holdAltTwice();
  for (let i = 0; i < 3; i++) {
    await keydown({ code: "ArrowDown", key: "ArrowDown", altKey: true });
  }
  // 짧은 둘째 줄은 줄 끝, 셋째 줄에서 다시 넷째 열, 빈 넷째 줄은 줄 끝.
  expect(ranges()).toEqual([
    [4, 4],
    [9, 9],
    [14, 14],
    [17, 17],
  ]);
  expect(view().state.selection.mainIndex).toBe(3);
  expect(statusText("cursor")).toBe("4 커서");
  await keyup({ code: "AltLeft", key: "Alt" });
  // 수정 키를 뗀 뒤에는 ⌥ 를 한 번만 누른 ↓ 도 커서를 더하지 않는다(⌥↓ 는 보통 키 — 지금은 바인딩이 없다).
  await keydown({ code: "AltLeft", key: "Alt", altKey: true });
  await keydown({ code: "ArrowDown", key: "ArrowDown", altKey: true });
  await keyup({ code: "AltLeft", key: "Alt" });
  expect(ranges()).toEqual([
    [4, 4],
    [9, 9],
    [14, 14],
    [17, 17],
  ]);
  // ⌥ 없는 ↓ 는 CodeMirror 의 보통 이동이다 — 커서가 늘지 않는다.
  await keydown({ code: "ArrowDown", key: "ArrowDown" }, view().contentDOM);
  expect(view().state.selection.ranges.length).toBeLessThanOrEqual(4);
});

it("selects a rectangle with ⇧+arrows in column mode (⌘⇧8), turning a short line into a cursor at its end", async () => {
  await mount();
  await open(BLOCK, 1, 4);
  await keydown({ code: "Digit8", key: "*", metaKey: true, shiftKey: true });
  expect(document.querySelector("[data-testid=status-column-mode]")?.textContent).toBe("컬럼");
  const shift = (code: string) => keydown({ code, key: code, shiftKey: true });
  await shift("ArrowRight");
  await shift("ArrowRight");
  await shift("ArrowDown");
  await shift("ArrowDown");
  expect(ranges()).toEqual([
    [3, 5],
    [9, 9],
    [13, 15],
  ]);
  expect(statusText("cursor")).toBe("3 커서 (4자)컬럼");
  // 위로 거두면 사각형이 줄어든다.
  await shift("ArrowUp");
  expect(ranges()).toEqual([
    [3, 5],
    [9, 9],
  ]);
  // 끄면 ⇧↓ 는 보통 선택이다.
  await keydown({ code: "Digit8", key: "*", metaKey: true, shiftKey: true });
  expect(document.querySelector("[data-testid=status-column-mode]")).toBeNull();
  await act(async () => view().dispatch({ selection: { anchor: 3 } }));
  await shift("ArrowRight");
  expect(ranges()).toEqual([[3, 4]]);
});

it("drags a rectangle with ⌥ (or in column mode), and ⌥⇧+click — not ⌘+click — adds a cursor", async () => {
  await mount();
  await open(BLOCK, 1, 1);
  const target = view().contentDOM.querySelector(".cm-line")!;
  const press = (init: MouseEventInit) =>
    new MouseEvent("mousedown", { button: 0, clientX: 1, clientY: 1, ...init, bubbles: true });
  const fakeTarget = (e: MouseEvent) => {
    Object.defineProperty(e, "target", { value: target });
    return e;
  };
  const rectangular = (init: MouseEventInit) =>
    view()
      .state.facet(EditorView.mouseSelectionStyle)
      .some((style) => style(view(), fakeTarget(press(init))) !== null);
  const addsRange = (init: MouseEventInit) =>
    view().state.facet(EditorView.clickAddsSelectionRange)[0]!(press(init));
  expect(rectangular({ altKey: true })).toBe(true);
  expect(rectangular({ altKey: true, shiftKey: true })).toBe(false);
  expect(rectangular({})).toBe(false);
  expect(addsRange({ altKey: true, shiftKey: true })).toBe(true);
  expect(addsRange({ metaKey: true })).toBe(false);
  expect(addsRange({ ctrlKey: true })).toBe(false);
  await keydown({ code: "Digit8", key: "*", metaKey: true, shiftKey: true });
  expect(rectangular({})).toBe(true);
});

/** React 가 듣는 입력 칸에 글자를 넣는다. */
async function typeInto(input: HTMLInputElement, text: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

const findInput = () => document.querySelector<HTMLInputElement>("input[aria-label='찾을 내용']")!;

it("collapses to one cursor with Esc in the text, and closes a find bar opened over the text first", async () => {
  await mount();
  await open(WORDS, 1, 1);
  await ctrl("KeyG", { metaKey: true });
  expect(view().state.selection.ranges).toHaveLength(3);
  // 찾기 막대가 열린 채 글자 영역에서 Esc — 막대를 먼저 닫는다.
  await keydown({ code: "KeyF", key: "f", metaKey: true });
  expect(document.querySelector("[role=search]")).not.toBeNull();
  await act(async () => view().focus());
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(document.querySelector("[role=search]")).toBeNull();
  expect(view().state.selection.ranges).toHaveLength(3);
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(ranges()).toEqual([[0, 3]]);
  expect(statusText("cursor")).toBe("1:4 (3자)");
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(ranges()).toEqual([[3, 3]]);
});

// 찾기 칸의 Esc 는 막대가 닫으며 막는다(`preventDefault`) — 같은 눌림이 창의 키맵까지 올라와도 커서 하나로가 돌지 않는다
// (M1-18 의 찾기 흐름: 찾은 일치가 선택으로 남는다).
it("leaves the found match selected when Esc closes the find bar from its field", async () => {
  await mount();
  await open(WORDS, 1, 1);
  await keydown({ code: "KeyF", key: "f", metaKey: true });
  await typeInto(findInput(), "bar");
  await keydown({ code: "Enter", key: "Enter" }, findInput());
  expect(ranges()).toEqual([[8, 11]]);
  await keydown({ code: "Escape", key: "Escape" }, findInput());
  expect(document.querySelector("[role=search]")).toBeNull();
  expect(ranges()).toEqual([[8, 11]]);
});

it("keeps every cursor when Esc closes the find bar from its field", async () => {
  await mount();
  await open(WORDS, 1, 1);
  await ctrl("KeyG", { metaKey: true });
  await keydown({ code: "KeyF", key: "f", metaKey: true });
  await act(async () => findInput().focus());
  await keydown({ code: "Escape", key: "Escape" }, findInput());
  expect(document.querySelector("[role=search]")).toBeNull();
  expect(view().state.selection.ranges).toHaveLength(3);
});

// 자동완성 목록처럼 CodeMirror 키맵이 Esc 로 닫는 툴팁: 툴팁은 Esc 를 받는 그 업데이트에서 DOM 까지 사라지므로 창이 볼 때는 이미
// 없다 — 「열려 있는가」 가 아니라 「이미 처리됐는가」 로 가른다.
it("keeps every cursor when Esc closes an editor popup that CodeMirror's keymap handles", async () => {
  await mount();
  await open(WORDS, 1, 1);
  await ctrl("KeyG", { metaKey: true });
  const close = StateEffect.define<null>();
  const popup = StateField.define<boolean>({
    create: () => true,
    update: (open, tr) => (tr.effects.some((e) => e.is(close)) ? false : open),
    provide: (f) =>
      showTooltip.from(f, (open) =>
        open
          ? {
              pos: 0,
              create: () => {
                const dom = document.createElement("div");
                dom.textContent = "completions";
                return { dom };
              },
            }
          : null,
      ),
  });
  const escape = Prec.highest(
    keymap.of([
      {
        key: "Escape",
        run: (v) => {
          if (!v.state.field(popup)) return false;
          v.dispatch({ effects: close.of(null) });
          return true;
        },
      },
    ]),
  );
  await act(async () => view().dispatch({ effects: StateEffect.appendConfig.of([popup, escape]) }));
  await settle();
  expect(view().dom.querySelector(".cm-tooltip")).not.toBeNull();
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(view().dom.querySelector(".cm-tooltip")).toBeNull();
  expect(view().state.selection.ranges).toHaveLength(3);
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(view().state.selection.ranges).toHaveLength(1);
});

// undo 기록은 문서의 허브(live-text.ts)에 있다 — 허브가 여러 선택을 받아야 undo · redo 가 커서 셋을 되돌린다.
it("undoes and redoes an edit made at three cursors in one step, cursors included", async () => {
  await mount();
  await open(ROWS, 1, 2);
  await act(async () =>
    view().dispatch({
      selection: EditorSelection.create([1, 3, 5].map((p) => EditorSelection.cursor(p))),
    }),
  );
  await act(async () =>
    view().dispatch(
      view().state.update(view().state.replaceSelection("x"), { userEvent: "input.type" }),
    ),
  );
  expect(view().state.doc.toString()).toBe("ax\nbx\ncx\n");
  await keydown({ code: "KeyZ", key: "z", metaKey: true }, view().contentDOM);
  expect(view().state.doc.toString()).toBe("a\nb\nc\n");
  expect(ranges()).toEqual([
    [1, 1],
    [3, 3],
    [5, 5],
  ]);
  await keydown({ code: "KeyZ", key: "z", metaKey: true, shiftKey: true }, view().contentDOM);
  expect(view().state.doc.toString()).toBe("ax\nbx\ncx\n");
  expect(ranges()).toEqual([
    [2, 2],
    [5, 5],
    [8, 8],
  ]);
});

it("selects the occurrences around the caret when ⌃⌘G stops at the cap, and says so", async () => {
  await mount();
  await open(MANY, 351, 1);
  await ctrl("KeyG", { metaKey: true });
  expect(view().state.selection.ranges).toHaveLength(MAX_OCCURRENCES);
  // 커서 자리(351행)의 일치가 주 범위다 — 타이핑이 보이는 자리에서 시작한다.
  expect(view().state.doc.lineAt(view().state.selection.main.from).number).toBe(351);
  expect(document.body.textContent).toContain(
    `일치가 ${MAX_OCCURRENCES}개를 넘어 커서 주변 ${MAX_OCCURRENCES}곳만 골랐다`,
  );
});

// 창이 포커스를 잃는 사이 ⌥ 의 뗌이 다른 앱으로 가면 더블 탭의 두 번째 누름이 남는다 — 창의 blur 가 그것을 버린다.
it("forgets a held ⌥⌥ when the window loses focus", async () => {
  await mount();
  await open(BLOCK, 1, 2);
  await holdAltTwice();
  await act(async () => window.dispatchEvent(new Event("blur")));
  await keydown({ code: "ArrowDown", key: "ArrowDown", altKey: true });
  expect(view().state.selection.ranges).toHaveLength(1);
});

it("pastes one clipboard line per cursor when the line count equals the cursor count", async () => {
  await mount();
  await open(ROWS, 1, 2);
  const three = EditorSelection.create([1, 3, 5].map((p) => EditorSelection.cursor(p)));
  const paste = async (text: string) =>
    act(async () => {
      const e = new Event("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(e, "clipboardData", { value: { getData: () => text } });
      view().contentDOM.dispatchEvent(e);
    });
  await act(async () => view().dispatch({ selection: three }));
  await paste("1\n2\n3");
  expect(view().state.doc.toString()).toBe("a1\nb2\nc3\n");
  // 줄 수가 다르면 모든 커서에 통째로 넣는다.
  await act(async () =>
    view().dispatch({
      selection: EditorSelection.create([2, 5, 8].map((p) => EditorSelection.cursor(p))),
    }),
  );
  await paste("x\ny");
  expect(view().state.doc.toString()).toBe("a1x\ny\nb2x\ny\nc3x\ny\n");
});

// 한글은 입력기가 조합한다 — CodeMirror 는 조합 중의 DOM 바뀜(주 커서 자리)을 읽어 다른 커서에도 같은 글자를 넣는다. 여기서는
// 입력기 대신 조합 이벤트와 글자 노드의 바뀜을 차례로 낸다(WebKit 의 한글 입력기가 내는 순서). 실제 입력기는 「사람이 확인할 것」.
it("types hangul through an IME composition at all three cursors", async () => {
  await mount();
  await open(ROWS, 1, 2);
  await act(async () =>
    view().dispatch({
      selection: EditorSelection.create([1, 3, 5].map((p) => EditorSelection.cursor(p))),
    }),
  );
  const content = view().contentDOM;
  const text = content.querySelector(".cm-line")!.firstChild as Text;
  expect(text.data).toBe("a");
  const compose = async (data: string) =>
    act(async () => {
      text.data = `a${data}`;
      document.getSelection()!.collapse(text, text.data.length);
      content.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data }));
      await new Promise((r) => setTimeout(r, 20));
    });
  await act(async () =>
    content.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" })),
  );
  await compose("ㄱ");
  await compose("가");
  await compose("간");
  await compose("가나");
  await act(async () => {
    content.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "가나" }));
    await new Promise((r) => setTimeout(r, 20));
  });
  await settle();
  expect(view().state.doc.toString()).toBe("a가나\nb가나\nc가나\n");
  expect(ranges()).toEqual([
    [3, 3],
    [7, 7],
    [11, 11],
  ]);
});

it("on Windows, adds cursors with Ctrl Ctrl held and ↑", async () => {
  useUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
  await mount();
  await open(BLOCK, 3, 2);
  await keydown({ code: "ControlLeft", key: "Control", ctrlKey: true });
  await keyup({ code: "ControlLeft", key: "Control" });
  await keydown({ code: "ControlLeft", key: "Control", ctrlKey: true });
  // 이 파일의 CodeMirror 는 macOS 로 불러와(머리 주석) Ctrl+↑ 를 mac 전용 「페이지 위로」 로 먼저 받는다. Windows 의 CodeMirror 에는
  // 그 키가 없으므로 ↑ 는 창(키맵 엔진)에만 보낸다 — 포커스는 그대로 에디터 글자 영역이다.
  for (let i = 0; i < 2; i++) {
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { code: "ArrowUp", key: "ArrowUp", ctrlKey: true }),
      );
    });
  }
  await keyup({ code: "ControlLeft", key: "Control" });
  expect(ranges()).toEqual([
    [1, 1],
    [8, 8],
    [11, 11],
  ]);
  expect(view().state.selection.mainIndex).toBe(0);
});
