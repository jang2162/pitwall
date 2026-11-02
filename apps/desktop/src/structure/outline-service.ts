// 창 하나의 아웃라인(15-m4-plan M4-10): 활성 에디터 탭(`workspace.activeTab()` — 툴 윈도우를 보는 중이면 마지막으로 쓴 탭)의
// 심볼 트리를 읽어 둔다. 구조 툴 윈도우와 파일 구조 팝업(⌘F12)이 함께 보고, 보는 쪽이 하나라도 있는 동안만 돈다(`retain`).
//
// 출처 고르기(메인 판정 2026-11-02 — 대체는 웹뷰가 한다): 탭의 LSP 문서(`EditorFeatureContext.lsp`)의 능력 칸에 `document_symbol` 이
// 있으면 언어 서버(`lsp.symbols.document` — 요청 도우미 `doc.call` 로, 같은 종류의 앞 요청 취소 · 낡은 답 버림), 없으면(서버 없음 ·
// 준비 전 · 미신뢰 · LSP 에 열리지 않은 문서) 심볼 인덱스(`index.file.symbols` — 저장하지 않은 내용을 `text` 로 싣는다). 서버
// 요청이 실패하거나 「맡을 서버 없음」(`5001`)이면 인덱스로 대신한다. 능력 칸이 바뀌면(서버가 뜸 · 멈춤) 다시 읽는다 — 그래서
// 서버가 준비되면 LSP 로, 멈추면 인덱스로 바뀌고 출처 표지도 따라간다.
//
// 다시 읽는 때: 탭이 바뀌면 곧바로, 편집 뒤 300 ms 멈추면(`OUTLINE_REREAD_MS`), 문서를 다시 읽으면(`reset`) 곧바로. 낡은 답(그사이
// 문서가 바뀜 · 다른 탭으로 감)은 버린다. 커서는 탭의 주 커서 자리(UTF-16 오프셋)를 둔다 — 커서 따라가기가 본다.
//
// 이 모듈은 공유 부품(lsp-sessions.ts · 창 배선)을 고치지 않는다: LSP 문서는 뷰의 context facet 으로 읽고, 창 단위의 Host 호출은
// 작업 공간의 LSP 세션(`workspace.lsp.services`)으로 한다. 파일 창(LSP 세션 없음 — projectId 가 없다)에는 아웃라인이 없다.

import type { Text } from "@codemirror/state";
import { ERROR_CODE } from "@pitwall/rpc-client";
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { createStore, type StoreApi } from "zustand/vanilla";

import { errorCode } from "../editor/doc-sync";
import { isFileTab, tabParams } from "../editor/editor-tabs";
import type { EditorWorkspace } from "../editor/editor-workspace";
import { editorContext } from "../editor/extensions/context";
import { LSP_ERROR, type LspDocument } from "../lsp/lsp-sessions";
import { fromIndex, fromLsp, type Outline } from "./outline";

/** 언어 서버 요청이 「맡을 서버 없음」 으로 끝났다(인덱스로 대신한다). */
const NO_SERVER = { noServer: true } as const;

/** 편집 뒤 이만큼(ms) 멈추면 다시 읽는다(계획 행). */
export const OUTLINE_REREAD_MS = 300;

/**
 * 인덱스가 읽는 텍스트의 상한(UTF-8 바이트, crates/pw-index/src/lib.rs `MAX_FILE_BYTES`). 넘는 텍스트에는 빈 답이 오므로 보내지
 * 않는다 — 큰 파일을 편집이 멈출 때마다 통째로 보내고 「심볼이 없다」를 보이지 않게(M4-10 리뷰 L1).
 */
export const INDEX_MAX_TEXT_BYTES = 1024 * 1024;

/** `text` 의 UTF-8 바이트 수가 `limit` 를 넘는가. UTF-16 단위 하나는 1 ~ 3 바이트라 대부분 세지 않고 정한다. */
export function exceedsUtf8(text: Text, limit: number): boolean {
  if (text.length > limit) return true;
  if (text.length * 3 <= limit) return false;
  let bytes = text.lines - 1; // 줄바꿈
  for (const chunk of text.iterLines()) {
    for (let i = 0; i < chunk.length; i++) {
      const c = chunk.charCodeAt(i);
      // 대리쌍은 둘이 합쳐 4 바이트 — 하나에 2 씩.
      bytes += c < 0x80 ? 1 : c < 0x800 || (c >= 0xd800 && c < 0xe000) ? 2 : 3;
    }
    if (bytes > limit) return true;
  }
  return false;
}

