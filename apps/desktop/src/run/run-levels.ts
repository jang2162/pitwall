// 실행 콘솔의 로그 레벨 강조(17-m6-plan M6-06 ④, 메인 판정 M6-P14): 줄의 `ERROR` · `WARN` · `INFO` · `DEBUG` · `TRACE`(단어 경계,
// 대소문자 무시, 줄 앞 120자 안)를 xterm decoration 으로 칠한다. 프로그램이 이미 ANSI 색을 쓴 줄은 덮지 않는다 — 로거가 고른
// 색이 이긴다. 정규식 · 색은 설정이 아니다(사람의 정규식은 설정 화면이 든다 — 나중에 얹는다).
//
// 칠하는 것: ERROR · WARN 은 줄 전체의 글자색(빨강 · 노랑), INFO 는 그 낱말만(초록), DEBUG · TRACE 는 줄 전체를 흐리게(밝은
// 검정). 색은 터미널 테마의 ANSI 색이다 — 외관을 바꾸면 그 뒤에 칠하는 줄부터 따른다.
//
// 줄은 논리 줄이다(폭에 걸려 접힌 행을 이어 본다). 끝나지 않은 줄(커서가 있는 줄)은 칠하지 않고 기다린다 — 출력이 청크 가운데서
// 끊겨도 낱말을 반만 보고 판정하지 않게. 실행이 끝나면 마지막 줄까지 본다(`flush`). 대체 화면(TUI)은 칠하지 않는다.

import type { IBufferLine, IMarker, ITheme, Terminal } from "@xterm/xterm";

import { cellOffsets } from "../terminal/terminal-links";

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

/** 레벨 낱말을 찾는 앞부분의 길이(UTF-16 단위). 낱말이 이 안에서 끝나야 한다. */
export const LEVEL_WINDOW = 120;

const LEVEL = /\b(ERROR|WARN|INFO|DEBUG|TRACE)\b/gi;

export interface LevelMatch {
  level: LogLevel;
  /** 글자열 안의 위치(끝은 포함하지 않음). */
  start: number;
  end: number;
}

/** 줄의 레벨 — 앞 120자 안에서 끝나는 첫 낱말. 없으면 `undefined`. */
export function findLevel(text: string): LevelMatch | undefined {
  for (const m of text.matchAll(LEVEL)) {
    const end = m.index + m[0].length;
    if (end > LEVEL_WINDOW) return undefined;
    return { level: m[0].toLowerCase() as LogLevel, start: m.index, end };
  }
  return undefined;
}

/** 칠할 범위: 줄 전체인가, 낱말만인가. */
export function levelSpan(level: LogLevel): "line" | "word" {
  return level === "info" ? "word" : "line";
}

/** 이 칸들에 프로그램이 쓴 색(글자색 · 배경색 · 반전)이 있는가. */
export function hasAnsiColor(line: Pick<IBufferLine, "length" | "getCell">): boolean {
  for (let x = 0; x < line.length; x++) {
    const cell = line.getCell(x);
    if (!cell) continue;
    if (!cell.isFgDefault() || !cell.isBgDefault() || cell.isInverse()) return true;
  }
  return false;
}

const HEX = /^#[0-9a-fA-F]{6}/;

/** decoration 은 `#RRGGBB` 만 받는다. 테마 색이 그 꼴이 아니면(알파 · 이름) 앞 6자리 · 기본값으로. */
function hex(color: string | undefined, fallback: string): string {
  const m = color?.match(HEX);
  return m ? m[0] : fallback;
}

export function levelColor(level: LogLevel, theme: ITheme): string {
  switch (level) {
    case "error":
      return hex(theme.red, "#cd3131");
    case "warn":
      return hex(theme.yellow, "#b58900");
    case "info":
      return hex(theme.green, "#00a000");
    default:
      return hex(theme.brightBlack, "#808080");
  }
}

/** 논리 줄 하나: 첫 행의 절대 번호와 행들. */
interface LogicalLine {
  first: number;
  rows: IBufferLine[];
}

export class LevelHighlighter {
  /** 다음에 볼 행. 버퍼 앞이 잘려도 따라 움직이도록 marker 로 둔다. */
  private next: IMarker | undefined;
  private theme: ITheme = {};

  constructor(private readonly term: Terminal) {}

  setTheme(theme: ITheme): void {
    this.theme = theme;
  }

  /** 끝난 줄(커서가 있는 논리 줄 앞까지)을 칠한다. 출력을 쓴 뒤마다 부른다. */
  scan(): void {
    this.run(false);
  }

  /** 마지막 줄까지 칠한다(실행이 끝났다). */
  flush(): void {
    this.run(true);
  }

  /** 화면을 처음부터 다시(다시 실행 · 지우기 뒤). */
  reset(): void {
    this.next?.dispose();
    this.next = undefined;
  }

  private run(all: boolean) {
    const buffer = this.term.buffer.active;
    if (buffer.type !== "normal") return;
    const cursor = buffer.baseY + buffer.cursorY;
    // 표시가 잘려 나갔으면(스크롤백 상한 · 지우기) 그 앞도 모두 사라졌다 — 남은 처음부터.
    let row = this.next && !this.next.isDisposed ? Math.max(this.next.line, 0) : 0;
    // 커서가 있는 논리 줄의 첫 행 — 끝나지 않았으니 그 앞까지만.
    let stop = cursor;
    while (stop > 0 && buffer.getLine(stop)?.isWrapped) stop--;
    const limit = all ? buffer.length : stop;
    while (row < limit) {
      const line = this.logical(row, all ? buffer.length : stop);
      if (!line) break;
      this.paint(line, cursor);
      row = line.first + line.rows.length;
    }
    this.next?.dispose();
    this.next = this.term.registerMarker(row - cursor);
  }

  private logical(first: number, limit: number): LogicalLine | undefined {
    const buffer = this.term.buffer.active;
    const head = buffer.getLine(first);
    if (!head) return undefined;
    const rows = [head];
    for (let y = first + 1; y < limit; y++) {
      const line = buffer.getLine(y);
      if (!line?.isWrapped) break;
      rows.push(line);
    }
    return { first, rows };
  }

  private paint({ first, rows }: LogicalLine, cursor: number) {
    if (rows.some(hasAnsiColor)) return;
    // 행마다 글자열과 칸 위치. 접힌 행은 끝 공백도 글자다.
    const texts = rows.map((r, i) => r.translateToString(i === rows.length - 1));
    const match = findLevel(texts.join(""));
    if (!match) return;
    const color = levelColor(match.level, this.theme);
    if (levelSpan(match.level) === "line") {
      const marker = this.term.registerMarker(first - cursor);
      this.term.registerDecoration({
        marker,
        x: 0,
        width: this.term.cols,
        height: rows.length,
        foregroundColor: color,
        layer: "bottom",
      });
      return;
    }
    // 낱말만: 그 낱말이 든 행(들)에.
    let base = 0;
    rows.forEach((row, i) => {
      const text = texts[i] ?? "";
      const from = Math.max(match.start - base, 0);
      const to = Math.min(match.end - base, text.length);
      base += text.length;
      if (from >= to) return;
      const offsets = cellOffsets(row);
      const x = offsets[from] ?? 0;
      const width = (offsets[to] ?? row.length) - x;
      if (width <= 0) return;
      const marker = this.term.registerMarker(first + i - cursor);
      this.term.registerDecoration({ marker, x, width, foregroundColor: color, layer: "bottom" });
    });
  }
}
