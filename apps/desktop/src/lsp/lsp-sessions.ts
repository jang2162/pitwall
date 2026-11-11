// LSP 세션(15-m4-plan M4-07 ① · ②): 열린 문서 하나(와 그 언어)마다 Host 의 서버 관리자(crates/pw-lsp, M4-05)에 문서 하나를 연다
// (`lsp.document.*`). 버전 · 대기열 · 다시 열기 · 물러남 · 닫기 미루기는 문서 동기화 공용 부품(src/editor/doc-sync.ts)이 한다 — 구문
// 세션과 같은 길이다(결정 M4-D10). 탭(뷰어)이 문서를 보이는 동안 `acquire` 로 잡고 — 같은 파일을 탭 여럿이 보여도 세션은 하나다 —
// 마지막 탭이 놓으면 닫는다. 잡은 것(`LspDocument`)이 에디터 기능의 context(`EditorFeatureContext.lsp`)로 간다.
//
// 열지 않는 문서: 편집 등급 밖(5 MiB 초과 · 1만 자 줄 — limits.ts, 결정 M1-D9 — 편집으로 벗어나면 Host 가 `off` 로 끈다), 서버 정의가
// 없는 언어(`off: noServer`), 그 창이 열 수 없는 경로(루트 밖 · 다른 창 — invalid params · permission denied, 다시 묻지 않는다). 어떤
// 탭의 문서를 여는지는 `lspEligible`(파일 탭의 편집 등급 텍스트 문서만 — git · DB 문서 · 루트 밖 읽기 전용 문서는 열지 않는다).
//
// 버전: 웹뷰 버전은 창마다 따로다. 같은 파일을 다른 창도 열면 Host 가 참조 수로 묶고 서버 쪽 버전을 따로 올린다(M4-05) — 이 창은
// 자기 버전만 보내고, 다른 창이 그사이 서버의 텍스트를 바꿨으면 Host 가 답에 `staleVersion` 을 단다(요청 도우미가 버린다).
//
// 요청 도우미(`request`): 기능 행이 자기 RPC 를 `(docId, version)` 과 함께 부르게 한다. 위치는 CodeMirror 의 UTF-16 오프셋 그대로
// 넘긴다(Host 가 LSP 위치로 바꾼다). 같은 종류(`kind`)의 새 요청이 오면 옛 요청을 취소하고(`AbortSignal` — RPC 의 `request.cancel`
// → 서버의 `$/cancelRequest`), 답이 `staleVersion` 이거나 그사이 이 문서의 버전 · 문서 id 가 바뀌었으면 버린다(`undefined`). 한글
// 조합 중(쥐고 있는 바뀜이 있으면)에는 묻지 않는다 — 서버의 텍스트가 화면보다 뒤다.
//
// 한글 IME 조합(⑥): 조합 중의 바뀜은 보내지 않고 조합이 끝난 뒤 한 번 보낸다. 조합은 뷰(CodeMirror)가 알므로 LSP 기능 칸
// (src/editor/extensions/lsp.ts)이 조합 트랜잭션을 보면 `hold` 로 쥐고 조합이 끝나면 놓는다.
//
// 능력 칸(`capabilities`): 기능마다 그 기능을 맡을 서버와 그 서버가 알린 능력(`lsp.document.capabilities` — Host 가 요청 길과 같은
// 규칙으로 고른다, 결정 M4-D12). 문서가 열리면 묻고, 창이 `lsp.status` 이벤트를 받으면(`statusChanged` — 서버가 뜸 · 끝남 · 동적
// 등록) 다시 묻는다. 묻는 중에 또 오면 끝난 뒤 한 번 더 묻는다(진행 알림이 잦아도 문서마다 하나만 돈다).
//
// 저장: 문서 모델이 `fs.write` 를 마치면(`saving` 이 끝나며 디스크 `version` 이 바뀜) `lsp.document.save` 를 보낸다(rust-analyzer 의
// 저장 시 검사 등).

import type { Text } from "@codemirror/state";
import {
  ERROR_CODE,
  type EventMessage,
  type HighlightSpan,
  type LspDocumentCapabilities,
  type LspDocumentChangeParams,
  type LspDocumentChangeResult,
  type LspDocumentOpened,
  type LspDocumentOpenParams,
  type LspDocumentRef,
  type LspFeature,
  type LspFeatureCapability,
  type UnaryMethods,
} from "@pitwall/rpc-client";

