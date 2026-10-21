// 에디터 탭의 머리(dockview 탭 `EDITOR_TAB_HEADER`): 파일 이름, 미리보기면 이탤릭, 고정이면 핀. 마우스(10-ui §5.2): 클릭 활성 ·
// 가운데 클릭 닫기(dockview 기본 탭) · 더블클릭 에디터 최대화 토글 · 우클릭 메뉴. 메뉴 항목은 모두 탭 액션(editor-actions.ts,
// 「외부 도구로 열기」 · 「Finder 에서 보기」 는 project-window.tsx 의 ProjectOpenWith)이다 — 그 탭을 활성으로 만든 뒤 액션을
// 실행하므로 키로 실행한 것과 같은 길을 지난다. ×(가운데 클릭 포함)도 탭 닫기 액션이라 미저장 확인을 지난다(M1-13).
// 문서에 미저장 편집이 있으면 이름 앞에 ● 를 둔다.

import * as ContextMenu from "@radix-ui/react-context-menu";
import { DockviewDefaultTab, type IDockviewPanelHeaderProps } from "dockview-react";
import { Pin } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { useStore } from "zustand";

import { useAppStrings } from "../i18n/app-strings";
import { useKeymap, useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import type { EditorTabParams } from "./editor-tabs";
import { useEditorWorkspace } from "./editor-workspace";

/** 탭 params. `updateParameters`(미리보기 · 고정이 바뀜)를 따라 다시 그린다. */
function useTabParams(
  api: IDockviewPanelHeaderProps["api"],
  initial: EditorTabParams,
): EditorTabParams {
  const [params, setParams] = useState(initial);
  useEffect(() => {
    const changed = api.onDidParametersChange((p) => setParams(p as unknown as EditorTabParams));
    return () => changed.dispose();
  }, [api]);
  return params;
}

function MenuItem({ children, onSelect }: { children: ReactNode; onSelect(): void }) {
  return (
    <ContextMenu.Item
      onSelect={onSelect}
      className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active"
    >
      {children}
    </ContextMenu.Item>
  );
}

export function EditorTabHeader(props: IDockviewPanelHeaderProps<EditorTabParams>) {
  const T = useProjectStrings().editor;
  const openWith = useAppStrings().openWith;
  const { registry, platform } = useKeymap();
  const params = useTabParams(props.api, props.params);
  const { documents } = useEditorWorkspace();
  const dirty = useStore(documents.store, (s) => {
    const doc = s.docs[params.path];
    return doc?.status === "text" && doc.dirty;
  });
  const [menuOpen, setMenuOpen] = useState(false);
  useKeymapPopup(menuOpen);

  const run = (action: string) => {
    props.api.setActive();
    registry.run(action);
  };

  return (
    <ContextMenu.Root onOpenChange={setMenuOpen}>
      <ContextMenu.Trigger asChild>
        <div
          data-editor-tab-header={props.api.id}
          data-preview={params.preview ? "" : undefined}
          data-dirty={dirty ? "" : undefined}
          title={params.path}
          onDoubleClick={() => run("editor.toggleMaximized")}
          className={`flex h-full items-center ${params.preview ? "italic" : ""}`}
        >
          {params.pinned && <Pin size={11} aria-label={T.pinned} className="ml-1.5 flex-none" />}
          {dirty && (
            <span role="img" aria-label={T.unsaved} title={T.unsaved} className="ml-1.5 flex-none">
              ●
            </span>
          )}
          <DockviewDefaultTab {...props} closeActionOverride={() => run("editor.closeTab")} />
        </div>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          onCloseAutoFocus={(e) => e.preventDefault()}
          className="min-w-[200px] rounded-md border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg"
        >
          <MenuItem onSelect={() => run("editor.closeTab")}>{T.actions.closeTab}</MenuItem>
          <MenuItem onSelect={() => run("editor.closeOtherTabs")}>
            {T.actions.closeOtherTabs}
          </MenuItem>
          <MenuItem onSelect={() => run("editor.togglePin")}>
            {params.pinned ? T.menu.unpin : T.menu.pin}
          </MenuItem>
          <MenuItem onSelect={() => run("editor.splitRight")}>{T.actions.splitRight}</MenuItem>
          <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
          <MenuItem onSelect={() => run("editor.copyPath")}>{T.actions.copyPath}</MenuItem>
          <MenuItem onSelect={() => run("editor.reveal")}>{T.actions.reveal(platform)}</MenuItem>
          <MenuItem onSelect={() => run("openWith.popup")}>{openWith.action}</MenuItem>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
