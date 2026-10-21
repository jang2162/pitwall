// 결과 한 줄의 미리보기를 일치 범위를 칠해 그린다(팝업 목록 · 「검색 결과」 툴 윈도우). 범위는 줄 기준 UTF-16 위치이고 미리보기는
// 줄의 `previewStart` 부터다(crates/pw-fs/src/search.rs `SearchLine`).

import type { SearchLine } from "@pitwall/rpc-client";

/** 일치 표시. 테마에 검색 전용 토큰이 없어 강조 색(`ui.accent`)을 옅게 깐다. 미리보기 에디터도 같은 색이다(find-preview.tsx). */
export const MATCH_CLASS =
  "rounded-[2px] bg-[color-mix(in_srgb,var(--pw-ui-accent)_30%,transparent)] text-ui-foreground";

/** 미리보기를 [일치 아님, 일치, …] 조각으로 나눈다. 앞의 공백은 깎는다(들여쓰기가 목록을 밀지 않게). */
export function matchSegments(line: SearchLine): { text: string; match: boolean }[] {
  const text = line.preview;
  const indent = text.length - text.trimStart().length;
  const out: { text: string; match: boolean }[] = [];
  let at = indent;
  for (const range of line.ranges) {
    const start = Math.max(range.start - line.previewStart, at);
    const end = Math.min(range.end - line.previewStart, text.length);
    if (end <= start) continue;
    if (start > at) out.push({ text: text.slice(at, start), match: false });
    out.push({ text: text.slice(start, end), match: true });
    at = end;
  }
  if (at < text.length) out.push({ text: text.slice(at), match: false });
  return out;
}

export function MatchText({ line }: { line: SearchLine }) {
  return (
    <span className="min-w-0 flex-1 truncate font-mono text-[12px] whitespace-pre">
      {line.previewStart > 0 && "…"}
      {matchSegments(line).map((s, i) =>
        s.match ? (
          <mark key={i} data-match="" className={MATCH_CLASS}>
            {s.text}
          </mark>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
      {line.more && "…"}
    </span>
  );
}

/** 경로의 마지막 이름. */
export function baseName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
}
