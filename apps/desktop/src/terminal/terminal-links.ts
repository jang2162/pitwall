// 터미널 출력의 `파일:줄[:열]` → 에디터(10-ui.md 「5.7 실행 · 터미널」). 줄에서 경로처럼 보이는 낱말을 찾고(`findFileLinks`), 상대 경로는
// 셸을 띄운 폴더 기준으로 풀어, Host 에 실제로 있는 파일(`fs.stat`)만 링크로 내준다 — 없는 경로에 밑줄을 긋지 않는다. 열기는
// ⌘+클릭(macOS) · Ctrl+클릭(그 밖)이다. 셸 안에서 `cd` 한 뒤의 상대 경로는 셸이 처음 뜬 폴더 기준으로 풀린다(셸의 현재 폴더를
// 알려 주는 OSC 7 은 아직 읽지 않는다).

import type { IBufferLine, ILink, ILinkProvider, Terminal } from "@xterm/xterm";

export interface FileLinkMatch {
  /** 줄 글자열 안의 위치(UTF-16 단위, 끝은 포함하지 않음). */
  start: number;
  end: number;
  path: string;
  line?: number;
  column?: number;
}

// URL 은 링크 후보에서 뺀다(`https://a/b` 의 `//a/b` 가 경로로 읽히지 않게).
const URL = /\b[a-zA-Z][a-zA-Z0-9+.-]*:\/\/\S+/g;
// 경로 글자의 연속(+ 선택 `:줄` · `:줄:열`). Windows 드라이브 접두(`C:\`)를 받는다.
const CANDIDATE = /(?:[A-Za-z]:[\\/])?[^\s:"'`()<>[\]{}|,;]+(?::(\d+)(?::(\d+))?)?/g;
// 문장 끝의 마침표 등은 경로가 아니다(`see src/a.rs.`).
const TRAILING = /[.]+$/;

export function findFileLinks(text: string): FileLinkMatch[] {
  const masked = text.replace(URL, (url) => " ".repeat(url.length));
  const links: FileLinkMatch[] = [];
  for (const m of masked.matchAll(CANDIDATE)) {
    const whole = m[0];
    const lineText = m[1];
    const columnText = m[2];
    let path = whole.slice(0, whole.length - suffixLength(lineText, columnText));
    if (lineText === undefined) path = path.replace(TRAILING, "");
    if (!looksLikePath(path)) continue;
    const start = m.index;
    const end = lineText === undefined ? start + path.length : start + whole.length;
    links.push({
      start,
      end,
      path,
      ...(lineText !== undefined && { line: Number(lineText) }),
      ...(columnText !== undefined && { column: Number(columnText) }),
    });
  }
  return links;
}

function suffixLength(line: string | undefined, column: string | undefined): number {
  if (line === undefined) return 0;
  return 1 + line.length + (column === undefined ? 0 : 1 + column.length);
}

// 구분자가 있거나 확장자(글자가 하나는 든 — `3.14` 는 아니다)가 있어야 경로로 본다 — 낱말마다 Host 에 묻지 않게.
function looksLikePath(path: string): boolean {
  if (path === "" || /^[.]+$/.test(path) || /^\d+$/.test(path)) return false;
  return /[\\/]/.test(path) || /[^.\\/]\.[A-Za-z0-9]*[A-Za-z][A-Za-z0-9]*$/.test(path);
}

function isAbsolute(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}

/** `base`(절대 경로) 기준으로 `path` 를 푼다. `.` · `..` 는 글자로 정리한다 — Host 가 다시 정규화한다. 홈(`~`)은 풀지 않는다. */
export function resolvePath(base: string, path: string): string | undefined {
  if (path.startsWith("~")) return undefined;
  const windows = /^[A-Za-z]:[\\/]/.test(base) || base.startsWith("\\\\");
  const sep = windows ? "\\" : "/";
  const joined = isAbsolute(path) ? path : `${base}${sep}${path}`;
  const parts = joined.split(/[\\/]/);
  const head = parts.shift() ?? "";
  const out: string[] = [];
  for (const part of parts) {
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return `${head}${sep}${out.join(sep)}`;
}

/** 줄의 글자열 위치 → 칸(넓은 글자는 두 칸). 끝 위치 하나를 더 담는다. */
export function cellOffsets(line: Pick<IBufferLine, "length" | "getCell">): number[] {
  const offsets: number[] = [];
  for (let x = 0; x < line.length; x++) {
    const cell = line.getCell(x);
    if (!cell || cell.getWidth() === 0) continue;
    for (let i = 0; i < Math.max(1, cell.getChars().length); i++) offsets.push(x);
  }
  offsets.push(line.length);
  return offsets;
}

export interface FileLinkOptions {
  /** 상대 경로의 기준(셸이 뜬 폴더). 셸이 뜨기 전이면 `undefined`. */
  cwd(): string | undefined;
  /** 있는 파일인가(Host 의 `fs.stat`). */
  isFile(path: string): Promise<boolean>;
  /** 링크를 눌렀다. */
  open(target: { path: string; line?: number; column?: number }): void;
  /** 이 클릭이 링크를 여는가(⌘ · Ctrl). */
  opens(event: MouseEvent): boolean;
}

export function fileLinkProvider(term: Terminal, options: FileLinkOptions): ILinkProvider {
  return {
    provideLinks(y, callback) {
      const line = term.buffer.active.getLine(y - 1);
      const base = options.cwd();
      if (!line || base === undefined) return callback(undefined);
      const text = line.translateToString(true);
      const found = findFileLinks(text).flatMap((m) => {
        const path = resolvePath(base, m.path);
        return path ? [{ ...m, path }] : [];
      });
      if (found.length === 0) return callback(undefined);
      const offsets = cellOffsets(line);
      void Promise.all(found.map((m) => options.isFile(m.path).catch(() => false))).then((ok) => {
        const links: ILink[] = found
          .filter((_, i) => ok[i])
          .map((m) => ({
            range: {
              // 1부터 세고 끝을 포함한다 — 0부터 센 「끝 다음 칸」 과 같은 수다.
              start: { x: (offsets[m.start] ?? 0) + 1, y },
              end: { x: offsets[m.end] ?? line.length, y },
            },
            text: text.slice(m.start, m.end),
            activate: (event) => {
              if (options.opens(event))
                options.open({ path: m.path, line: m.line, column: m.column });
            },
          }));
        callback(links.length > 0 ? links : undefined);
      });
    },
  };
}
