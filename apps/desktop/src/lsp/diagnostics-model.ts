// 진단(15-m4-plan M4-06)의 창 단위 상태: 창의 Host 연결(`LspServices` · 프로젝트 · 활성 워크트리), 화면 문자열, 상태바의 수.
// 프로젝트 창 하나가 웹뷰 하나라 모듈에 하나 둔다(`activeWorktreeStore` 와 같은 꼴) — 에디터 기능(diagnostics.ts) · 문제 툴 윈도우
// (problems.tsx) · 상태바 항목이 함께 읽는다. 창이 `useDiagnosticsUi`(problems.tsx)로 채운다.
//
// 진단의 꼴은 Host 의 것(`LspDiagnostic` — crates/pw-lsp/src/diagnostics.rs)이다: 위치는 LSP 그대로(줄 · UTF-16 열, 0 부터) — CodeMirror
// 의 오프셋과 같은 단위라 줄 시작에 열을 더하면 된다. 코드 액션(M4-12)에 넘기는 LSP `Diagnostic` 은 `lspDiagnostic` 으로 되살린다
// (Host 가 상한 안으로 다듬은 값 — 메시지 4 KiB · `data` 4 KiB 를 넘으면 버린 채다).

import type {
  LspDiagnostic,
  LspDiagnosticsCounts,
  LspDiagnosticsEvent,
  LspSeverity,
  UnaryMethods,
} from "@pitwall/rpc-client";
import { createStore } from "zustand/vanilla";

import type { LspServices } from "./lsp-sessions";

export type { LspDiagnostic, LspDiagnosticsCounts, LspDiagnosticsEvent, LspSeverity };

/** Host 가 경로의 진단이 바뀔 때 내는 이벤트(crates/pw-lsp `TOPIC_DIAGNOSTICS`). */
export const DIAGNOSTICS_TOPIC = "lsp.diagnostics";
/** Host 의 상한(crates/pw-lsp/src/diagnostics.rs `FILE_LIMIT` · `PROJECT_LIMIT`) — 넘친 표시의 글에 쓴다. */
export const FILE_LIMIT = 1_000;
export const PROJECT_LIMIT = 100_000;

/** 심한 것부터. */
export const SEVERITIES: readonly LspSeverity[] = ["error", "warning", "information", "hint"];

export const severityRank = (s: LspSeverity) => SEVERITIES.indexOf(s);

/** LSP 의 `DiagnosticSeverity` 수. */
const LSP_SEVERITY: Record<LspSeverity, number> = {
  error: 1,
  warning: 2,
  information: 3,
  hint: 4,
};

/** 코드 액션 요청(`CodeActionContext.diagnostics`)에 넘길 LSP `Diagnostic`(M4-12 — 진단 팝업의 `data.lsp`). */
export function lspDiagnostic(d: LspDiagnostic): Record<string, unknown> {
  return {
    range: d.range,
    severity: LSP_SEVERITY[d.severity],
    message: d.message,
    ...(d.code !== null && { code: d.code }),
    ...(d.source !== null && { source: d.source }),
    ...(d.codeHref !== null && { codeDescription: { href: d.codeHref } }),
    ...(d.tags.length > 0 && {
      tags: d.tags.map((t) => (t === "unnecessary" ? 1 : 2)),
    }),
    ...(d.related.length > 0 && {
      relatedInformation: d.related.map((r) => ({
        location: { uri: r.uri, range: r.range },
        message: r.message,
      })),
    }),
    ...(d.data !== null && { data: d.data }),
  };
}

/** 진단 팝업의 값(`PopupSpec.data`). M4-12 의 「빠른 수정」 이 `lsp[i]` 를 `servers[i]` 서버에만 넘긴다. */
export interface DiagnosticPopupData {
  readonly diagnostics: readonly LspDiagnostic[];
  /** `diagnostics` 와 같은 차례의 LSP `Diagnostic`. */
  readonly lsp: readonly Record<string, unknown>[];
  /** `diagnostics[i]` 를 낸 서버 id. */
  readonly servers: readonly string[];
  /**
   * `lsp` 의 범위가 맞는 문서 버전 — 모든 진단이 같은 버전일 때만 있다(버전이 없거나 서로 다르면 없다). M4-12 의 「빠른 수정」 은
   * 이것이 없으면 진단을 싣지 않고, Host 는 지금 문서 버전과 다르면 버린다(편집 뒤의 지난 범위를 서버에 넘기지 않는다).
   */
  readonly version?: number;
}

export function popupData(diagnostics: readonly LspDiagnostic[]): DiagnosticPopupData {
  const versions = new Set(diagnostics.map((d) => d.version));
  const [version] = versions;
  return {
    diagnostics,
    lsp: diagnostics.map(lspDiagnostic),
    servers: diagnostics.map((d) => d.server),
    ...(versions.size === 1 && typeof version === "number" && { version }),
  };
}

/** 워크트리 글을 견줄 꼴로(Host 의 `normalize_worktree` 와 같다 — 끝의 `/` 를 뗀다). */
export function normalizeWorktree(worktree: string | null | undefined): string | undefined {
  if (worktree === null || worktree === undefined) return undefined;
  const trimmed = worktree.replace(/\/+$/, "");
  return trimmed === "" ? worktree : trimmed;
}

/**
 * `services.call` 을 부르되 동기로 던진 것(창의 Host 가 아직 붙지 않음 — project-window.tsx 의 `host()`)도 거절된 약속으로 바꾼다.
 * 부르는 쪽(뷰 플러그인 · effect)이 던짐으로 깨지지 않게.
 */
export function callSafely<K extends keyof UnaryMethods & string>(
  services: LspServices,
  method: K,
  params: UnaryMethods[K]["params"],
): Promise<UnaryMethods[K]["result"]> {
  try {
    return services.call(method, params);
  } catch (e) {
    return Promise.reject(e);
  }
}

/** 화면 문자열(창의 언어). 창이 채우기 전에는 영어. */
export interface DiagnosticStrings {
  severity: Record<LspSeverity, string>;
  /** 「서버 · 코드」 줄의 서버 이름 앞 말. */
  source: (server: string, code: string | null) => string;
  /** 링크 단추(코드 설명). */
  openDocs: string;
  related: string;
}

export const ENGLISH_STRINGS: DiagnosticStrings = {
  severity: { error: "Error", warning: "Warning", information: "Information", hint: "Hint" },
  source: (server, code) => (code ? `${server} · ${code}` : server),
  openDocs: "Open documentation",
  related: "Related",
};

export interface DiagnosticsConnection {
  readonly services: LspServices;
  readonly projectId: string;
  /** 창의 활성 워크트리(없으면 메인 체크아웃). */
  readonly worktree?: string;
}

export interface DiagnosticsUiState {
  /** 창의 Host 연결. 붙기 전 · 프로젝트 창이 아니면 없다. */
  connection?: DiagnosticsConnection;
  strings: DiagnosticStrings;
  /** 상태바의 수(`lsp.diagnostics.counts`). 처음 받기 전에는 `null`. */
  counts: LspDiagnosticsCounts | null;
}

export const diagnosticsUi = createStore<DiagnosticsUiState>()(() => ({
  strings: ENGLISH_STRINGS,
  counts: null,
}));
