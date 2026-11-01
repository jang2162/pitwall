// 셀 값 `DbValue`(14-m3-plan 결정 M3-D8)의 표시 · 정렬 · 필터 규칙(M3-16). 그리드 · 셀 상세 · 복사가 이것 하나를 쓴다.
//
// - 표시: 문자열로 온 큰 정수 · decimal 도 숫자처럼 오른쪽 정렬, NULL 은 `NULL` 을 흐린 색 + 기울임으로 그린다(색만으로 가르지
//   않는다 — 10-ui.md §6, 기울임은 glide-grid.tsx 가 정한다), 바이트는 `0x…`, Host 가 셀 상한에서 자른 값은 앞부분 + `… (원래
//   크기)`. 셀 한 칸에는 첫 줄의 앞부분만 그린다(`CELL_CHARS`) — 앞 `CELL_CHARS` 글자만 훑는다(1 MiB 셀을 그릴 때마다 통째로 훑지
//   않게). 보이지 않는 제어 · 방향 · 0폭 문자는 `⟨U+202E⟩` 꼴로 보인다(값이 다르게 읽히지 않게). 긴 글 · JSON 은 셀 상세 패널이
//   보인다.
// - 정렬: 숫자끼리는 값으로(문자열로 온 큰 정수 · decimal 은 10진 글 비교라 정밀도를 잃지 않는다), 그 밖은 글자로. NULL 은
//   늘 맨 뒤(오름 · 내림 모두).
// - 필터: 값 글의 앞 `FILTER_CHARS` 글자(줄바꿈 뒤 포함)에 대소문자를 가리지 않는 포함 검색. NULL 은 `NULL` 로 찾는다.

import type { DbValue } from "@pitwall/rpc-client";

/** 셀 한 칸에 그리는 글자 수 상한. 넘으면 앞부분 + `…`. */
export const CELL_CHARS = 500;
/** 필터가 보는 값 글의 앞부분(글자). 넘는 뒤로는 찾지 못한다 — 10만 행을 글자마다 거를 때 큰 셀이 시간을 다 쓰지 않게. */
export const FILTER_CHARS = 4096;

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GiB`;
}

// C0 · C1 제어(탭 · 줄바꿈 제외), 줄 · 단락 구분자(U+2028 · 2029), 그리고 Unicode 의 Default_Ignorable_Code_Point 전부 —
// 0폭 공백 · 결합자(ZWJ · ZWNJ 포함) · 단어 결합자 · BOM · 방향 표시 · 덮어쓰기 · 격리(U+200E · 200F · 202A~202E · 2066~2069 ·
// 061C) · 이형 선택자 · 한글 채움 문자 · 태그 문자(U+E0000 대) 등. 그려지지 않거나 글 방향 · 모양만 바꾸는 글자는 사람이 값을
// 확인하는 자리(셀 · 셀 상세 · 위험 확인 다이얼로그)에서 숨지 않는다. 예외는 이모지를 이루는 자리뿐이다(`isEmojiPart`).
// 속성 이스케이프는 엔진의 Unicode 판을 따른다.
const INVISIBLE =
  // oxlint-disable-next-line no-control-regex -- 제어 문자를 찾아 보이게 하는 것이 이 식의 목적이다.
  /[\p{Default_Ignorable_Code_Point}\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u2028\u2029]/gu;

const PICTOGRAPHIC = /^\p{Extended_Pictographic}$/u;
const ZWJ = 0x200d;
const VS15 = 0xfe0e;
const VS16 = 0xfe0f;
/** U+20E3 COMBINING ENCLOSING KEYCAP. */
const KEYCAP = 0x20e3;

const isPictographic = (cp: number | undefined) =>
  cp !== undefined && PICTOGRAPHIC.test(String.fromCodePoint(cp));
const isSkinTone = (cp: number | undefined) => cp !== undefined && cp >= 0x1f3fb && cp <= 0x1f3ff;
/** 키캡 바탕(`1️⃣` · `#️⃣` · `*️⃣`). */
const isKeycapBase = (cp: number | undefined) =>
  cp !== undefined && ((cp >= 0x30 && cp <= 0x39) || cp === 0x23 || cp === 0x2a);

/** `at` 바로 앞 코드 포인트와 그 시작 자리(서로게이트 쌍을 하나로). */
function before(s: string, at: number): { cp: number; start: number } | undefined {
  if (at <= 0) return undefined;
  const low = s.charCodeAt(at - 1);
  if (low >= 0xdc00 && low <= 0xdfff && at >= 2) {
    const high = s.charCodeAt(at - 2);
    if (high >= 0xd800 && high <= 0xdbff) return { cp: s.codePointAt(at - 2)!, start: at - 2 };
  }
  return { cp: low, start: at - 1 };
}

