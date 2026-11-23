// 다른 머신 문서의 한 번 읽기 · 쓰기 상한(18-m7-plan M7-13, 메인 판정 — 조각 읽기 · 쓰기는 M7-13b). 다른 머신의 RPC 는 링크
// 틀 하나에 실리고, 틀 상한(`pw_wire::MAX_BODY_BYTES` 64 MiB)을 넘는 메시지는 그 **채널을 닫는다**(crates/pw-link/src/mux.rs) —
// 그 머신의 멤버 연결 전체가 끊겨 그 머신의 다른 탭 · 터미널까지 끊긴다. 그래서 넘을 수 있는 것은 보내기 전에 · 받기 전에 막는다.
// Rust 의 정본이 없는 값이라 limits.ts(pw-fs 와 견주는 기준값)와 따로 둔다.

const MIB = 1024 * 1024;

/** 링크 틀 본문의 상한(`pw_wire::MAX_BODY_BYTES`). */
export const LINK_FRAME_BYTES = 64 * MIB;
/** 틀 머리(`pw_link::LinkFrame` 의 채널 · 종류 칸)와 JSON-RPC 봉투(`jsonrpc` · `id` · 응답의 `path` · 판정 · `version`)의 몫. */
const FRAME_HEADROOM = 64 * 1024;

/**
 * 다른 머신 파일을 통째로 읽는 상한(파일 바이트 — 받기 전에는 응답 크기를 잴 수 없어 파일 크기로 막는다). 응답 JSON 은 파일
 * 바이트보다 **최대 6배** 커진다: 바이너리 판정은 앞 8 KiB 의 NUL 하나뿐이라(crates/pw-fs/src/judge.rs) NUL 이 없는 제어 문자
 * (0x01 등)나 8 KiB 뒤에만 NUL 이 있는 파일도 텍스트로 오고, U+0000 ~ U+001F 는 JSON 에서 `\u00XX` 6바이트가 된다(serde_json).
 * 다른 꼴은 더 작다 — 짐작한 인코딩으로 푼 글 · U+FFFD 로 푼 깨진 바이트는 3배까지, base64 는 4/3 배.
 * 10 MiB × 6 = 60 MiB 에 머리(`FRAME_HEADROOM`)를 더해도 64 MiB 틀 안이다.
 */
export const REMOTE_WHOLE_READ = 10 * MIB;

/**
 * 다른 머신에 보내는 요청 하나의 상한(보낼 JSON-RPC 요청의 UTF-8 바이트 — 보내기 직전에 [`requestBytes`] 로 잰다). 틀 상한에서 틀
 * 머리 몫만 뺀다. 통째로 읽을 수 있던 파일(`REMOTE_WHOLE_READ` × 최악 6배)은 그대로 다시 쓸 수 있다.
 */
export const REMOTE_MESSAGE_LIMIT = LINK_FRAME_BYTES - FRAME_HEADROOM;

/**
 * `method` 요청을 `params` 로 보낼 때의 바이트 — RpcClient 가 보내는 꼴(`{ jsonrpc, id, method, params }`)을 그대로 직렬화해 잰다.
 * `id` 는 가장 긴 자리 수로 둔다.
 */
export function requestBytes(method: string, params: unknown): number {
  const request = { jsonrpc: "2.0", id: Number.MAX_SAFE_INTEGER, method, params };
  return new TextEncoder().encode(JSON.stringify(request)).length;
}
