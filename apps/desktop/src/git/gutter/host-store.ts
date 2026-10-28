// 창의 거터 · blame 호스트를 거는 자리(git-gutter.tsx 가 건다). 텍스트 뷰어 · git 문서가 이것만 import 한다 — 호스트를 만드는 쪽
// (git-gutter.tsx)은 diff 뷰 · 에디터 탭까지 끌고 와서, 텍스트 뷰어가 그것을 import 하면 확장 목록(editor/extensions/index.ts)과
// 구문 강조(highlight.ts)의 순환을 먼저 밟아 목록에 빈 기능이 생긴다.

import { create } from "zustand";

import type { GutterHost } from "./gutter-host";

/** 이 창의 거터 · blame 호스트. 없으면(웰컴 창 · 시험) 거터와 blame 이 꺼진다. */
export const gitGutterHost = create<{ host: GutterHost | undefined }>(() => ({ host: undefined }));
