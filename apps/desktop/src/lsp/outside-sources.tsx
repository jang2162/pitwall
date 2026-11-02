// 루트 밖 문서 — 「외부 소스」 탭(15-m4-plan M4-08, 결정 M4-D13). 언어 서버가 이 창에 돌려준 위치가 프로젝트 루트 밖(`~/.cargo/registry`
// · rustup 표준 라이브러리 소스 · JDK · `jdt://`)이면 Host 가 그 위치에 읽기 토큰을 달아 준다. 그 토큰으로만(`lsp.document.read`) 읽어
// 읽기 전용 탭으로 연다 — 파일 RPC(`fs.*`)의 루트 판정(결정 M1-D8)은 그대로다.
//
// - 작업 공간(editor-workspace.tsx)의 `OutsideSources` 자리를 채운다(M4-09 가 비워 둔 자리): 위치로 가기 · 뒤로 · 앞으로가 토큰 든
//   위치를 `open` 으로 열고, 지금 자리 · 큰 이동이 이 탭의 자리를 `locate` 로 읽는다. 같은 토큰은 탭 하나다.
// - 탭은 문서 모델 밖이다: 파일 탭이 아니므로(`isFileParams` 가 `outside` 를 본다) 저장 · 경로 복사 · 최근 파일 · 워크트리 탭 묶음에서
//   빠지고 다시 시작하면 되살리지 않는다(토큰은 그 창의 연결이 닫히면 Host 에서 지워진다). 글은 연 때 한 번 읽은 것이다.
// - 보기는 읽기 전용 CodeMirror 다(`EditorState.readOnly` — 편집 키는 아무것도 하지 않는다). 머리에 경로와 출처(그 위치를 돌려준
//   서버)를 보인다. 구문 강조는 리비전 텍스트와 같은 길(구문 분석 서비스에 그 글로 문서 하나 — git/diff/revision-syntax.ts). 그
//   안의 hover · 정의 · 사용처는 같은 서버로 간다(./navigation.ts 의 토큰 요청 길 — 진단 · 편집은 없다).
// - 키맵 문맥은 에디터 글자 영역(`editorFocus`)이라 `⌘B` · `F1` 등 내비게이션 키가 돈다.

import { EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  lineNumbers,
} from "@codemirror/view";
import type { StaticSyntaxSource } from "../git/diff/revision-syntax";
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { useEffect, useRef } from "react";

import { useEditorFont } from "../editor/editor-font";
import {
  baseName,
  EDITOR_PANEL,
  EDITOR_TAB_HEADER,
  type EditorTabParams,
  editorPanels,
  targetGroup,
  tabParams,
} from "../editor/editor-tabs";
import {
  type Cursor,
  cursorOf,
  type EditorWorkspace,
  type OutsideSources,
  placeCursor,
  useEditorWorkspace,
} from "../editor/editor-workspace";
import type { EditorFeatureContext } from "../editor/extensions";
import { editorContext } from "../editor/extensions/context";
import { fontTheme } from "../editor/extensions/operations";
import { editorTheme } from "../editor/extensions/theme";
import type { NavLocation, NavTarget } from "../editor/navigation";
import { revisionHighlight, revisionSyntax } from "../git/diff/revision-syntax";
import { useProjectStrings } from "../layout/strings";
import type { LspServices } from "./lsp-sessions";
import { navigationExtension, tokenSource } from "./navigation";
import { popupBase } from "./popup";

/** 외부 소스 탭의 `path` 앞부분(문서 키 — 실제 경로와 겹치지 않는다). */
export const OUTSIDE_KEY_PREFIX = "lsp:outside:";

/** 외부 소스 탭의 params `outside` 칸. */
export interface OutsideSpec {
  readonly token: string;
  /** 보일 경로(파일이면 절대 경로, 아니면 URI). */
  readonly path: string;
  /** 그 위치를 돌려준 언어 서버. */
  readonly server: string;
  /** Linguist 언어 이름(구문 강조 · 미리보기). 모르면 `null`. */
  readonly language: string | null;
  /** 그 토큰을 받은 문서의 워크트리 — 이 탭 안에서 루트 안 위치로 갈 때 · 미리볼 때 쓴다(리뷰 L4). 메인 체크아웃이면 없다. */
  readonly worktree?: string;
}