import { DocSync, DocSyncPool, errorCode } from "../editor/doc-sync";
import type { DocumentState, DocumentStore } from "../editor/documents";
import { PLAIN_TEXT } from "../editor/language";
import { overHighlightLimit } from "../editor/limits";

/**
 * 서버 관리자의 오류 코드(crates/pw-lsp/src/manager.rs 의 `code` — lsp-sessions.test.ts 가 그 파일을 읽어 값이 같은지 본다).
 */
export const LSP_ERROR = {
  /** 그 기능을 맡을 떠 있는 서버가 없다(능력 없음 · 끔 · 아직 뜨지 않음). */
  NO_SERVER: 5001,
  /** 서버가 오류로 답했다. */
  SERVER_ERROR: 5002,
  /** 서버가 시한 안에 답하지 않았거나 끝났다. */
  SERVER_UNAVAILABLE: 5003,
} as const;

/** 서버 인스턴스 상태 이벤트의 토픽(crates/pw-lsp/src/methods.rs 의 `TOPIC_STATUS`). 프로젝트 범위다. */
export const LSP_STATUS_TOPIC = "lsp.status";

/** 서버 관리자의 문서 RPC. 창은 Host 연결을 넘기고 시험은 가짜를 넘긴다. */
export interface LspSource {
  open(params: LspDocumentOpenParams): Promise<LspDocumentOpened>;
  change(params: LspDocumentChangeParams): Promise<LspDocumentChangeResult>;
  save(params: LspDocumentRef): Promise<unknown>;
  close(params: LspDocumentRef): Promise<unknown>;
  capabilities(params: LspDocumentRef): Promise<LspDocumentCapabilities>;
}

/** 팝업 바탕(src/lsp/popup.ts)이 쓰는 창의 것. */
/** 이 창의 Host 연결로 부르는 RPC — 생성된 메서드 표(`UnaryMethods`)로 타입이 붙는다. */
export type LspCall = <K extends keyof UnaryMethods & string>(
  method: K,
  params: UnaryMethods[K]["params"],
  options?: { signal?: AbortSignal },
) => Promise<UnaryMethods[K]["result"]>;

/**
 * 창의 것: 기능 행이 자기 Host RPC 를 부르고(`call` — `AbortSignal` 은 RPC 의 `request.cancel` 이 된다) 프로젝트 범위 이벤트를 듣는다
 * (`subscribe` — 이 창의 프로젝트로 구독한다, 리뷰 H-2). 팝업 바탕(src/lsp/popup.ts)은 링크 · 코드 강조를 쓴다.
 */
export interface LspServices {
  call: LspCall;
  /** 이 창의 프로젝트의 이벤트 `topics` 를 듣는다. 해제 함수를 돌려준다. */
  subscribe(topics: readonly string[], listener: (event: EventMessage) => void): () => void;
  /** 링크를 외부 브라우저로 연다(`http(s)` · `mailto` 만 — 팝업이 고른다). 없으면 링크를 누를 수 없다. */
  openExternal?(url: string): void;
  /** 팝업 액션 줄의 접근성 이름(창의 언어). */
  popupActionsLabel?(): string;
  /** 코드 조각(`language` 는 Linguist 이름)의 구문 강조 구간. 없거나 실패면 칠하지 않는다. */
  highlight?(language: string, code: string): Promise<readonly HighlightSpan[]>;
}

/** 문서의 능력 칸: 기능 → 그 기능을 맡을 서버와 그 능력. 맡을 서버가 없는 기능은 없다. */
export type LspCapabilities = Readonly<Partial<Record<LspFeature, LspFeatureCapability>>>;

/**
 * 요청을 계산한 문서. 기능 행은 이것을 자기 RPC 에 싣는다 — `docId` · `version`(Host 의 `expected_version`)은 문서 요청에,
 * `projectId` · `worktree` · `path` 는 프로젝트 범위 조회(`lsp.diagnostics.list` 등)에.
 */
export interface LspAt {
  readonly docId: string;
  readonly version: number;
  readonly projectId: string;
  /** 탭의 워크트리 꼭대기(`ActiveWorktree.top`). 메인 체크아웃이면 없다. */
  readonly worktree?: string;
  readonly path: string;
}

