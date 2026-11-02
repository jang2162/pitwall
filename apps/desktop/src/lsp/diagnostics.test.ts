// @vitest-environment happy-dom
// 에디터의 진단(15-m4-plan M4-06, ./diagnostics.ts)을 실제 CodeMirror 뷰와 가짜 창 서비스(./lsp-testing.ts)로 본다: 이벤트가 곧바로
// 장식이 든 트랜잭션이 되고(①, 가짜 시계 — 타이머 · 마이크로태스크를 돌리지 않는다), 지난 버전은 그리지 않고(②), 편집에 범위가
// 따라가고, F2 · ⇧F2 가 돌고(⑤), 색 말고도 모양으로 가르고(⑦), 호버 · 조합 · 목록 · 워크트리 규칙이 맞다.

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { LspDiagnostic } from "@pitwall/rpc-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { EditorFeatureContext } from "../editor/extensions";
import { editorContext } from "../editor/extensions/context";
import {
  diagnosticPopup,
  diagnosticsFeature,
  diagnosticsOf,
  gotoDiagnostic,
  HOVER_DELAY,
} from "./diagnostics";
import {
  type DiagnosticPopupData,
  diagnosticsUi,
  ENGLISH_STRINGS,
  popupData,
} from "./diagnostics-model";
import type { LspDocument } from "./lsp-sessions";
import { fakeLspServer } from "./lsp-testing";
import { currentPopup, popupBase } from "./popup";

const PATH = "/work/demo/src/lib.rs";

function diag(
  line: number,
  start: number,
  end: number,
  severity: LspDiagnostic["severity"] = "error",
  extra: Partial<LspDiagnostic> = {},
): LspDiagnostic {
  return {
    server: "rust-analyzer",
    origin: "push",
    version: null,
    range: { start: { line, character: start }, end: { line, character: end } },
    severity,
    message: `${severity} at ${line}:${start}`,
    code: null,
    source: null,
    codeHref: null,
    tags: [],
    related: [],
    data: null,
    ...extra,
  };
}

interface Setup {
  view: EditorView;
  doc: { version: number };
  fake: ReturnType<typeof fakeLspServer>;
  emit(diagnostics: LspDiagnostic[], extra?: { path?: string; worktree?: string | null }): void;
  opened: string[];
}

const views: EditorView[] = [];

function setup(text: string, options: { worktree?: string; list?: LspDiagnostic[] } = {}): Setup {
  const fake = fakeLspServer();
  const opened: string[] = [];
  fake.services.openExternal = (url) => void opened.push(url);
  if (options.list)
    fake.handle("lsp.diagnostics.list", async () => ({
      files: [{ path: PATH, diagnostics: options.list, truncated: false }],
      nextCursor: null,
      truncated: false,
    }));
  const state = { version: 0 };
  const doc = {
    projectId: "proj",
    path: PATH,
    ...(options.worktree !== undefined && { worktree: options.worktree }),
    language: "Rust",
    get version() {
      return state.version;
    },
    capabilities: {},
    onCapabilities: () => () => {},
    request: async () => undefined,
    call: async () => undefined,
    hold: () => () => {},
    services: fake.services,
  } as unknown as LspDocument;
  const context = { path: PATH, lsp: doc } as unknown as EditorFeatureContext;
  const view = new EditorView({
    state: EditorState.create({
      doc: text,
      extensions: [
        editorContext.of(context),
        popupBase,
        diagnosticsFeature.extension(context, doc),
      ],
    }),
    parent: document.body,
  });
  views.push(view);
  return {
    view,
    doc: state,
    fake,
    opened,
    emit(diagnostics, extra = {}) {
      fake.emit("lsp.diagnostics", {
        projectId: "proj",
        worktree: extra.worktree ?? null,
        path: extra.path ?? PATH,
        diagnostics,
        truncated: false,
      });
    },
  };
}

beforeEach(() => {
  diagnosticsUi.setState({ strings: ENGLISH_STRINGS });
});

afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  vi.useRealTimers();
});

const TEXT = 'fn a() {}\nlet x: u32 = "s";\nlet y = 1;\nfn z() {}\n';