/** 탭의 외부 소스 칸. 외부 소스 탭이 아니면 `undefined`. */
export function outsideSpecOf(params: EditorTabParams): OutsideSpec | undefined {
  const spec = (params as { outside?: OutsideSpec }).outside;
  return spec && typeof spec.token === "string" ? spec : undefined;
}

interface Meta {
  path: string;
  server: string;
  language: string | null;
  worktree?: string;
}

/** 창의 것(늦게 붙는다 — 작업 공간을 만든 뒤). */
export interface OutsideBinding {
  workspace: EditorWorkspace;
  services: LspServices;
  projectId: string;
  /** 구문 강조(리비전 텍스트와 같은 길). 없으면 칠하지 않는다. */
  syntax?: StaticSyntaxSource;
  /** 열지 못했다(위치로 가기에서만 — 뒤로 · 앞으로는 조용하다). */
  failed?(message: string): void;
}

export interface OutsideSourcesRuntime extends OutsideSources {
  /** 창의 것을 붙인다. 돌려준 함수가 뗀다. */
  bind(binding: OutsideBinding): () => void;
  /** 위치 결과에서 본 토큰의 머리 정보를 적는다(내비게이션이 부른다). */
  remember(token: string, meta: Meta): void;
  /** 탭의 글(연 때 읽은 것). */
  text(token: string): string | undefined;
  binding(): OutsideBinding | undefined;
  /** 탭의 뷰가 생길 때 둘 커서(`open` 이 남긴 것)를 꺼낸다. */
  takeCursor(panelId: string): Cursor | undefined;
}

let current: OutsideSourcesRuntime | undefined;
let seq = 0;

function nextId(api: DockviewApi) {
  let id: string;
  do id = `outside-${++seq}`;
  while (api.getPanel(id));
  return id;
}

/** 창마다 하나. 작업 공간에 `outside` 로 넘기고, 창이 `bind` 로 Host 연결을 붙인다. */
export function createOutsideSources(): OutsideSourcesRuntime {
  const metas = new Map<string, Meta>();
  const texts = new Map<string, string>();
  const cursors = new Map<string, Cursor>();
  let bound: OutsideBinding | undefined;
  const runtime: OutsideSourcesRuntime = {
    bind(binding) {
      bound = binding;
      current = runtime;
      return () => {
        if (bound === binding) bound = undefined;
        if (current === runtime) current = undefined;
      };
    },
    binding: () => bound,
    remember(token, meta) {
      if (!metas.has(token)) metas.set(token, meta);
    },
    text: (token) => texts.get(token),
    takeCursor(panelId) {
      const cursor = cursors.get(panelId);
      cursors.delete(panelId);
      return cursor;
    },

    async open(location: NavTarget & { token: string }, { travel }) {
      const b = bound;
      const api = b?.workspace.api();
      if (!b || !api) return undefined;
      const { token } = location;
      if (!texts.has(token)) {
        try {
          const read = await b.services.call("lsp.document.read", { token });
          texts.set(token, read.text);
          const known = metas.get(token);
          metas.set(token, {
            path: read.path,
            server: read.server,
            language: known?.language ?? null,
            ...(known?.worktree !== undefined && { worktree: known.worktree }),
          });
        } catch (e) {
          if (!travel) b.failed?.(e instanceof Error ? e.message : String(e));
          return undefined;
        }
      }
      const meta = metas.get(token) ?? { path: location.path, server: "", language: null };
      let panel = editorPanels(api).find((p) => outsideSpecOf(tabParams(p))?.token === token);
      if (!panel) {
        const params = {
          path: `${OUTSIDE_KEY_PREFIX}${token}`,
          outside: { token, ...meta } satisfies OutsideSpec,
        };
        panel = api.addPanel({
          id: nextId(api),
          component: EDITOR_PANEL,
          tabComponent: EDITOR_TAB_HEADER,
          title: baseName(meta.path),
          params,
          position: { referenceGroup: targetGroup(api, b.workspace.recent()).id },
        });
      }
      if (location.line !== undefined) {
        const cursor = { line: location.line, column: location.column ?? 1 };
        const view = b.workspace.view(panel.id);
        if (view) placeCursor(view, cursor);
        else cursors.set(panel.id, cursor);
      }
      return panel;
    },

    locate(panel: IDockviewPanel, cursor: Cursor | undefined): NavLocation | undefined {
      const spec = outsideSpecOf(tabParams(panel));
      if (!spec) return undefined;
      return {
        path: spec.path,
        token: spec.token,
        line: cursor?.line ?? 1,
        column: cursor?.column ?? 1,
      };
    },
  };
  return runtime;
}