/** LSP 기능이 보는 문서(`EditorFeatureContext.lsp`). 탭이 잡고 있는 동안 산다. */
export interface LspDocument {
  readonly projectId: string;
  /** 탭의 워크트리 꼭대기. 메인 체크아웃이면 없다. */
  readonly worktree?: string;
  readonly path: string;
  /** Linguist 언어 이름. */
  readonly language: string;
  /** 지금 버전(서버에 보냈거나 보낼 마지막 편집). 조합 중에 쥔 바뀜은 아직 들지 않는다. */
  readonly version: number;
  /** 지금 능력 칸. */
  readonly capabilities: LspCapabilities;
  /** 능력 칸이 바뀌면 부른다. 해제 함수를 돌려준다. */
  onCapabilities(listener: () => void): () => void;
  /**
   * 능력 칸을 묻는 중이면 그 답(묻는 중에 온 다시 묻기까지)이 칸에 든 뒤, 아니면 곧바로 풀린다. 칸이 빈 채 친 트리거 글자가
   * `lsp.status`(준비) 직후의 묻는 틈에 걸렸는지 가른다 — 상태바는 같은 이벤트에서 따로 물어 칸보다 먼저 「준비」 를 그릴 수 있다.
   */
  capabilitiesSettled(): Promise<void>;
  /**
   * 같은 종류(`kind`)의 앞 요청을 취소하고, 그 버전의 편집이 Host 에 닿은 뒤 `call` 을 부른다. 답이 `staleVersion: true` 를
   * 실었거나 그사이 문서가 바뀌었거나 취소됐거나 맡을 서버가 없으면(`5001`) `undefined`. 그 밖의 실패는 그대로 던진다(문서를
   * 잃었으면 — not found — 다시 연다). 물러서 있는 동안은 다시 열 때까지(최대 30 s) 기다린다 — 답을 받아 팝업을 열기 전에 커서 ·
   * 마우스가 아직 그 자리인지 본다.
   */
  request<T>(
    kind: string,
    call: (at: LspAt, signal: AbortSignal) => Promise<T>,
  ): Promise<T | undefined>;
  /**
   * 기능 행의 Host RPC 하나를 요청 도우미로 부른다: `params` 가 문서(`at`)로 매개변수를 만들고, 같은 `kind` 의 앞 요청은 취소된다.
   * 답이 `staleVersion: true` 를 실었거나(문서 요청의 답) 그사이 문서가 바뀌었으면 `undefined` — 규칙은 `request` 와 같다.
   */
  call<K extends keyof UnaryMethods & string>(
    kind: string,
    method: K,
    params: (at: LspAt) => UnaryMethods[K]["params"],
  ): Promise<UnaryMethods[K]["result"] | undefined>;
  /** 바뀜을 쥔다(IME 조합 중). 돌려준 함수로 놓는다 — 마지막이 놓으면 모은 바뀜을 한 번에 보낸다. */
  hold(): () => void;
  readonly services: LspServices;
}

export interface LspHandle {
  readonly document: LspDocument;
  /** 놓는다. 마지막이면 Host 의 문서를 닫는다. */
  release(): void;
}

export interface LspSessions {
  /** 이 창의 프로젝트. 문서 없이 부르는 기능(Search Everywhere 의 심볼 — M4-16)이 `services.call` 에 싣는다. */
  readonly projectId: string;
  readonly services: LspServices;
  /** 문서 `path` 를 언어 `language`(Linguist 이름)로 연다. `worktree` 는 그 탭의 워크트리 꼭대기(없으면 메인 체크아웃). */
  acquire(path: string, language: string, worktree?: string): LspHandle;
  /** 창이 `lsp.status` 이벤트를 받았다 — 열린 문서의 능력 칸을 다시 묻는다. */
  statusChanged(): void;
}

export interface LspSessionsDeps {
  source: LspSource;
  documents: Pick<DocumentStore, "onChange" | "store">;
  projectId: string;
  services: LspServices;
}

/** 코드 조각을 강조하는 `LspServices.highlight`: 구문 분석 서비스에 조각 하나를 열어 전체를 묻고 닫는다. */
export function snippetHighlighter(source: {
  open(p: { docId: string; language: string; text: string }): Promise<{ grammar: string | null }>;
  highlights(p: {
    docId: string;
    range: { from: number; to: number };
  }): Promise<{ spans: HighlightSpan[] }>;
  close(p: { docId: string }): Promise<unknown>;
}): NonNullable<LspServices["highlight"]> {
  const tag = Math.random().toString(36).slice(2, 8);
  let seq = 0;
  return async (language, code) => {
    const docId = `lsp-snippet-${tag}-${++seq}`;
    try {
      if ((await source.open({ docId, language, text: code })).grammar === null) return [];
      return (await source.highlights({ docId, range: { from: 0, to: code.length } })).spans;
    } catch {
      return [];
    } finally {
      void source.close({ docId }).catch(() => {});
    }
  };
}

