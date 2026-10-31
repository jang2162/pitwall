// Glide Data Grid 의 이미지 칸 오버레이 편집기가 import 하는 `react-responsive-carousel` 의 자리(M3-16, 메인 판정 2026-10-31). 앱의
// DB 셀은 늘 글자 칸이고 오버레이 편집기를 열지 않으므로 이 길에 닿지 않는다 — vite 설정(vite.config.ts 의 `resolve.alias`)이 이
// 파일로 바꾼다. 그려지면 조용히 넘어가지 않고 던진다.

export function Carousel(): never {
  throw new Error(
    "react-responsive-carousel is not bundled: the result grid never opens image editors",
  );
}
