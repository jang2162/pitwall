// 창 종류는 Rust 가 창을 만들 때 붙이는 `?window=` 쿼리로 받는다(src-tauri/src/windows.rs 의 WindowKind).
export type WindowKind = "welcome" | "project" | "settings" | "file";

const KINDS: readonly WindowKind[] = ["welcome", "project", "settings", "file"];

export function parseWindowKind(search: string): WindowKind {
  const value = new URLSearchParams(search).get("window");
  return KINDS.find((kind) => kind === value) ?? "welcome";
}

/** 웰컴 창의 제목 줄. 앱 이름이라 번역하지 않는다 — 다른 창의 제목은 UI 언어를 따른다(i18n/app-strings.ts). */
export const WELCOME_TITLE = "Pitwall";
