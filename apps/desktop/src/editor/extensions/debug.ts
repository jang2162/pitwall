// 에디터의 브레이크포인트 거터 · 멈춘 줄(17-m6-plan M6-18). 구현은 src/debug/breakpoint-gutter.ts 다 — 이 모듈은 켜는 조건만 정한다:
// 창이 디버그 호스트를 넘긴 파일 탭(`context.debug`, 프로젝트 창). 그 밖의 문서(git · DB 문서 · 외부 소스 · 파일 창)는 끈다.

import { breakpointGutter } from "../../debug/breakpoint-gutter";
import type { EditorFeature } from ".";

export const debug: EditorFeature = {
  id: "debug",
  extension: (c) => (c.debug ? breakpointGutter(c.debug.host, c.path, c.debug.worktree) : []),
};
