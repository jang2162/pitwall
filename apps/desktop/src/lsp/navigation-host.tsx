// 프로젝트 창에 내비게이션(15-m4-plan M4-08)을 붙인다: 키맵 액션(선언 · 구현 · 사용처 · 빠른 문서), 위치 팝업이 쓰는 창의 것
// (위치로 가기 · 문자열 · 사용처 고정), 외부 소스 탭의 Host 연결, 서버의 `window/showDocument`(Host 가 루트 안 파일만 받아 낸
// `lsp.showDocument` 이벤트 — 위치로 가기로 연다). 그 이벤트는 프로젝트의 모든 창에 오지만 Host 가 고른 창(그 서버에 문서를 연
// 창 중 가장 최근에 그 서버를 쓴 창, `window`)만 연다. `takeFocus` 가 아니면 키보드 포커스를 그 전 자리에 두고, `takeFocus` 면
// 앱이 포커스를 가졌을 때만 창을 앞으로 가져온다(리뷰 M2).
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
import { bringForward, windowLabel } from "./window-focus";

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
        outsideText: (token) => outside?.text(token),
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
      if ((show.window ?? null) !== windowLabel()) return;
      void showDocument(workspace, show);
    });
  }, [workspace]);
  const actions = useMemo(() => lspNavigationActions({ workspace, strings }), [workspace, strings]);
  useRegisterActions(actions);
}

/** 서버가 보이라고 한 문서를 연다(위치로 가기 — 기록이 남는다). 포커스는 머리 주석. */
async function showDocument(workspace: EditorWorkspace, show: LspShowDocument) {
  const before = document.activeElement;
  const opened = await workspace.navigateTo(
    {
      path: show.path,
      ...(show.worktree !== null && { worktree: show.worktree }),
      ...(show.line !== null && { line: show.line, column: show.column ?? 1 }),
    },
    { record: true },
  );
  if (!opened) return;
  if (show.takeFocus) return bringForward();
  // 위치로 가기는 다음 프레임에 에디터로 포커스를 준다(새 탭이면 뷰가 붙을 때) — 그 뒤에 그 전 자리로 돌려놓는다.
  if (!(before instanceof HTMLElement)) return;
  await new Promise<void>((done) =>
    requestAnimationFrame(() => requestAnimationFrame(() => done())),
  );
  if (before.isConnected && document.activeElement !== before)
    before.focus({ preventScroll: true });
}

function isMac(platform: Platform) {
  return platform === "mac";
}
