// 최근 파일(⌘E): 에디터 탭이 활성이 되면 기억하되, git 문서 탭(문서 키 `git:…`)은 파일이 아니라 넣지 않는다(M2-15 리뷰 F2 —
// 넣으면 프로젝트 상태에 저장돼 다시 시작한 뒤 그 문자열을 경로로 한 파일 탭이 열린다).

import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { expect, it } from "vitest";

import { EDITOR_PANEL, type EditorTabParams } from "../editor/editor-tabs";
import { createRecentFiles } from "./recent-files";

function fakeApi(params: EditorTabParams[]) {
  const panels = params.map(
    (p, i) =>
      ({ id: String(i), api: { component: EDITOR_PANEL }, params: p }) as unknown as IDockviewPanel,
  );
  let listener: ((e: { panel: IDockviewPanel }) => void) | undefined;
  const api = {
    panels,
    activePanel: panels[0],
    onDidActivePanelChange: (l: (e: { panel: IDockviewPanel }) => void) => {
      listener = l;
      return { dispose() {} };
    },
  } as unknown as DockviewApi;
  return { api, panels, activate: (i: number) => listener?.({ panel: panels[i]! }) };
}

it("git 문서 탭은 최근 파일에 넣지 않고 파일 탭만 넣는다", () => {
  const hash = "0123456789abcdef0123456789abcdef01234567";
  const { api, activate } = fakeApi([
    { path: `git:commit:${hash}`, git: { kind: "commit", rev: hash } },
    { path: "/work/demo/src/main.rs" },
    {
      path: `git:revision:${hash}:src/a.rs`,
      git: { kind: "revision", rev: hash, path: "src/a.rs" },
    },
  ]);
  const recent = createRecentFiles();
  recent.track(api);
  activate(2);
  activate(0);
  expect(recent.list()).toEqual([{ path: "/work/demo/src/main.rs" }]);
});