/**
 * 이모지를 이루는 보이지 않는 글자인가 — 데이터 도구에서 흔한 값(이름 · 채팅 글)의 이모지를 표지로 깨지 않는다(메인 판정).
 * - 이형 선택자 U+FE0E · FE0F: 바로 앞이 Extended_Pictographic 일 때(`❤️`). 키캡 바탕(`0-9 # *`) 뒤의 FE0F 는 바로 다음이
 *   U+20E3 일 때만(`1️⃣`) — 그렇지 않으면 숫자 하나로 그려져 `user:1️` 이 `user:1` 과 같아 보인다. 키캡 바탕 뒤의 FE0E 는 표지.
 * - ZWJ: Extended_Pictographic 두 글자 사이일 때(`👨‍👩‍👧`). 앞 글자 뒤의 FE0F · 피부색 수식은 건너뛰고 본다(`❤️‍🔥` · `👩🏽‍💻`).
 * 지역 표시 글자(🇰🇷) · 피부색 수식(🏽)은 보이는 글자라 처음부터 고르지 않는다. 그 밖의 ZWJ · 이형 선택자는 표지다.
 */
function isEmojiPart(s: string, c: string, at: number): boolean {
  const cp = c.codePointAt(0)!;
  if (cp === VS15 || cp === VS16) {
    const prev = before(s, at)?.cp;
    if (isPictographic(prev)) return true;
    return cp === VS16 && isKeycapBase(prev) && s.charCodeAt(at + 1) === KEYCAP;
  }
  if (cp !== ZWJ) return false;
  let prev = before(s, at);
  if (prev && (prev.cp === VS16 || isSkinTone(prev.cp))) prev = before(s, prev.start);
  return isPictographic(prev?.cp) && isPictographic(s.codePointAt(at + c.length));
}

export interface ShowInvisibleOptions {
  /**
   * 이모지를 이루는 ZWJ · 이형 선택자를 그대로 둔다(기본 `true` — 셀 · 셀 상세가 이름 · 채팅 글을 읽기 좋게). 위험 확인
   * 다이얼로그는 `false` 다 — 서버에 갈 것을 확인하는 마지막 자리라 그림이 바뀌지 않는 선택자(`©︎`), 합자가 없는 ZWJ(`🍎‍🍌`)로
   * 같아 보이는 다른 키도 드러낸다.
   */
  keepEmoji?: boolean;
}

/** 보이지 않는 제어 · 방향 · 0폭 문자를 `⟨U+XXXX⟩` 로 보인다(셀 · 셀 상세의 글 보기). 보조 평면 글자는 다섯 자리 이상이다. */
export function showInvisible(s: string, options: ShowInvisibleOptions = {}): string {
  const keepEmoji = options.keepEmoji ?? true;
  return s.replace(INVISIBLE, (c, at: number) =>
    keepEmoji && isEmojiPart(s, c, at)
      ? c
      : `⟨U+${c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}⟩`,
  );
}

/**
 * `showInvisible(raw)` 이 그린 글 `shown` 의 `[from, to)` 가 덮는 원래 글의 조각 — 고른 글을 복사할 때 표지 대신 원래 글자를
 * 넣는다(셀 상세). 표지는 일부만 덮여도 그 글자 하나 전부다. 그대로 둔 글자는 `shown` 에 같은 글로 있고 표지는 `⟨` 로
 * 시작하므로(표지가 되는 글자는 `⟨` 가 아니다) 옵션과 상관없이 맞대어 읽는다.
 */
export function originalSlice(raw: string, shown: string, from: number, to: number): string {
  if (from >= to) return "";
  let at = 0;
  let start: number | undefined;
  for (let i = 0; i < raw.length;) {
    const width = raw.codePointAt(i)! > 0xffff ? 2 : 1;
    if (at >= to) return start === undefined ? "" : raw.slice(start, i);
    const c = raw.slice(i, i + width);
    const drawn = shown.startsWith(c, at) ? width : shown.indexOf("⟩", at) + 1 - at;
    if (start === undefined && at + drawn > from) start = i;
    at += drawn;
    i += width;
  }
  return start === undefined ? "" : raw.slice(start);
}

export type ValueKind =
  | "null"
  | "bool"
  | "number"
  | "text"
  | "bytes"
  | "json"
  | "datetime"
  | "uuid"
  | "array"
  | "truncated"
  | "other";

