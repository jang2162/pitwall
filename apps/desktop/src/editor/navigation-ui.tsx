// 내비게이션 기록(navigation.ts)의 화면 쪽(15-m4-plan M4-09): 뒤로 · 앞으로 액션(⌘[ · ⌘], Win/Linux Ctrl+Alt+← · →), 마우스
// 4 · 5 버튼(10-ui §5.1), 「최근 위치」 팝업(⌘⇧E — 기록을 최근 것부터 보인다, 고르면 위치로 가기로 연다).
//
// 마우스 뒤로 · 앞으로 버튼은 `mousedown`(button 3 · 4)에서 한 번 돈다. `mousedown` 없이 `auxclick` 만 오는 웹뷰를 위해 `auxclick`
// 도 받되, 같은 누름의 `mousedown` 을 이미 처리했으면 건너뛴다. 웹뷰의 기본 동작(페이지 뒤로가기)은 webview-guards.ts 가 막는다.
// 실제 장치의 버튼이 WKWebView 에 이 이벤트로 오는지는 사람이 확인한다(15-m4-plan §4 「사람 확인」 ⑦). 창이 띄운 팝업(대화상자 ·
// Search Everywhere · 이 파일의 「최근 위치」)이 떠 있으면 버튼은 쉰다 — 키(⌘[)가 팝업 동안 쉬는 것과 같다.
//
// 「최근 위치」 팝업은 열리면 목록이 포커스를 가져가고 키를 목록에서 받는다 — 떠 있는 동안 글자 · ⌫ 가 밑의 문서로 가지 않는다.
// 한글 조합 중의 키(`isComposingKey`)는 보지 않는다. 닫으면(Esc · 바깥 누르기) 에디터로 포커스를 돌리고, 고르면 위치로 가기가 옮긴다.

