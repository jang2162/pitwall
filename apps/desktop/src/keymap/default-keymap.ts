// 기본 키맵을 앱 번들에 묶는다(docs/design/10-ui.md §4.4 「정의 형식」 — 기본 키맵은 앱 내장).
// M0-15 확장 레지스트리 경유로 교체 예정 — 그때는 내장 확장 keymap-default 의 기여(`extension.toml`)로 이 파일을 받는다.

import text from "../../../../extensions/keymap-default/keymaps/default.toml?raw";

export const DEFAULT_KEYMAP_TOML: string = text;
