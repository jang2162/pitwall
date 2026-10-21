import { expect, it } from "vitest";

import {
  firstRowAt,
  formatAscii,
  formatHex,
  formatOffset,
  lastFirstRow,
  MAX_SCROLL_PX,
  offsetDigits,
  pagesFor,
  parseOffset,
  rowCount,
  scrollHeight,
  scrollTopFor,
} from "./hex-model";
import { HEX_PAGE_BYTES } from "./limits";

const GIB = 1024 * 1024 * 1024;
const ROWS_PER_PAGE = HEX_PAGE_BYTES / 16;

it("formats a row as 16 hex bytes with a gap after eight, and printable ASCII", () => {
  const bytes = [...Buffer.from("Hello, hex view!")];
  expect(formatHex(bytes)).toBe("48 65 6C 6C 6F 2C 20 68  65 78 20 76 69 65 77 21");
  expect(formatAscii(bytes)).toBe("Hello, hex view!");
  // 출력할 수 없는 바이트는 점, 파일 끝(없는 바이트)은 빈칸.
  const tail = [0x00, 0x7f, 0x41, 0xff];
  expect(formatHex(tail)).toBe(`00 7F 41 FF${" ".repeat(12 * 3 + 1)}`);
  expect(formatAscii(tail)).toBe("..A.");
  expect(formatOffset(0x1f0, offsetDigits(100))).toBe("000001F0");
  expect(offsetDigits(GIB * 64)).toBe(9);
});

it("asks only for the pages the visible rows cross", () => {
  expect(pagesFor(0, 40, GIB)).toEqual([0]);
  // 페이지 경계에 걸친 화면은 두 장.
  expect(pagesFor(ROWS_PER_PAGE - 10, 40, GIB)).toEqual([0, 1]);
  // 1 GB 의 마지막 화면은 마지막 페이지 하나.
  const last = lastFirstRow(GIB, 40);
  expect(pagesFor(last, 40, GIB)).toEqual([GIB / HEX_PAGE_BYTES - 1]);
  // 파일 끝을 넘는 줄은 읽지 않는다.
  expect(pagesFor(0, 40, 100)).toEqual([0]);
  expect(pagesFor(10, 40, 100)).toEqual([]);
  expect(pagesFor(0, 40, 0)).toEqual([]);
});

it("scrolls 1:1 for small files and scales a 1 GB file into the scroll height cap", () => {
  const row = 18;
  const viewport = 400;
  // 작은 파일: 스크롤 px 그대로.
  expect(scrollHeight(16_000, row)).toBe(1000 * row);
  expect(firstRowAt(row * 30, 16_000, row, viewport)).toBe(30);
  expect(scrollTopFor(30, 16_000, row, viewport)).toBe(row * 30);

  // 1 GB: 높이는 상한, 맨 아래는 마지막 줄, 맨 위는 0 줄.
  expect(rowCount(GIB) * row).toBeGreaterThan(MAX_SCROLL_PX);
  expect(scrollHeight(GIB, row)).toBe(MAX_SCROLL_PX);
  const bottom = MAX_SCROLL_PX - viewport;
  expect(firstRowAt(bottom, GIB, row, viewport)).toBe(
    lastFirstRow(GIB, Math.floor(viewport / row)),
  );
  expect(firstRowAt(0, GIB, row, viewport)).toBe(0);
  // 왕복하면 같은 줄이다.
  for (const first of [0, 1, 12_345, 33_554_432, lastFirstRow(GIB, 22)])
    expect(firstRowAt(scrollTopFor(first, GIB, row, viewport), GIB, row, viewport)).toBe(first);
});

it("reads offsets in decimal or hex and clamps them to the last byte", () => {
  expect(parseOffset("4096", GIB)).toBe(4096);
  expect(parseOffset(" 0x1000 ", GIB)).toBe(4096);
  expect(parseOffset("0X1f", GIB)).toBe(31);
  expect(parseOffset("1000h", GIB)).toBe(4096);
  expect(parseOffset("1_000", GIB)).toBe(1000);
  expect(parseOffset("0x3FFFFFFF", GIB)).toBe(GIB - 1);
  expect(parseOffset("99999999999", GIB)).toBe(GIB - 1);
  for (const bad of ["", "-1", "0x", "12k", "1.5", "0xZZ"])
    expect(parseOffset(bad, GIB)).toBeUndefined();
});
