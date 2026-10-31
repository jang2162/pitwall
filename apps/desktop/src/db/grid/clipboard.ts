// 「다른 형식으로 복사」 의 클립보드 쓰기(M3-16 리뷰). 글은 비동기로 만들어진다 — INSERT 는 Host 왕복(`db.sql.insertText`) 뒤다.
// 웹뷰의 `navigator.clipboard.writeText` 는 WebKit 에서 사용자 동작(클릭 · 키) 안에서만 쓰기를 허락하고 `await` 뒤는 그 밖이라
// 거절될 수 있다. 사용자 동작 안에서 `ClipboardItem` 에 Promise 를 넘기는 길도 있지만, 그것은 웹뷰의 보안 문맥(`tauri://` ·
// 개발 서버) · 플랫폼 웹뷰(WKWebView · WebKitGTK · WebView2)마다 조건이 달라 확실하지 않다. 그래서 Host 의 클립보드 쓰기
// (`write_clipboard` — src-tauri/src/clipboard.rs, 설정 창의 복사와 같은 길)를 쓴다: 사용자 동작 · 포커스 · 보안 문맥과 상관없이
// OS 클립보드에 쓴다. ⌘C(TSV)는 이 길이 아니다 — `copy` 이벤트의 `clipboardData.setData` 로 동작 안에서 곧바로 쓴다.

import { invoke } from "@tauri-apps/api/core";

export function writeClipboardText(text: string): Promise<void> {
  return invoke<void>("write_clipboard", { text });
}