export function valueKind(v: DbValue): ValueKind {
  if (v === "null") return "null";
  if (v.bool !== undefined) return "bool";
  if (v.int !== undefined || v.float !== undefined || v.decimal !== undefined) return "number";
  if (v.text !== undefined) return "text";
  if (v.bytes !== undefined) return "bytes";
  if (v.json !== undefined) return "json";
  if (v.datetime !== undefined) return "datetime";
  if (v.uuid !== undefined) return "uuid";
  if (v.array !== undefined) return "array";
  if (v.truncated !== undefined) return "truncated";
  return "other";
}

/** 오른쪽 정렬하는 값(숫자 — 문자열로 온 큰 정수 · decimal 포함). */
export function isNumeric(v: DbValue): boolean {
  return valueKind(v) === "number";
}

/**
 * 값의 글(잘리지 않은 전부). 복사 · 상세 · 필터가 쓴다. NULL 은 `null` 이다 — 표시(`NULL`)와 복사 규칙이 형식마다 다르다.
 */
export function valueText(v: DbValue): string | null {
  if (v === "null") return null;
  if (v.bool !== undefined) return v.bool ? "true" : "false";
  if (v.int !== undefined) return String(v.int);
  if (v.float !== undefined) return String(v.float);
  if (v.decimal !== undefined) return v.decimal;
  if (v.text !== undefined) return v.text;
  if (v.bytes !== undefined) return v.bytes.hex;
  if (v.json !== undefined) return v.json;
  if (v.datetime !== undefined) return v.datetime;
  if (v.uuid !== undefined) return v.uuid;
  if (v.array !== undefined) return arrayText(v.array);
  if (v.truncated !== undefined) return v.truncated.prefix;
  return v.other.text;
}