export type OutlineStatus =
  /** 보일 것이 없다 — 에디터 탭이 없거나 파일 탭이 아니거나 파일 창이다. */
  | "none"
  /** 처음 읽는 중(앞의 트리가 없다). */
  | "loading"
  /** 트리가 있다(다시 읽는 중이어도 앞의 것을 보인다). */
  | "ready"
  /** 두 출처 모두 실패했다. */
  | "failed"
  /** 언어 서버가 답하지 않고 파일이 인덱스의 상한(`INDEX_MAX_TEXT_BYTES`)보다 커서 인덱스로도 읽지 않았다. */
  | "tooLarge";

export interface OutlineState {
  readonly status: OutlineStatus;
  readonly panelId?: string;
  readonly path?: string;
  readonly worktree?: string;
  readonly outline?: Outline;
  /** 탭의 주 커서(UTF-16 오프셋). */
  readonly cursor?: number;
}

export interface OutlineService {
  readonly store: StoreApi<OutlineState>;
  /**
   * 보기 시작한다. 돌려준 함수로 그만 본다 — 마지막이 그만 보면 구독을 다 푼다. `api` 는 탭 바뀜을 들을 도킹 영역이다 — 툴
   * 윈도우는 작업 공간이 도킹 영역에 붙기 전(레이아웃 복원)에 마운트될 수 있어 자기 `containerApi` 를 넘긴다.
   */
  retain(api?: DockviewApi): () => void;
  /** 지금 탭의 뷰(고른 심볼로 가기 전 커서 확인 등). */
  view(): ReturnType<EditorWorkspace["view"]>;
}

const services = new WeakMap<EditorWorkspace, OutlineService>();

/** 작업 공간(창)마다 하나. */
export function outlineService(workspace: EditorWorkspace): OutlineService {
  let service = services.get(workspace);
  if (!service) services.set(workspace, (service = createOutlineService(workspace)));
  return service;
}

