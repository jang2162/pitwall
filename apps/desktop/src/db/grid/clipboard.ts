// 「다른 형식으로 복사」 의 클립보드 쓰기(M3-16 리뷰). 글은 비동기로 만들어진다 — INSERT 는 Host 왕복(`db.sql.insertText`) 뒤다.
// 웹뷰의 `navigator.clipboard.writeText` 는 WebKit 에서 사용자 동작(클릭 · 키) 안에서만 쓰기를 허락하고 `await` 뒤는 그 밖이라
// 거절될 수 있다. 사용자 동작 안에서 `ClipboardItem` 에 Promise 를 넘기는 길도 있지만, 그것은 웹뷰의 보안 문맥(`tauri://` ·
// 개발 서버) · 플랫폼 웹뷰(WKWebView · WebKitGTK · WebView2)마다 조건이 달라 확실하지 않다. 그래서 Host 의 클립보드 쓰기
// (`write_clipboard` — src-tauri/src/clipboard.rs, 설정 창의 복사와 같은 길)를 쓴다: 사용자 동작 · 포커스 · 보안 문맥과 상관없이
// OS 클립보드에 쓴다. ⌘C(TSV)는 이 길이 아니다 — `copy` 이벤트의 `clipboardData.setData` 로 동작 안에서 곧바로 쓴다.
//
// Host 는 한 번에 쓰는 글에 크기 상한을 둔다(clipboard.rs 의 `MAX_CLIPBOARD_BYTES` — 그 값의 근거도 거기). 넘으면
// `ClipboardTooLarge` 로 던진다 — 부르는 쪽이 번역 문구로 알린다.

import { invoke } from "@tauri-apps/api/core";

/** Host 가 클립보드 상한을 넘는 글을 거절했다(`code: "tooLarge"`). */
export class ClipboardTooLarge extends Error {
  constructor(
    readonly maxBytes: number,
    message: string,
  ) {
    super(message);
  }
}

/** Host 의 거절(`{ code, message, … }`)을 `Error` 로 — 상한이면 `ClipboardTooLarge`. */
function refusal(e: unknown): unknown {
  if (typeof e !== "object" || e === null) return e;
  const { code, maxBytes, message } = e as Record<string, unknown>;
  if (typeof message !== "string") return e;
  if (code === "tooLarge" && typeof maxBytes === "number")
    return new ClipboardTooLarge(maxBytes, message);
  return new Error(message);
}

export async function writeClipboardText(text: string): Promise<void> {
  try {
    await invoke<void>("write_clipboard", { text });
  } catch (e) {
    throw refusal(e);
  }
}
