// 프로젝트 창에 내비게이션(15-m4-plan M4-08)을 붙인다: 키맵 액션(선언 · 구현 · 사용처 · 빠른 문서), 위치 팝업이 쓰는 창의 것
// (위치로 가기 · 문자열 · 사용처 고정), 외부 소스 탭의 Host 연결, 서버의 `window/showDocument`(Host 가 루트 안 파일만 받아 낸
// `lsp.showDocument` 이벤트 — 위치로 가기로 연다).
//
// 액션은 글자 영역(`editorFocus`)에서 돈다(기본 키맵의 「코드 (LSP · 편집)」 묶음). 대상은 포커스를 가진 에디터, 없으면 활성 탭의
// 뷰다(멀티 커서 액션과 같다). 그 뷰에 요청 길(`navSource` — 열린 문서 · 외부 소스)이 없거나 맡을 서버가 없으면 아무것도 하지
// 않는다(미신뢰 프로젝트 · 서버 없음 — 오류를 알리지 않는다).

import type { Action, Platform } from "@pitwall/keymap";
import type { LspShowDocument } from "@pitwall/rpc-client";
import { useEffect, useMemo } from "react";

import { targetView } from "../editor/editor-ops";
import type { EditorWorkspace } from "../editor/editor-workspace";
import type { StaticSyntaxSource } from "../git/diff/revision-syntax";
import { useKeymap, useRegisterActions } from "../keymap/keymap-provider";
import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { useNotify } from "../notifications/notifications";
import { useHost } from "../rpc/host";
import { pinUsages } from "../search/find-in-files";
import {
  findUsages,
  gotoDeclaration,
  gotoImplementation,
  navSource,
  quickDocumentation,
  setNavigationHost,
} from "./navigation";
import type { OutsideSourcesRuntime } from "./outside-sources";

/** 서버가 루트 안 문서를 열라고 했다(crates/pw-lsp/src/navigation.rs 의 `TOPIC_SHOW_DOCUMENT`). */
export const LSP_SHOW_DOCUMENT_TOPIC = "lsp.showDocument";

/** 키맵 액션. 키는 기본 키맵의 「코드 (LSP · 편집)」 묶음. */
export function lspNavigationActions(deps: {
  workspace: EditorWorkspace;
  strings: ProjectStrings["editor"]["lsp"];
}): Action[] {
  const T = deps.strings;
  const view = () => targetView(deps.workspace);
  const enabled = () => {
    const v = view();
    return v !== undefined && v.state.facet(navSource) !== undefined;
  };
  const at =
    (run: (v: NonNullable<ReturnType<typeof view>>, offset: number) => Promise<void>) => () => {
      const v = view();
      if (v) void run(v, v.state.selection.main.head);
    };
  return [
    {
      id: "lsp.gotoDeclaration",
      label: T.gotoDeclaration,
      keywords: ["Go to Declaration or Usages", "Definition"],
      enabled,
      run: at(gotoDeclaration),
    },
    {
      id: "lsp.gotoImplementation",
      label: T.gotoImplementation,
      keywords: ["Go to Implementation(s)"],
      enabled,
      run: at(gotoImplementation),
    },
    {
      id: "lsp.findUsages",
      label: T.findUsages,
      keywords: ["Find Usages", "References"],
      enabled,
      run: at(findUsages),
    },
    {
      id: "lsp.quickDocumentation",
      label: T.quickDocumentation,
      keywords: ["Quick Documentation", "Hover"],
      enabled,
      run: at(quickDocumentation),
    },
  ];
}

/** 창의 내비게이션을 붙인다. `outside` 는 작업 공간에 넘긴 외부 소스(같은 것). */
export function useLspNavigation(
  workspace: EditorWorkspace,
  outside: OutsideSourcesRuntime | undefined,
  projectId: string | undefined,
) {
  const strings = useProjectStrings().editor.lsp;
  const { client } = useHost();
  // 외부 소스 탭의 구문 강조(리비전 텍스트와 같은 길 — 구문 분석 서비스에 그 글로 문서 하나).
  const syntax = useMemo<StaticSyntaxSource | undefined>(
    () =>
      client && {
        open: (params) => client.call("syntax.open", params),
        highlights: (params) => client.call("syntax.highlights", params),
        close: (params) => client.call("syntax.close", params),
      },
    [client],
  );
  const { platform } = useKeymap();
  const notify = useNotify();
  useEffect(
    () =>
      setNavigationHost({
        navigateTo: (target, options) => workspace.navigateTo(target, options),
        strings,
        mac: isMac(platform),
        pinUsages: (title, files) => pinUsages(workspace.api(), title, files),
        rememberToken: (token, meta) => outside?.remember(token, meta),
      }),
    [workspace, strings, platform, outside],
  );
  useEffect(() => {
    const services = workspace.lsp?.services;
    if (!outside || !services || projectId === undefined) return;
    return outside.bind({
      workspace,
      services,
      projectId,
      ...(syntax && { syntax }),
      failed: (message) => notify({ level: "error", title: strings.outsideFailed(message) }),
    });
  }, [workspace, outside, projectId, syntax, notify, strings]);
  useEffect(() => {
    const services = workspace.lsp?.services;
    if (!services) return;
    return services.subscribe([LSP_SHOW_DOCUMENT_TOPIC], (event) => {
      const show = event.payload as unknown as LspShowDocument;
      void workspace.navigateTo(
        {
          path: show.path,
          ...(show.worktree !== null && { worktree: show.worktree }),
          ...(show.line !== null && { line: show.line, column: show.column ?? 1 }),
        },
        { record: true },
      );
    });
  }, [workspace]);
  const actions = useMemo(() => lspNavigationActions({ workspace, strings }), [workspace, strings]);
  useRegisterActions(actions);
}

function isMac(platform: Platform) {
  return platform === "mac";
}