describe("drawing", () => {
  it("draws a published diagnostic in the same turn — no queue or debounce (①, fake clock)", () => {
    vi.useFakeTimers();
    const { view, emit } = setup(TEXT);
    emit([diag(1, 13, 16)]);
    // 타이머도 마이크로태스크도 돌리지 않았다 — 이벤트를 받은 그 자리에서 장식이 든 트랜잭션이 났다.
    expect(diagnosticsOf(view).map((p) => [p.from, p.to])).toEqual([[23, 26]]);
    const mark = view.contentDOM.querySelector(".pw-diagnostic-error");
    expect(mark?.textContent).toBe('"s"');
  });

  it("does not draw diagnostics measured on an older document version (②)", () => {
    const { view, doc, emit } = setup(TEXT);
    doc.version = 1;
    emit([diag(1, 13, 16, "error", { version: 1, message: "v1" })]);
    expect(diagnosticsOf(view).map((p) => p.diagnostic.message)).toEqual(["v1"]);
    // 편집이 앞섰다(버전 2). 서버가 지난 텍스트(버전 1)로 낸 것은 그리지 않고 앞서 그린 것을 옮긴 채 둔다.
    view.dispatch({ changes: { from: 0, insert: "// x\n" } });
    doc.version = 2;
    emit([diag(0, 0, 2, "error", { version: 1, message: "stale" })]);
    expect(diagnosticsOf(view).map((p) => [p.diagnostic.message, p.from])).toEqual([["v1", 28]]);
    expect(view.contentDOM.textContent).not.toContain("stale");
    // 그 버전의 진단이 오면 바뀐다.
    emit([diag(0, 0, 2, "warning", { version: 2, message: "fresh" })]);
    expect(diagnosticsOf(view).map((p) => p.diagnostic.message)).toEqual(["fresh"]);
    // 버전이 없는 진단(열지 않은 파일 · 버전을 알리지 않는 서버)은 받은 대로 그린다.
    emit([diag(0, 0, 2, "hint", { message: "unversioned" })]);
    expect(diagnosticsOf(view).map((p) => p.diagnostic.message)).toEqual(["unversioned"]);
  });

  it("swaps one server's pulled diagnostics on each edit while its older pushed ones stay (rust-analyzer: pull + flycheck)", () => {
    const { view, doc, emit } = setup(TEXT);
    const pull = (v: number) =>
      diag(0, 0, 2, "error", { origin: "pull", version: v, message: `pull@${v}` });
    // flycheck(push)는 저장 때만 바뀐다 — 열 때(버전 0) 같은 버전의 pull 과 함께 온다.
    const flycheck = diag(2, 4, 5, "warning", {
      origin: "push",
      version: 0,
      message: "flycheck@0",
    });
    emit([pull(0), flycheck]);
    for (let v = 1; v <= 4; v += 1) {
      view.dispatch({ changes: { from: 0, insert: "x" } });
      doc.version = v;
      // Host 의 이벤트: 새 버전 pull + 아직 남은 옛 버전 flycheck(live 로 본 꼴 — M4-06 리뷰 H1).
      emit([pull(v), flycheck]);
    }
    expect(diagnosticsOf(view).map((p) => p.diagnostic.message)).toEqual(["pull@4", "flycheck@0"]);
    expect(view.contentDOM.querySelectorAll(".pw-diagnostic")).toHaveLength(2);
    // 새 flycheck 가 오면 그 묶음만 바뀐다.
    emit([pull(4), { ...flycheck, version: 4, message: "flycheck@4" }]);
    expect(diagnosticsOf(view).map((p) => p.diagnostic.message)).toEqual(["pull@4", "flycheck@4"]);
  });

  it("moves drawn ranges with edits until the next diagnostics arrive", () => {
    const { view, emit } = setup(TEXT);
    emit([diag(1, 13, 16)]);
    view.dispatch({ changes: { from: 10, insert: "    " } });
    expect(diagnosticsOf(view).map((p) => [p.from, p.to])).toEqual([[27, 30]]);
    expect(view.contentDOM.querySelector(".pw-diagnostic-error")?.textContent).toBe('"s"');
    // 범위가 지워지면 빈 자리로 남고 그 줄의 한 글자에 긋는다.
    view.dispatch({ changes: { from: 27, to: 30 } });
    expect(diagnosticsOf(view).map((p) => [p.from, p.to])).toEqual([[27, 27]]);
  });

  it("marks each severity with its own gutter shape and underline, named for screen readers, not by colour alone (⑦)", () => {
    const { view, emit } = setup(TEXT);
    emit([
      diag(0, 0, 2, "error"),
      diag(1, 0, 3, "warning"),
      diag(2, 0, 3, "information"),
      diag(3, 0, 2, "hint", { tags: ["unnecessary"] }),
    ]);
    const markers = [...view.dom.querySelectorAll<HTMLElement>(".pw-diagnostic-marker")].map(
      (m) => [
        m.dataset.severity,
        m.textContent,
        m.getAttribute("role"),
        m.getAttribute("aria-label"),
      ],
    );
    expect(markers).toEqual([
      ["error", "✖", "img", "Error"],
      ["warning", "▲", "img", "Warning"],
      ["information", "●", "img", "Information"],
      ["hint", "◇", "img", "Hint"],
    ]);
    const underline = (severity: string) =>
      view.contentDOM.querySelector(`[data-diagnostic="${severity}"]`)?.className;
    expect(underline("error")).toContain("pw-diagnostic-error");
    expect(underline("hint")).toContain("pw-diagnostic-unnecessary");
    // 밑줄의 모양이 심각도마다 다르다(물결 · 점선 · 파선 — 색만이 아니다).
    const css = [
      ...[...document.querySelectorAll("style")].map((s) => s.textContent ?? ""),
      ...(document.adoptedStyleSheets ?? []).flatMap((sheet) =>
        [...sheet.cssRules].map((r) => r.cssText),
      ),
    ].join("\n");
    expect(css).toMatch(/pw-diagnostic-error[^}]*wavy/);
    expect(css).toMatch(/pw-diagnostic-information[^}]*dotted/);
    expect(css).toMatch(/pw-diagnostic-hint[^}]*dashed/);
    // 한 줄에 여럿이면 가장 심한 것의 표지에 수를 단다.
    emit([diag(1, 0, 3, "hint"), diag(1, 4, 5, "warning"), diag(1, 6, 7, "warning")]);
    const one = view.dom.querySelector<HTMLElement>(".pw-diagnostic-marker")!;
    expect([one.dataset.severity, one.title]).toEqual(["warning", "Warning (3)"]);
  });
});

