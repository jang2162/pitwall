// Glide Data Grid 의 markdown 칸(markdown-div — `marked` 의 HTML 을 innerHTML 로 넣는다)이 import 하는 `marked` 의 자리(M3-16, 메인
// 판정 2026-10-31). 앱의 DB 셀은 늘 글자 칸이라 이 길에 닿지 않는다 — markdown → HTML 코드를 앱에 싣지 않으려고 vite 설정
// (vite.config.ts 의 `resolve.alias`)이 `marked` 를 이 파일로 바꾼다. 불리면 조용히 넘어가지 않고 던진다.

export function marked(): never {
  throw new Error("marked is not bundled: the result grid never renders markdown cells");
}
