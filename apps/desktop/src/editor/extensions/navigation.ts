// 커서 이동 · 선택 키(화살표 · Home/End · PageUp/Down · 단어 이동, CodeMirror 의 `standardKeymap`). 편집 키는 읽기 전용이라 돌지
// 않는다. IntelliJ 식 편집 · 조작 키(줄 복제 · 줄로 이동 등)는 M1-18 이 앞에 끼운다.

import { standardKeymap } from "@codemirror/commands";
import { keymap } from "@codemirror/view";

import type { EditorFeature } from ".";

export const navigation: EditorFeature = {
  id: "navigation",
  extension: () => keymap.of(standardKeymap),
};