describe("receiving", () => {
  it("reads the current diagnostics when the document opens, unless an event came first", async () => {
    const listed = setup(TEXT, { list: [diag(1, 13, 16, "warning", { message: "listed" })] });
    await vi.waitFor(() => expect(diagnosticsOf(listed.view)).toHaveLength(1));
    expect(listed.fake.called("call:lsp.diagnostics.list")).toEqual([
      { projectId: "proj", worktree: null, path: PATH, limit: 1000 },
    ]);
    const raced = setup(TEXT, { list: [diag(1, 13, 16, "warning", { message: "listed" })] });
    raced.emit([diag(0, 0, 2, "error", { message: "event" })]);
    await new Promise((r) => setTimeout(r, 0));
    expect(diagnosticsOf(raced.view).map((p) => p.diagnostic.message)).toEqual(["event"]);
  });

  it("listens only to its own path and worktree, and stays silent when the Host has no diagnostics", async () => {
    const { view, emit, fake } = setup(TEXT, { worktree: "/work/demo-wt/" });
    // 목록 처리기가 없다(미신뢰 · 진단 없는 Host) — 오류 없이 빈다.
    await new Promise((r) => setTimeout(r, 0));
    expect(fake.called("call:lsp.diagnostics.list")).toHaveLength(1);
    emit([diag(0, 0, 2)], { path: "/work/demo/src/other.rs", worktree: "/work/demo-wt" });
    emit([diag(0, 0, 2)], { worktree: null });
    expect(diagnosticsOf(view)).toEqual([]);
    emit([diag(0, 0, 2)], { worktree: "/work/demo-wt" });
    expect(diagnosticsOf(view)).toHaveLength(1);
  });

  it("holds diagnostics during Korean IME composition and draws them once it ends", async () => {
    const { view, emit } = setup(TEXT);
    view.contentDOM.dispatchEvent(new CompositionEvent("compositionstart"));
    expect(view.compositionStarted).toBe(true);
    emit([diag(1, 13, 16)]);
    expect(diagnosticsOf(view)).toEqual([]);
    view.contentDOM.dispatchEvent(new CompositionEvent("compositionend"));
    await vi.waitFor(() => expect(view.compositionStarted).toBe(false));
    // 조합이 끝난 뒤의 첫 업데이트에서 놓는다.
    view.dispatch({ selection: { anchor: 1 } });
    await vi.waitFor(() => expect(diagnosticsOf(view)).toHaveLength(1));
  });
});