/** 창의 외부 소스(붙어 있으면). */
export function outsideSources(): OutsideSourcesRuntime | undefined {
  return current;
}

/** 외부 소스 탭의 내용(editor-panel.tsx 가 그린다). */
export function OutsideSourcePanel({ panelId, spec }: { panelId: string; spec: OutsideSpec }) {
  const T = useProjectStrings().editor;
  const workspace = useEditorWorkspace();
  const font = useEditorFont();
  const host = useRef<HTMLDivElement>(null);
  const runtime = outsideSources();
  const text = runtime?.text(spec.token);
  useEffect(() => {
    const parent = host.current;
    const binding = runtime?.binding();
    if (!parent || text === undefined || !binding) return;
    const { services, projectId, syntax } = binding;
    // 팝업 바탕(링크 · 코드 강조)이 읽는 문맥 — 열린 문서가 아니므로 그 몇 칸만 채운다.
    const context = {
      panelId,
      path: spec.path,
      language: spec.language,
      lsp: { services, language: spec.language },
    } as unknown as EditorFeatureContext;
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: text,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          highlightSpecialChars(),
          drawSelection(),
          editorTheme,
          fontTheme(font),
          EditorState.readOnly.of(true),
          editorContext.of(context),
          revisionHighlight(syntax && revisionSyntax(syntax, spec.language, text)),
          popupBase,
          navigationExtension(
            tokenSource({
              token: spec.token,
              path: spec.path,
              language: spec.language,
              ...(spec.worktree !== undefined && { worktree: spec.worktree }),
              services,
            }),
            projectId,
          ),
          EditorView.updateListener.of((u) => {
            if (!u.selectionSet) return;
            // 사람이 옮긴 커서만 큰 이동으로 본다(위치로 가기가 둔 커서는 아니다).
            const quiet = !u.transactions.some((tr) => tr.isUserEvent("select"));
            workspace.reportCursor(panelId, cursorOf(u.view), quiet);
          }),
        ],
      }),
    });
    const cursor = runtime?.takeCursor(panelId);
    if (cursor) placeCursor(view, cursor);
    const unregister = workspace.registerView(panelId, view);
    return () => {
      unregister();
      view.destroy();
    };
  }, [runtime, text, spec.token, spec.path, spec.language, panelId, workspace, font]);

  return (
    <div
      data-keymap-context="editorFocus"
      data-editor-tab={panelId}
      data-outside-source={spec.token}
      className="flex h-full min-h-0 flex-col bg-editor-background"
    >
      <div
        data-outside-source-bar=""
        className="flex flex-none items-center gap-2 border-b border-ui-border bg-ui-toolbar px-2 py-0.5 text-[12px] text-ui-muted-foreground"
      >
        <span className="flex-none">{T.lsp.outsideBar(spec.server)}</span>
        <span className="min-w-0 truncate font-mono" title={spec.path}>
          {spec.path}
        </span>
      </div>
      <div className="min-h-0 flex-1">
        {text === undefined ? (
          <p className="p-2 text-[12px]">{T.loading}</p>
        ) : (
          <div ref={host} data-outside-source-text="" className="h-full min-h-0" />
        )}
      </div>
    </div>
  );
}