/** 열면 다시 묻지 않는 오류 — 그 창이 이 문서를 열 수 없다. */
const refuses = (e: unknown) => {
  const code = errorCode(e);
  return code === ERROR_CODE.INVALID_PARAMS || code === ERROR_CODE.PERMISSION_DENIED;
};

/** 버려도 되는 실패: 취소 · 맡을 서버 없음. */
const dropped = (e: unknown) => {
  const code = errorCode(e);
  return code === ERROR_CODE.CANCELLED || code === LSP_ERROR.NO_SERVER;
};

/**
 * 이 탭의 문서를 언어 서버에 여는가: 파일 탭(`git` · `db` 문서가 아님)의 편집 등급(5 MiB 이하 · 1만 자 줄 없음) 텍스트 문서이고 언어가
 * 정해졌다. 루트 밖 문서(M4-08 의 읽기 전용 외부 소스 탭)는 `fileTab` 이 거짓이어야 한다 — 그 탭은 같은 서버의 hover · 정의만
 * 토큰 길로 쓴다(결정 M4-D13).
 */
export function lspEligible(
  fileTab: boolean,
  doc: DocumentState | undefined,
  language: string | null,
): boolean {
  return (
    fileTab &&
    doc?.status === "text" &&
    doc.judgment.sizeClass === "editable" &&
    !doc.judgment.longLines &&
    language !== null &&
    language !== PLAIN_TEXT
  );
}

