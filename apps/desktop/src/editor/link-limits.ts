// 다른 머신 문서의 한 번 읽기 · 쓰기 상한(18-m7-plan M7-13, 메인 판정 — 조각 읽기 · 쓰기는 M7-13b). 다른 머신의 RPC 는 링크
// 틀 하나에 실리고, 틀 상한(`pw_wire::MAX_BODY_BYTES` 64 MiB)을 넘는 메시지는 그 **채널을 닫는다**(crates/pw-link/src/mux.rs) —
// 그 머신의 멤버 연결 전체가 끊겨 그 머신의 다른 탭 · 터미널까지 끊긴다. 그래서 넘을 수 있는 것은 보내기 전에 · 받기 전에 막는다.
// Rust 의 정본이 없는 값이라 limits.ts(pw-fs 와 견주는 기준값)와 따로 둔다.

const MIB = 1024 * 1024;

/**
 * 다른 머신 파일을 통째로 읽는 상한(파일 바이트). 응답은 JSON 이다 — 텍스트는 이스케이프로 늘어나고(`"` · `\` · 줄바꿈 · 탭이
 * 두 글자가 된다 — 흔한 최악 2배), 바이트로 오면 base64 로 4/3 배다. 24 MiB × 2 = 48 MiB 에 응답 머리 · 판정을 더해도 64 MiB
 * 틀 안에 든다. 제어 문자투성이(`\u00XX` 6배)는 바이너리로 판정돼 통째로 읽지 않는다(hex 뷰어 — 페이지 읽기).
 */
export const REMOTE_WHOLE_READ = 24 * MIB;

/**
 * 다른 머신에 쓰는 내용의 상한(JSON 으로 보낼 내용의 UTF-8 바이트 — 보내기 전에 잰다). 읽을 수 있던 파일(`REMOTE_WHOLE_READ` 의
 * 흔한 최악 2배)은 그대로 다시 쓸 수 있게 48 MiB 이고, 요청 머리를 더해도 64 MiB 틀 안이다.
 */
export const REMOTE_WHOLE_WRITE = 48 * MIB;

/** 내용을 요청에 실었을 때의 바이트(JSON 이스케이프 포함). */
export function wireBytes(content: unknown): number {
  return new TextEncoder().encode(JSON.stringify(content)).length;
}
