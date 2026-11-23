// 다른 머신에 보내는 메시지 하나의 상한(18-m7-plan M7-13 · M7-13b). 다른 머신의 RPC 는 링크 틀 하나에 실리고, 틀 상한
// (`pw_wire::MAX_BODY_BYTES` 64 MiB)을 넘는 메시지는 그 **채널을 닫는다**(crates/pw-link/src/mux.rs) — 그 머신의 멤버 연결 전체가
// 끊겨 그 머신의 다른 탭 · 터미널까지 끊긴다. 다른 머신 문서의 통째 읽기 · 큰 쓰기는 조각으로 한다(editor/link-chunks.ts).
// Rust 의 정본이 없는 값이라 limits.ts(pw-fs 와 견주는 기준값)와 따로 둔다.

const MIB = 1024 * 1024;

/** 링크 틀 본문의 상한(`pw_wire::MAX_BODY_BYTES`). */
export const LINK_FRAME_BYTES = 64 * MIB;
/** 틀 머리(`pw_link::LinkFrame` 의 채널 · 종류 칸)와 JSON-RPC 봉투(`jsonrpc` · `id` · 응답의 `path` · 판정 · `version`)의 몫. */
const FRAME_HEADROOM = 64 * 1024;

/**
 * 다른 머신에 보내는 요청 하나의 상한(보낼 JSON-RPC 요청의 UTF-8 바이트 — 보내기 직전에 [`requestBytes`] 로 잰다). 틀 상한에서 틀
 * 머리 몫만 뺀다.
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