export function createLspSessions(deps: LspSessionsDeps): LspSessions {
  const { source, documents, projectId } = deps;
  const { services } = deps;

  class Session {
    readonly sync: DocSync;
    capabilities: LspCapabilities = {};
    private readonly listeners = new Set<() => void>();
    /** 종류마다 돌고 있는 요청. */
    private readonly inflight = new Map<string, AbortController>();
    private asking = false;
    private askAgain = false;
    /** 묻기가 끝나기를 기다리는 것(`settled`). */
    private settledWaiters: (() => void)[] = [];

    constructor(
      readonly path: string,
      readonly language: string,
      readonly worktree: string | undefined,
    ) {
      this.sync = new DocSync(
        path,
        documents,
        {
          name: "lsp.document",
          skip: (text: Text) => overHighlightLimit(text) !== null,
          open: async (text) => {
            try {
              const opened = await source.open({
                projectId,
                ...(worktree !== undefined && { worktree }),
                path,
                language,
                version: 0,
                text: text.toString(),
              });
              return { docId: opened.docId, off: opened.off !== null };
            } catch (e) {
              if (refuses(e)) return { docId: "", off: true };
              throw e;
            }
          },
          change: async (edit) => ({ off: (await source.change(edit)).off !== null }),
          close: (docId) => void source.close({ docId }).catch(() => {}),
        },
        {
          opening: () => this.setCapabilities({}),
          opened: () => this.refresh(),
          // 서버가 받지 않는 문서의 칸은 비운다 — 기능 행이 트리거 문자에서 헛되이 묻지 않게(리뷰 L-3).
          off: () => this.setCapabilities({}),
        },
      );
    }

    private setCapabilities(next: LspCapabilities) {
      this.capabilities = next;
      for (const listener of this.listeners) listener();
    }

    onCapabilities(listener: () => void) {
      this.listeners.add(listener);
      return () => void this.listeners.delete(listener);
    }

    /** 묻는 중이면 묻기(이어 묻는 것까지)가 끝난 뒤, 아니면 곧바로 풀린다. */
    settled(): Promise<void> {
      if (!this.asking) return Promise.resolve();
      return new Promise((resolve) => this.settledWaiters.push(resolve));
    }

    /** 능력 칸을 다시 묻는다. 묻는 중이면 끝난 뒤 한 번 더. */
    refresh() {
      if (this.asking) {
        this.askAgain = true;
        return;
      }
      const docId = this.sync.docId;
      if (docId === undefined) return;
      this.asking = true;
      source
        .capabilities({ docId })
        .then(
          (answer) => {
            if (this.sync.isCurrent(docId)) this.setCapabilities(answer.features);
          },
          (e: unknown) => {
            // 문서를 잃었으면 다시 연다(열리면 다시 묻는다). 그 밖은 다음 상태 이벤트를 기다린다.
            if (errorCode(e) === ERROR_CODE.NOT_FOUND) this.sync.failed(docId, "capabilities", e);
          },
        )
        .finally(() => {
          this.asking = false;
          if (this.askAgain) {
            this.askAgain = false;
            this.refresh();
          }
          if (!this.asking) {
            const waiters = this.settledWaiters;
            this.settledWaiters = [];
            for (const resolve of waiters) resolve();
          }
        });
    }

    /** 저장했다. */
    saved() {
      const docId = this.sync.docId;
      if (docId !== undefined && !this.sync.holding) void source.save({ docId }).catch(() => {});
    }

    async request<T>(
      kind: string,
      call: (at: LspAt, signal: AbortSignal) => Promise<T>,
    ): Promise<T | undefined> {
      this.inflight.get(kind)?.abort();
      const controller = new AbortController();
      this.inflight.set(kind, controller);
      const { signal } = controller;
      const { sync } = this;
      // 부른 쪽은 위치를 지금 텍스트로 쟀다 — 여는 중 · 물러섬을 기다리는 사이 고치면 그 위치는 새 텍스트에 맞지 않으므로
      // 버린다(재리뷰 N-2). 텍스트는 바뀔 때마다 새 객체라 동일성으로 본다.
      const text = sync.currentText;
      try {
        if (sync.holding) return undefined;
        const settled = await sync.settled();
        if (signal.aborted || settled.kind !== "open" || sync.holding) return undefined;
        const version = sync.version;
        // 이 버전의 편집이 Host 에 닿은 뒤에 묻는다 — 같은 틱에 나간 편집보다 요청이 먼저 도착하면 지난 버전으로 답해 버려진다
        // (리뷰 H-1: `.` 직후의 완성).
        await sync.synced(version, signal);
        if (
          signal.aborted ||
          sync.currentText !== text ||
          !sync.isCurrent(settled.docId) ||
          sync.version !== version ||
          sync.holding
        )
          return undefined;
        const at: LspAt = {
          docId: settled.docId,
          version,
          projectId,
          ...(this.worktree !== undefined && { worktree: this.worktree }),
          path: this.path,
        };
        let answer: T;
        try {
          answer = await call(at, signal);
        } catch (e) {
          if (signal.aborted || dropped(e)) return undefined;
          if (errorCode(e) === ERROR_CODE.NOT_FOUND) sync.failed(at.docId, kind, e);
          throw e;
        }
        const moved = !sync.isCurrent(at.docId) || sync.version !== at.version || sync.holding;
        const stale = (answer as { staleVersion?: unknown } | null)?.staleVersion === true;
        return signal.aborted || stale || moved ? undefined : answer;
      } finally {
        if (this.inflight.get(kind) === controller) this.inflight.delete(kind);
      }
    }

    /** 끝낼 때 돌고 있는 요청을 모두 취소한다. */
    cancelAll() {
      for (const controller of this.inflight.values()) controller.abort();
      this.inflight.clear();
    }
  }

  const pool = new DocSyncPool<Session>((key) => {
    const [language, worktree, path] = key.split("\u0000") as [string, string, string];
    return new Session(path, language, worktree === "" ? undefined : worktree);
  });

  // 저장을 본다: `saving` 이 끝나며 디스크 `version` 이 바뀐 문서.
  documents.store.subscribe(({ docs }, before) => {
    for (const session of pool.values()) {
      const now = docs[session.path];
      const was = before.docs[session.path];
      if (
        now?.status === "text" &&
        was?.status === "text" &&
        was.saving &&
        !now.saving &&
        now.version !== was.version
      )
        session.saved();
    }
  });

  return {
    projectId,
    services,
    acquire(path, language, worktree) {
      const { entry, release } = pool.acquire(`${language}\u0000${worktree ?? ""}\u0000${path}`);
      const document: LspDocument = {
        projectId,
        ...(worktree !== undefined && { worktree }),
        path,
        language,
        get version() {
          return entry.sync.version;
        },
        get capabilities() {
          return entry.capabilities;
        },
        onCapabilities: (listener) => entry.onCapabilities(listener),
        capabilitiesSettled: () => entry.settled(),
        request: (kind, call) => entry.request(kind, call),
        call: (kind, method, params) =>
          entry.request(kind, (at, signal) => services.call(method, params(at), { signal })),
        hold: () => entry.sync.hold(),
        services,
      };
      return {
        document,
        release: () => {
          release();
          queueMicrotask(() => {
            if (pool.get(`${language}\u0000${worktree ?? ""}\u0000${path}`) !== entry)
              entry.cancelAll();
          });
        },
      };
    },
    statusChanged() {
      for (const session of pool.values()) session.refresh();
    },
  };
}