describe("moving and popups", () => {
  const three = () => [
    diag(0, 3, 4, "error", { message: "first" }),
    diag(1, 4, 5, "warning", { message: "a warning" }),
    diag(2, 4, 5, "error", { message: "second" }),
    diag(3, 3, 4, "error", { message: "third" }),
  ];

  it("goes to the next error with F2 and wraps from the last to the first; ⇧F2 goes back (⑤)", () => {
    const { view, emit } = setup(TEXT);
    emit(three());
    const at = () => view.state.selection.main.head;
    const popupMessages = () =>
      (currentPopup(view, "main")!.data as DiagnosticPopupData).diagnostics.map((d) => d.message);
    expect(gotoDiagnostic(view, true)).toBe(true);
    expect(at()).toBe(3);
    expect(popupMessages()).toEqual(["first"]);
    gotoDiagnostic(view, true);
    expect(popupMessages()).toEqual(["second"]);
    gotoDiagnostic(view, true);
    expect(popupMessages()).toEqual(["third"]);
    gotoDiagnostic(view, true);
    expect(popupMessages()).toEqual(["first"]);
    gotoDiagnostic(view, false);
    expect(popupMessages()).toEqual(["third"]);
    // 오류가 없으면 다른 진단을 돈다.
    emit([diag(1, 4, 5, "warning", { message: "only warning" })]);
    gotoDiagnostic(view, true);
    expect(popupMessages()).toEqual(["only warning"]);
    emit([]);
    expect(gotoDiagnostic(view, true)).toBe(false);
  });

  it("carries the LSP diagnostics and their servers for quick fixes in the popup data", () => {
    const { view, emit } = setup(TEXT);
    emit([
      diag(1, 13, 16, "error", {
        server: "rust-analyzer",
        code: "E0308",
        source: "rustc",
        data: { rendered: "x" },
        tags: ["deprecated"],
      }),
      diag(1, 13, 16, "error", { server: "rust-analyzer", source: "rust-analyzer" }),
    ]);
    const popup = diagnosticPopup(diagnosticsOf(view));
    const data = popup.data as DiagnosticPopupData;
    expect(popup.kind).toBe("diagnostic");
    expect(data.servers).toEqual(["rust-analyzer", "rust-analyzer"]);
    expect(data.lsp[0]).toEqual({
      range: { start: { line: 1, character: 13 }, end: { line: 1, character: 16 } },
      severity: 1,
      message: "error at 1:13",
      code: "E0308",
      source: "rustc",
      tags: [2],
      data: { rendered: "x" },
    });
    // 모두 버전이 없으면(진단에 버전을 싣지 않는 서버) 버전 모름 — `null` 로 싣고 Host 가 통과시킨다(M4-06 재리뷰 N1).
    expect(data.version).toBeNull();
    // 버전이 있는 것이 모두 같은 버전이면 그 버전 — 범위가 맞는 문서 버전이다(버전 없는 것은 그 판정에서 빠진다). 서로 다르면 싣지
    // 않는다(M4-12 의 「빠른 수정」 은 그때 진단을 넘기지 않는다).
    const at = (version: number | null) =>
      diag(1, 13, 16, "error", { origin: "pull", version, message: `v${version}` });
    expect(popupData([at(3), at(3)]).version).toBe(3);
    expect(popupData([at(3), at(null)]).version).toBe(3);
    expect(popupData([at(3), at(2)]).version).toBeUndefined();
    expect(popupData([at(3), at(2), at(null)]).version).toBeUndefined();
  });

  it("opens the diagnostic popup after the pointer rests on a range, with the message, server, source and a web link", () => {
    vi.useFakeTimers();
    const { view, emit, opened } = setup(TEXT);
    emit([
      diag(1, 13, 16, "error", {
        message: "mismatched types\nexpected `u32`",
        code: "E0308",
        source: "rustc",
        codeHref: "https://doc.rust-lang.org/error_codes/E0308.html",
        related: [
          {
            path: "/work/demo/src/lib.rs",
            uri: "file:///work/demo/src/lib.rs",
            range: { start: { line: 1, character: 7 }, end: { line: 1, character: 10 } },
            message: "expected due to this",
          },
        ],
      }),
    ]);
    // happy-dom 은 배치가 없다 — 마우스 자리를 문서 위치로 돌려준다.
    let pos: number | null = 24;
    view.posAtCoords = (() => pos) as typeof view.posAtCoords;
    view.contentDOM.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
    vi.advanceTimersByTime(HOVER_DELAY - 1);
    expect(currentPopup(view, "main")).toBeNull();
    vi.advanceTimersByTime(1);
    expect(currentPopup(view, "main")?.kind).toBe("diagnostic");
    const dom = view.dom.querySelector(".pw-lsp-popup")!;
    expect(dom.querySelector(".pw-diagnostic-message")?.textContent).toBe(
      "mismatched types\nexpected `u32`",
    );
    expect(dom.querySelector(".pw-diagnostic-meta")?.textContent).toContain(
      "rust-analyzer · rustc E0308",
    );
    expect(dom.querySelector(".pw-diagnostic-related")?.textContent).toBe(
      "/work/demo/src/lib.rs:2:8 expected due to this",
    );
    dom.querySelector<HTMLElement>(".pw-diagnostic-docs")!.click();
    expect(opened).toEqual(["https://doc.rust-lang.org/error_codes/E0308.html"]);
    // 진단 밖으로 나간 마우스는 새 팝업을 기다리지 않는다(열린 것은 둔다).
    pos = 2;
    view.contentDOM.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
    vi.advanceTimersByTime(HOVER_DELAY);
    expect(currentPopup(view, "main")?.kind).toBe("diagnostic");
    view.contentDOM.dispatchEvent(new MouseEvent("mouseleave", { bubbles: false }));
    expect(currentPopup(view, "main")).toBeNull();
  });
});
