// 터미널이 포커스를 가졌을 때 앱 단축키와 셸로 보낼 키의 경계(10-ui.md §4.4 「터미널」). 터미널의 뿌리 요소가 `terminalFocus`
// 컨텍스트를 내고(src/keymap/context.ts), 그동안 키맵 엔진은 아래 둘 중 하나에 해당하는 바인딩만 본다(엔진의 「창 규칙」):
//
// 1. `when` 이 `terminalFocus` 를 읽는 바인딩 — 터미널 전용 액션(새 탭 · 탭 닫기 · Win/Linux 의 복사 · 붙여넣기)과, 사용자가
//    keymap.toml 에 `when = "terminalFocus"` 로 더한 키. 사용자가 셸 대신 앱이 받게 하고 싶은 키는 이렇게 더한다.
// 2. `TERMINAL_APP_ACTIONS` 의 액션 — 터미널에 있어도 쓰는 창 단위 액션(액션 찾기 · Search Everywhere 와 그 탭(파일로 이동 ·
//    최근 파일 · 프로젝트 스위처) · 툴 윈도우 열고 닫기 · 설정 · 외부 도구 · 탭 전환기). 단 Shift · Alt · ⌘ 없는 Ctrl+글자는
//    셸의 제어 문자(Ctrl+E 줄 끝 · Ctrl+N · Ctrl+P 기록 이동 등)라 이 액션의 키여도 셸로 보낸다 — Win/Linux 의 최근 파일
//    (Ctrl+E)이 그렇다.
//
// 나머지 키(Ctrl+C · Ctrl+W · ⌥ 조합 · 방향키 등)는 셸로 간다. 에디터 액션(⌘W 의 탭 닫기 등)도 터미널에서는 돌지 않는다.
// xterm 은 키를 먼저 받으므로 `attachCustomKeyEventHandler` 에서 엔진의 `peek` 으로 창이 가져갈 키인지 묻고 양보한다.
// macOS 의 ⌘C · ⌘V 는 키맵이 아니라 네이티브 Edit 메뉴(src-tauri/src/menu.rs)가 웹뷰의 copy · paste 이벤트로 보내고 xterm 이 받는다.

import { type Binding, type ContextLayers, lookup, type Stroke, whenReads } from "@pitwall/keymap";

export const TERMINAL_FOCUS = "terminalFocus";

/** 터미널이 포커스를 가져도 창이 받는 액션. 툴 윈도우 열기(`toolWindow.*`)는 모두 받는다. */
export const TERMINAL_APP_ACTIONS: ReadonlySet<string> = new Set([
  "action.find",
  "search.everywhere",
  "search.gotoFile",
  "search.recentFiles",
  "project.switcher",
  "settings.open",
  "openWith.popup",
  "openWith.default",
  "editor.switcher",
]);

/** 셸이 제어 문자로 받는 Ctrl+글자(Shift · Alt · ⌘ 없이). */
function isControlCharacter(s: Stroke): boolean {
  return s.ctrl && !s.shift && !s.alt && !s.cmd && /^Key[A-Z]$/.test(s.code);
}

/** 키맵 엔진의 `admits`. 터미널 포커스가 아니면 모든 바인딩을 본다. */
export function terminalAdmits(binding: Binding, layers: ContextLayers): boolean {
  if (!lookup(layers, TERMINAL_FOCUS)) return true;
  if (whenReads(binding.whenExpr, TERMINAL_FOCUS)) return true;
  const first = binding.sequence.kind === "strokes" ? binding.sequence.strokes[0] : undefined;
  if (first && isControlCharacter(first)) return false;
  return binding.action.startsWith("toolWindow.") || TERMINAL_APP_ACTIONS.has(binding.action);
}