function createOutlineService(workspace: EditorWorkspace): OutlineService {
  const store = createStore<OutlineState>(() => ({ status: "none" }));
  let users = 0;
  /** 지금 보는 탭에 묶인 구독(문서 바뀜 · 능력 칸). */
  let bound: (() => void)[] = [];
  /** 창에 묶인 구독(활성 탭 · 커서). */
  let watching: (() => void)[] = [];
  let boundLsp: LspDocument | undefined;
  let offCapabilities: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  /** 읽기마다 오른다 — 늦게 온 앞 답을 버린다. */
  let seq = 0;
  let inflight: AbortController | undefined;

  const panel = (): IDockviewPanel | undefined => {
    const id = store.getState().panelId;
    return id === undefined ? undefined : workspace.api()?.getPanel(id);
  };
  const view = () => {
    const id = store.getState().panelId;
    return id === undefined ? undefined : workspace.view(id);
  };

  function unbind() {
    clearTimeout(timer);
    timer = undefined;
    inflight?.abort();
    inflight = undefined;
    seq++;
    for (const off of bound) off();
    bound = [];
    offCapabilities?.();
    offCapabilities = undefined;
    boundLsp = undefined;
  }

  /** 활성 탭을 다시 본다. 같은 탭 · 같은 경로면 그대로. */
  function pick() {
    const tab = workspace.activeTab();
    const now = store.getState();
    const params = tab && isFileTab(tab) ? tabParams(tab) : undefined;
    if (tab?.id === now.panelId && params?.path === now.path) return;
    unbind();
    if (!tab || !params || !workspace.lsp) {
      store.setState({ status: "none" }, true);
      return;
    }
    store.setState(
      {
        status: "loading",
        panelId: tab.id,
        path: params.path,
        ...(params.worktree !== undefined && { worktree: params.worktree }),
      },
      true,
    );
    // 문서를 (다시) 읽으면 곧바로, 편집이면 멈춘 뒤 다시 읽는다. 구독하면 읽은 문서는 곧바로 `reset` 을 준다.
    bound.push(
      workspace.documents.onChange(params.path, (change) =>
        schedule(change.kind === "edit" ? OUTLINE_REREAD_MS : 0),
      ),
    );
    trackCursor();
    schedule(0);
  }

  function schedule(delay: number) {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void load();
    }, delay);
  }

  /** 뷰의 LSP 문서가 바뀌었으면(뷰가 늦게 생김 · 다시 엶) 능력 칸 구독을 옮긴다. */
  function bindLsp(doc: LspDocument | undefined) {
    if (doc === boundLsp) return;
    offCapabilities?.();
    boundLsp = doc;
    // 서버가 뜨거나 멈추면 칸이 바뀐다 — 출처를 다시 고른다.
    offCapabilities = doc?.onCapabilities(() => schedule(0));
  }

  async function load() {
    const id = ++seq;
    inflight?.abort();
    const controller = new AbortController();
    inflight = controller;
    const state = store.getState();
    const v = view();
    const sessions = workspace.lsp;
    if (!v || state.path === undefined || !sessions) return; // 뷰가 생기면 커서 보고가 다시 부른다.
    const text = v.state.doc;
    const doc = v.state.facet(editorContext)?.lsp;
    bindLsp(doc);
    const current = () => id === seq && view()?.state.doc === text;
    /** 첫 읽기의 서버 답이 낡았다 — 인덱스 트리를 그린 뒤 서버에 한 번 더 묻는다. */
    let askAgain = false;
    if (doc?.capabilities.document_symbol) {
      try {
        // 요청 도우미(`doc.request` — `doc.call` 과 같은 규칙: 편집이 Host 에 닿은 뒤 묻고, 같은 종류의 앞 요청을 취소하고,
        // `staleVersion` · 그사이 바뀐 답은 `undefined`). 「맡을 서버 없음」(5001)은 도우미가 `undefined` 로 삼키므로 여기서 따로
        // 받는다 — 낡은 답(앞 트리를 그대로 두고 다음 읽기를 기다린다)과 서버 없음(인덱스로)을 가른다.
        const answer = await doc.request("outline", async (at, signal) => {
          try {
            return await doc.services.call(
              "lsp.symbols.document",
              { docId: at.docId, version: at.version },
              { signal },
            );
          } catch (e) {
            if (errorCode(e) === LSP_ERROR.NO_SERVER) return NO_SERVER;
            throw e;
          }
        });
        if (id !== seq) return;
        if (answer === undefined) {
          // 낡은 답: 앞 트리가 있으면 그대로 두고 다음 읽기(편집 · 능력 칸)를 기다린다. 첫 읽기면 「읽는 중」 에 머물지 않게
          // 이번은 인덱스로 대신하고 조금 뒤 서버에 한 번 더 묻는다(그때 낡으면 인덱스 트리가 남는다, M4-10 리뷰 L2).
          if (store.getState().outline) return;
          askAgain = true;
        } else if (!("noServer" in answer)) return finish(fromLsp(answer));
      } catch (e) {
        if (id !== seq) return;
        // 서버 오류 · 시한 · 문서를 잃음 — 인덱스로 대신한다.
        if (errorCode(e) === ERROR_CODE.CANCELLED) return;
      }
    }
    if (exceedsUtf8(text, INDEX_MAX_TEXT_BYTES)) {
      if (current()) store.setState({ status: "tooLarge", outline: undefined });
      return;
    }
    try {
      const answer = await sessions.services.call(
        "index.file.symbols",
        {
          projectId: sessions.projectId,
          ...(state.worktree !== undefined && { worktree: state.worktree }),
          path: state.path,
          text: text.toString(),
        },
        { signal: controller.signal },
      );
      if (!current()) return;
      finish(fromIndex(answer, text));
      if (askAgain) schedule(OUTLINE_REREAD_MS);
    } catch {
      if (!current()) return;
      if (!store.getState().outline) store.setState({ status: "failed" });
    }
  }

  function finish(outline: Outline) {
    store.setState({ status: "ready", outline });
  }

  function cursorOf(): number | undefined {
    return view()?.state.selection.main.head;
  }

  function trackCursor() {
    const cursor = cursorOf();
    if (cursor !== store.getState().cursor) store.setState({ cursor });
  }

  /** 첫 그림 전에 뷰가 없던 탭: 뷰가 생기면(커서 보고) 읽는다. */
  function viewAppeared() {
    const state = store.getState();
    if (state.status === "loading" && timer === undefined) schedule(0);
  }

  function start(given: DockviewApi | undefined) {
    const api = given ?? workspace.api();
    const subscriptions: { dispose(): void }[] = [];
    if (api) {
      subscriptions.push(
        api.onDidActivePanelChange(() => pick()),
        api.onDidRemovePanel(() => pick()),
        api.onDidAddPanel(() => pick()),
      );
    }
    watching = [
      ...subscriptions.map((s) => () => s.dispose()),
      workspace.onCursor((panelId) => {
        if (panelId !== store.getState().panelId) return;
        trackCursor();
        viewAppeared();
      }),
    ];
    pick();
  }

  function stop() {
    unbind();
    for (const off of watching) off();
    watching = [];
    store.setState({ status: "none" }, true);
  }

  return {
    store,
    retain(api) {
      if (users++ === 0) start(api);
      let released = false;
      return () => {
        if (released) return;
        released = true;
        if (--users === 0) stop();
      };
    },
    view: () => (panel() ? view() : undefined),
  };
}

/** 고른 심볼의 이름 자리(줄 · 열 — 1부터, 열은 글자 단위)를 `text` 에서 구한다. */
export function cursorAt(text: Text, offset: number): { line: number; column: number } {
  const at = text.lineAt(Math.min(offset, text.length));
  return { line: at.number, column: offset - at.from + 1 };
}
