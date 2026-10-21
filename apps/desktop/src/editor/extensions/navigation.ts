// 커서 이동 · 선택 키(화살표 · Home/End · PageUp/Down · 단어 이동)와 기본 편집 키(지우기 · Enter), CodeMirror 의 `standardKeymap`.
// 편집 키는 읽기 전용 문서에서 돌지 않는다. undo · Tab 은 `editing` 이, IntelliJ 식 편집 · 조작 키(줄 복제 · 줄로 이동 등)는
// 창의 키맵 액션이 한다(../editor-ops.ts, 기본 키는 extensions/keymap-default/keymaps/default.toml).
//
// 창의 키맵과 같은 키는 여기서 뺀다 — CodeMirror 가 먼저 받아 처리해도 이벤트는 창(window)까지 올라가 키맵 액션이 또 돈다.
// macOS 의 ⌘⌫ 는 standardKeymap 에서 「줄 앞까지 지우기」 지만 IntelliJ 키맵에서는 줄 삭제(`editor.deleteLine`)다. ⌥⌫(단어
// 지우기)는 그대로 둔다.

import { standardKeymap } from "@codemirror/commands";
import { keymap } from "@codemirror/view";

import type { EditorFeature } from ".";

const keys = standardKeymap.filter((b) => !(b.mac === "Mod-Backspace" && b.key === undefined));

export const navigation: EditorFeature = {
  id: "navigation",
  extension: () => keymap.of(keys),
};
