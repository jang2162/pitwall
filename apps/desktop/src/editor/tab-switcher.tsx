// ⌃Tab 탭 전환기(10-ui §4.2 「탭 전환기」, IntelliJ Switcher): 에디터 탭을 최근에 쓴 순서로 보이고, ⌃ 를 누른 채 Tab 을 더 누르면
// 다음(⇧Tab 은 앞)으로 옮기고, ⌃ 를 떼면 고른 탭으로 간다. 열리면 바로 앞에 쓴 탭(두 번째)을 고른다 — ⌃Tab 한 번이 두 탭을 오간다.
// ↑↓ · Enter · Esc · 클릭도 받는다. 액션(`editor.switcher`)은 열기만 하고, 열린 동안의 키는 여기서 받는다.
//
// 키 리스너는 열리기 전부터 붙어 있다 — ⌃Tab 을 빨리 눌렀다 떼면 ⌃ 를 뗀 것이 팝업이 그려지기 전에 온다. 열림 상태는 ref 로 읽어
// 그 순간에도 고른 탭으로 간다.

import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { baseName, isEditorPanel, tabParams } from "./editor-tabs";
import type { EditorWorkspace } from "./editor-workspace";

interface SwitcherState {
  /** 최근 순서의 패널 id. */
  ids: string[];
  index: number;
}

function parentOf(path: string): string {
  const name = baseName(path);
  return path.slice(0, Math.max(0, path.length - name.length - 1));
}

export function useTabSwitcher(workspace: EditorWorkspace): {
  open(): void;
  element: ReactNode;
} {
  const T = useProjectStrings().editor;
  const [state, setState] = useState<SwitcherState>();
  const current = useRef<SwitcherState>(undefined);
  useKeymapPopup(state !== undefined);

  const update = useCallback((next: SwitcherState | undefined) => {
    current.current = next;
    setState(next);
  }, []);

  const choose = useCallback(
    (s: SwitcherState) => {
      update(undefined);
      const id = s.ids[s.index];
      const panel = id === undefined ? undefined : workspace.api()?.getPanel(id);
      if (!panel) return;
      panel.api.setActive();
      workspace.view(panel.id)?.focus();
    },
    [workspace, update],
  );

  const open = useCallback(() => {
    const api = workspace.api();
    const ids = workspace.recent().filter((id) => isEditorPanel(api?.getPanel(id)));
    if (ids.length === 0) return;
    update({ ids, index: ids.length > 1 ? 1 : 0 });
  }, [workspace, update]);

  useEffect(() => {
    const move = (s: SwitcherState, by: number) =>
      update({ ...s, index: (s.index + by + s.ids.length) % s.ids.length });
    const onDown = (e: KeyboardEvent) => {
      const s = current.current;
      if (!s) return;
      const handled = () => {
        e.preventDefault();
        e.stopImmediatePropagation();
      };
      if (e.key === "Tab") {
        handled();
        move(s, e.shiftKey ? -1 : 1);
      } else if (e.key === "ArrowDown") {
        handled();
        move(s, 1);
      } else if (e.key === "ArrowUp") {
        handled();
        move(s, -1);
      } else if (e.key === "Enter") {
        handled();
        choose(s);
      } else if (e.key === "Escape") {
        handled();
        update(undefined);
      }
    };
    const onUp = (e: KeyboardEvent) => {
      const s = current.current;
      if (s && e.key === "Control") choose(s);
    };
    const onBlur = () => current.current && update(undefined);
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      window.removeEventListener("blur", onBlur);
    };
  }, [choose, update]);

  const api = workspace.api();
  const element = state ? (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[15vh]">
      <div
        role="listbox"
        aria-label={T.switcher}
        className="w-[420px] max-w-[90vw] rounded-md border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg"
      >
        <div className="px-3 pb-1 text-[11px] text-ui-muted-foreground">{T.switcher}</div>
        {state.ids.map((id, i) => {
          const panel = api?.getPanel(id);
          if (!panel) return null;
          const { path, preview } = tabParams(panel);
          return (
            <div
              key={id}
              role="option"
              aria-selected={i === state.index}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => choose({ ...state, index: i })}
              className={`flex h-[24px] items-center gap-2 px-3 ${i === state.index ? "bg-ui-active" : ""}`}
            >
              <span className={`flex-none ${preview ? "italic" : ""}`}>{baseName(path)}</span>
              <span className="min-w-0 flex-1 truncate text-[11px] text-ui-muted-foreground">
                {parentOf(path)}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  ) : null;

  return { open, element };
}
