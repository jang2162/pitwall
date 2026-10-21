// 파일을 터미널로 드롭 → 경로 붙여넣기(10-ui.md 「5.7 실행 · 터미널」). 웹뷰의 HTML 드롭은 경로를 주지 않으므로 Tauri 의 네이티브
// 드롭 이벤트(경로 + 창 안 위치)를 받아, 위치가 보이는 터미널 위면 경로를 셸 문법으로 감싸 그 터미널에 붙여 넣는다.

import type { Platform } from "@pitwall/keymap";

// POSIX 셸에서 따옴표 없이 써도 한 낱말인 글자.
const POSIX_PLAIN = /^[A-Za-z0-9_\-./,:@%+=]+$/;

/** 셸 한 낱말로 감싼다. Windows 의 기본 셸은 PowerShell 이다(결정 M1-D14) — 작은따옴표 안에서 `'` 는 `''`. */
export function quoteForShell(path: string, platform: Platform): string {
  if (platform === "win")
    return /^[A-Za-z0-9_\-.\\/:]+$/.test(path) ? path : `'${path.replaceAll("'", "''")}'`;
  return POSIX_PLAIN.test(path) ? path : `'${path.replaceAll("'", `'\\''`)}'`;
}

/** 붙여 넣을 글자: 경로를 공백으로 잇고 끝에 공백 하나(이어서 칠 수 있게). */
export function dropText(paths: readonly string[], platform: Platform): string {
  return paths.length === 0 ? "" : `${paths.map((p) => quoteForShell(p, platform)).join(" ")} `;
}

/** 드롭 이벤트의 위치(물리 픽셀)가 요소의 사각형(CSS 픽셀) 안인가. */
export function dropHits(
  rect: Pick<DOMRect, "left" | "top" | "right" | "bottom">,
  position: { x: number; y: number },
  scale: number,
): boolean {
  const x = position.x / scale;
  const y = position.y / scale;
  return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
}