import type { Action } from "@pitwall/keymap";
import {
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { useKeymap, useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { isComposingKey } from "../ui/ime";
import { baseName } from "./editor-tabs";
import type { EditorWorkspace } from "./editor-workspace";
import type { NavLocation } from "./navigation";

/** MouseEvent.button 의 뒤로 · 앞으로(마우스 4 · 5번 버튼). */
const MOUSE_BACK = 3;
const MOUSE_FORWARD = 4;

type NavStrings = ProjectStrings["editor"]["navigation"];

export function navigationActions(deps: {
  workspace: EditorWorkspace;
  strings: NavStrings;
  openRecentLocations(): void;
}): Action[] {
  const { workspace, strings: T } = deps;
  return [
    {
      id: "navigate.back",
      label: T.back,
      keywords: ["Back", "Navigate Back"],
      run: () => void workspace.navigation.back(),
    },
    {
      id: "navigate.forward",
      label: T.forward,
      keywords: ["Forward", "Navigate Forward"],
      run: () => void workspace.navigation.forward(),
    },
    {
      id: "navigate.recentLocations",
      label: T.recentLocations,
      keywords: ["Recent Locations"],
      run: deps.openRecentLocations,
    },
  ];
}

/** 마우스 뒤로 · 앞으로 버튼을 듣는다(머리 주석). `blocked()` 가 참이면(창 팝업이 떠 있다) 쉰다. 해제 함수를 돌려준다. */
export function listenMouseNavigation(
  target: Window,
  workspace: EditorWorkspace,
  blocked: () => boolean = () => false,
): () => void {
  /** `mousedown` 으로 처리한 누름 — 그 누름의 `auxclick` 은 건너뛴다. */
  let handled = false;
  const go = (button: number) => {
    if (blocked()) return;
    if (button === MOUSE_BACK) void workspace.navigation.back();
    else void workspace.navigation.forward();
  };
  const onDown = (e: MouseEvent) => {
    if (e.button !== MOUSE_BACK && e.button !== MOUSE_FORWARD) return;
    handled = true;
    go(e.button);
  };
  const onAux = (e: MouseEvent) => {
    if (e.button !== MOUSE_BACK && e.button !== MOUSE_FORWARD) return;
    if (handled) handled = false;
    else go(e.button);
  };
  target.addEventListener("mousedown", onDown);
  target.addEventListener("auxclick", onAux);
  return () => {
    target.removeEventListener("mousedown", onDown);
    target.removeEventListener("auxclick", onAux);
  };
}

function parentOf(path: string): string {
  const name = baseName(path);
  return path.slice(0, Math.max(0, path.length - name.length - 1));
}

function RecentLocations({
  workspace,
  entries,
  onClose,
}: {
  workspace: EditorWorkspace;
  entries: readonly NavLocation[];
  /** `refocus` 면 에디터로 포커스를 돌린다(고르지 않고 닫음). */
  onClose(refocus: boolean): void;
}) {
  const T = useProjectStrings().editor.navigation;
  useKeymapPopup(true);
  const [index, setIndex] = useState(0);
  const list = useRef<HTMLDivElement>(null);

  // 열리면 목록이 포커스를 가져간다 — 떠 있는 동안의 키가 밑의 에디터로 가지 않는다.
  useEffect(() => list.current?.focus(), []);
  useEffect(() => {
    const onBlur = () => onClose(false);
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [onClose]);

  const choose = (i: number) => {
    const target = entries[i];
    onClose(target === undefined);
    if (target) void workspace.navigateTo(target, { record: true });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (isComposingKey(e.nativeEvent)) return;
    const n = entries.length;
    if (e.key === "ArrowDown") {
      if (n > 0) setIndex((index + 1) % n);
    } else if (e.key === "ArrowUp") {
      if (n > 0) setIndex((index - 1 + n) % n);
    } else if (e.key === "Enter") choose(index);
    else if (e.key === "Escape") onClose(true);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <div
      className="fixed inset-0 z-[2100] flex items-start justify-center pt-[15vh]"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose(true);
      }}
    >
      <div
        ref={list}
        tabIndex={-1}
        onKeyDown={onKeyDown}
        role="listbox"
        aria-label={T.recentLocations}
        data-testid="recent-locations"
        className="w-[560px] max-w-[90vw] rounded-md outline-none border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg"
      >
        <div className="px-3 pb-1 text-[11px] text-ui-muted-foreground">{T.recentLocations}</div>
        {entries.length === 0 && (
          <div className="px-3 py-1 text-ui-muted-foreground">{T.empty}</div>
        )}
        {entries.map((loc, i) => {
          const doc = workspace.documents.get(loc.path);
          const text =
            doc?.status === "text" && loc.line <= doc.live.text.lines
              ? doc.live.text.line(loc.line).text.trim()
              : undefined;
          return (
            <div
              // 같은 자리가 두 번 들 수 있다(뒤로 · 앞으로 양쪽) — 차례로 가른다.
              key={`${i}:${loc.path}:${loc.line}`}
              role="option"
              aria-selected={i === index}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => choose(i)}
              className={`flex flex-col px-3 py-0.5 ${i === index ? "bg-ui-active" : ""}`}
            >
              <div className="flex items-center gap-2">
                <span className="flex-none">{`${baseName(loc.path)}:${loc.line}`}</span>
                <span className="min-w-0 flex-1 truncate text-[11px] text-ui-muted-foreground">
                  {parentOf(loc.path)}
                </span>
              </div>
              {text !== undefined && (
                <div className="truncate font-mono text-[11px] text-ui-muted-foreground">
                  {text}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** 창에 내비게이션 기록의 액션 · 마우스 버튼 · 「최근 위치」 팝업을 붙인다. 돌려준 요소(팝업)를 창이 그린다. */
export function useNavigationUi(workspace: EditorWorkspace): ReactNode {
  const strings = useProjectStrings().editor.navigation;
  const [entries, setEntries] = useState<readonly NavLocation[]>();
  // 최근 것부터. 떠 있는 동안 기록이 바뀌어도(편집으로 옮김) 연 때의 목록을 보인다.
  const openRecent = useCallback(
    () => setEntries([...workspace.navigation.entries()].reverse()),
    [workspace],
  );
  const close = useCallback(
    (refocus: boolean) => {
      setEntries(undefined);
      const tab = refocus ? workspace.activeTab() : undefined;
      if (tab) workspace.focus(tab.id);
    },
    [workspace],
  );
  const { popupOpen } = useKeymap();
  const actions = useMemo(
    () => navigationActions({ workspace, strings, openRecentLocations: openRecent }),
    [workspace, strings, openRecent],
  );
  useRegisterActions(actions);
  useEffect(() => listenMouseNavigation(window, workspace, popupOpen), [workspace, popupOpen]);
  return entries ? (
    <RecentLocations workspace={workspace} entries={entries} onClose={close} />
  ) : null;
}