function arrayText(items: DbValue[]): string {
  return `{${items
    .map((item) => {
      const t = valueText(item);
      if (t === null) return "NULL";
      if (item !== "null" && item.array !== undefined) return t;
      return /^[^\s",{}\\]+$/.test(t) && t.toUpperCase() !== "NULL"
        ? t
        : `"${t.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
    })
    .join(",")}}`;
}

/** 값이 셀 상한에서 잘렸는가(원래 크기는 `originalSize`). */
export function isTruncated(v: DbValue): boolean {
  if (v === "null") return false;
  if (v.truncated !== undefined) return true;
  if (v.bytes !== undefined) return v.bytes.truncated;
  return false;
}

/** 잘린 값의 원래 크기(바이트). 잘리지 않았으면 `undefined`. */
export function originalSize(v: DbValue): number | undefined {
  if (v === "null") return undefined;
  if (v.truncated !== undefined) return v.truncated.size;
  if (v.bytes !== undefined && v.bytes.truncated) return v.bytes.size;
  return undefined;
}

/** 셀 한 칸에 그릴 글. NULL 은 `NULL`(흐린 색 · 기울임은 그리기가 정한다), 첫 줄의 앞부분만. 앞 `CELL_CHARS` 글자만 훑는다. */
export function displayText(v: DbValue): string {
  const t = valueText(v);
  if (t === null) return "NULL";
  // 그리는 것은 앞 `CELL_CHARS` 글자뿐이다 — 그 뒤는 줄바꿈을 찾지도 않는다.
  let line = t.length > CELL_CHARS ? t.slice(0, CELL_CHARS + 1) : t;
  const nl = line.search(/[\r\n]/);
  if (nl >= 0) line = `${line.slice(0, nl)} ⏎…`;
  if (line.length > CELL_CHARS) line = `${line.slice(0, CELL_CHARS)}…`;
  line = showInvisible(line);
  const size = originalSize(v);
  // Host 가 자른 값은 원래 크기를 붙인다 — 원래 `…` 로 끝나는 값과 갈린다.
  if (size !== undefined) line = `${line.endsWith("…") ? line : `${line}…`} (${formatSize(size)})`;
  return line;
}

/** 필터가 견주는 글: 값 글의 앞 `FILTER_CHARS` 글자(여러 줄 그대로), NULL 은 `NULL`. */
export function filterText(v: DbValue): string {
  const t = valueText(v);
  if (t === null) return "NULL";
  return t.length > FILTER_CHARS ? t.slice(0, FILTER_CHARS) : t;
}

// --- 정렬 ---------------------------------------------------------------------------------------------------------------

/** 10진 글(`-12.50`, `3e5` 는 아님)을 부호 · 정수부 · 소수부로. 숫자 모양이 아니면 `undefined`. */
function decimalParts(s: string): { neg: boolean; int: string; frac: string } | undefined {
  const m = /^([+-])?(\d*)(?:\.(\d*))?$/.exec(s.trim());
  if (!m || (m[2] === "" && (m[3] ?? "") === "")) return undefined;
  const int = (m[2] ?? "").replace(/^0+/, "");
  const frac = (m[3] ?? "").replace(/0+$/, "");
  const zero = int === "" && frac === "";
  return { neg: m[1] === "-" && !zero, int, frac };
}

function compareMagnitude(a: { int: string; frac: string }, b: { int: string; frac: string }) {
  if (a.int.length !== b.int.length) return a.int.length < b.int.length ? -1 : 1;
  if (a.int !== b.int) return a.int < b.int ? -1 : 1;
  const len = Math.max(a.frac.length, b.frac.length);
  const fa = a.frac.padEnd(len, "0");
  const fb = b.frac.padEnd(len, "0");
  return fa === fb ? 0 : fa < fb ? -1 : 1;
}

/** 숫자 값을 견줄 키: 유한한 JS 수면 수, 아니면 10진 글(정밀도를 잃지 않게), NaN · ±Infinity 는 그 이름. */
function numberKey(v: DbValue): number | string {
  if (v === "null") return Number.NaN;
  if (v.int !== undefined) return v.int;
  if (v.float !== undefined) return v.float;
  if (v.decimal !== undefined) return v.decimal;
  return Number.NaN;
}

/** 숫자 둘을 값으로 견준다. NaN 은 가장 크고, -Infinity < 유한 < Infinity. */
export function compareNumbers(a: DbValue, b: DbValue): number {
  const ka = numberKey(a);
  const kb = numberKey(b);
  const special = (k: number | string): number | undefined => {
    if (k === "NaN" || (typeof k === "number" && Number.isNaN(k))) return 2;
    if (k === "Infinity" || k === Number.POSITIVE_INFINITY) return 1;
    if (k === "-Infinity" || k === Number.NEGATIVE_INFINITY) return -1;
    return undefined;
  };
  const sa = special(ka);
  const sb = special(kb);
  if (sa !== undefined || sb !== undefined) return Math.sign((sa ?? 0) - (sb ?? 0));
  if (typeof ka === "number" && typeof kb === "number") return ka === kb ? 0 : ka < kb ? -1 : 1;
  // 한쪽이 정수가 아닌 JS 수(실수)면 수로 견준다 — 실수는 원래 근삿값이다.
  const inexact = (k: number | string) => typeof k === "number" && !Number.isInteger(k);
  if (inexact(ka) || inexact(kb)) {
    const na = Number(ka);
    const nb = Number(kb);
    return na === nb ? 0 : na < nb ? -1 : 1;
  }
  // 정수(±2^53 안은 지수 꼴 없이 찍힌다)와 10진 글 — 글로 견줘 정밀도를 잃지 않는다.
  const da = decimalParts(String(ka));
  const db = decimalParts(String(kb));
  if (!da || !db) return String(ka) < String(kb) ? -1 : String(ka) > String(kb) ? 1 : 0;
  if (da.neg !== db.neg) return da.neg ? -1 : 1;
  const m = compareMagnitude(da, db);
  return da.neg ? -m : m;
}

/**
 * 정렬 키. 숫자는 값 그대로(비교는 `compareNumbers`), 그 밖은 소문자로 접은 글(코드 포인트 순 — 10만 행에서 Intl.Collator 는
 * 초 단위로 느리다). NULL 은 `null`(늘 맨 뒤).
 */
export type SortKey = { kind: "number"; value: DbValue } | { kind: "text"; text: string } | null;

export function sortKey(v: DbValue): SortKey {
  if (v === "null") return null;
  if (isNumeric(v)) return { kind: "number", value: v };
  if (v.bool !== undefined) return { kind: "text", text: v.bool ? "1" : "0" };
  return { kind: "text", text: (valueText(v) ?? "").toLowerCase() };
}

/** 오름차순 비교. NULL 은 늘 뒤다 — 내림차순이어도(호출자가 `nullsLast` 로 뒤집지 않는다). */
export function compareKeys(a: SortKey, b: SortKey): number {
  if (a === null || b === null) return a === b ? 0 : a === null ? 1 : -1;
  if (a.kind === "number" && b.kind === "number") return compareNumbers(a.value, b.value);
  // 숫자와 글이 섞인 열(SQLite)은 숫자가 앞.
  if (a.kind !== b.kind) return a.kind === "number" ? -1 : 1;
  const ta = (a as { text: string }).text;
  const tb = (b as { text: string }).text;
  return ta === tb ? 0 : ta < tb ? -1 : 1;
}
