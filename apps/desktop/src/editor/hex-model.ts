// hex 뷰어(hex-viewer.tsx)의 계산: 줄 서식, 화면에 필요한 페이지, 큰 파일의 스크롤 축척, 오프셋 입력. 화면 없이 시험한다.
//
// 한 줄 = 16바이트(결정 M1-D10). 바이트는 `HEX_PAGE_BYTES`(64 KiB) 페이지 단위로 읽고 페이지 경계는 그 크기의 배수다 — 화면에 보이는
// 줄이 걸친 페이지만 읽는다.
//
// 스크롤: 1 GB 파일은 6,700만 줄이라 줄마다 실제 높이를 주면 브라우저의 요소 높이 상한(수천만 px)을 넘는다. 그래서 스크롤 영역의
// 높이를 `MAX_SCROLL_PX` 로 자르고, 스크롤 위치를 첫 줄 번호로 비례해 바꾼다(`firstRowAt` · `scrollTopFor`). 자르지 않아도 되는
// 파일은 1:1 이다.

import { HEX_PAGE_BYTES } from "./limits";

export const BYTES_PER_ROW = 16;
/** 스크롤 영역 높이의 상한(px). WebKit · Chromium 의 요소 높이 상한보다 충분히 작다. */
export const MAX_SCROLL_PX = 8_000_000;

export function rowCount(size: number): number {
  return Math.ceil(size / BYTES_PER_ROW);
}

/** 줄 `[first, first + rows)` 이 걸친 페이지 번호들. 파일 끝을 넘는 줄은 없다. */
export function pagesFor(first: number, rows: number, size: number): number[] {
  const start = first * BYTES_PER_ROW;
  const end = Math.min(size, (first + rows) * BYTES_PER_ROW);
  if (end <= start) return [];
  const pages: number[] = [];
  for (let p = Math.floor(start / HEX_PAGE_BYTES); p * HEX_PAGE_BYTES < end; p++) pages.push(p);
  return pages;
}

/** 스크롤 영역의 높이(px). */
export function scrollHeight(size: number, rowHeight: number): number {
  return Math.min(rowCount(size) * rowHeight, MAX_SCROLL_PX);
}

/** 첫 줄로 둘 수 있는 가장 큰 줄 번호(마지막 줄이 화면 아래 끝에 온다). */
export function lastFirstRow(size: number, visibleRows: number): number {
  return Math.max(0, rowCount(size) - visibleRows);
}

/** 스크롤 위치 → 첫 줄. 축척이 1:1 이면 `scrollTop / rowHeight` 다. */
export function firstRowAt(
  scrollTop: number,
  size: number,
  rowHeight: number,
  viewport: number,
): number {
  const visible = Math.floor(viewport / rowHeight);
  const max = lastFirstRow(size, visible);
  const room = scrollHeight(size, rowHeight) - viewport;
  if (room <= 0) return 0;
  if (rowCount(size) * rowHeight <= MAX_SCROLL_PX)
    return Math.min(max, Math.floor(scrollTop / rowHeight));
  return Math.min(max, Math.round((scrollTop / room) * max));
}

/** 첫 줄 → 스크롤 위치(`firstRowAt` 의 역). */
export function scrollTopFor(
  first: number,
  size: number,
  rowHeight: number,
  viewport: number,
): number {
  const visible = Math.floor(viewport / rowHeight);
  const max = lastFirstRow(size, visible);
  const room = scrollHeight(size, rowHeight) - viewport;
  if (room <= 0 || max === 0) return 0;
  if (rowCount(size) * rowHeight <= MAX_SCROLL_PX) return first * rowHeight;
  return (Math.min(first, max) / max) * room;
}

/** 오프셋 열: 파일 크기에 맞춘 자릿수(최소 8)의 대문자 16진수. */
export function offsetDigits(size: number): number {
  return Math.max(8, Math.max(0, size - 1).toString(16).length);
}

export function formatOffset(offset: number, digits: number): string {
  return offset.toString(16).toUpperCase().padStart(digits, "0");
}

/** 한 줄의 바이트를 `"4F 6B …"`(8바이트마다 한 칸 더)로. 없는 바이트(파일 끝 · 아직 읽지 않음)는 `undefined`. */
export function formatHex(bytes: readonly (number | undefined)[]): string {
  let out = "";
  for (let i = 0; i < BYTES_PER_ROW; i++) {
    if (i > 0) out += i === 8 ? "  " : " ";
    const b = bytes[i];
    out += b === undefined ? "  " : b.toString(16).toUpperCase().padStart(2, "0");
  }
  return out;
}

/** 출력 가능한 ASCII 는 그대로, 나머지는 `.`. */
export function formatAscii(bytes: readonly (number | undefined)[]): string {
  let out = "";
  for (const b of bytes) {
    if (b === undefined) break;
    out += b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".";
  }
  return out;
}

/**
 * 오프셋 입력: 10진수(`4096`) 또는 16진수(`0x1000`, `1000h`). 앞뒤 공백 · 자릿수 구분 `_` 는 무시한다. 숫자가 아니면 `undefined`.
 * 파일 크기를 넘으면 마지막 바이트로 붙인다.
 */
export function parseOffset(input: string, size: number): number | undefined {
  const text = input.trim().replaceAll("_", "");
  let value: number;
  if (/^0x[0-9a-f]+$/i.test(text)) value = Number.parseInt(text.slice(2), 16);
  else if (/^[0-9a-f]+h$/i.test(text)) value = Number.parseInt(text.slice(0, -1), 16);
  else if (/^\d+$/.test(text)) value = Number(text);
  else return undefined;
  if (!Number.isSafeInteger(value)) return undefined;
  return Math.min(value, Math.max(0, size - 1));
}
