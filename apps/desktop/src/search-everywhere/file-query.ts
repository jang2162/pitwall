// 파일로 이동(⌘⇧O)의 입력 해석과 최근 파일의 일치 위치. 서버 매칭(crates/pw-fs/src/index/matching.rs)이 하지 않는 것만 여기 둔다.

/** 서버가 받는 질의 글자 수 상한(crates/pw-fs/src/index/mod.rs `MAX_QUERY_CHARS`). 넘는 질의는 거절되므로 잘라 보낸다. */
export const MAX_QUERY_CHARS = 256;

export interface FileQuery {
  /** 찾을 것(`:줄[:열]` 을 뗀 것). */
  readonly text: string;
  /** 1부터. */
  readonly line?: number;
  /** 1부터(글자 단위). */
  readonly column?: number;
}

/**
 * `main.rs:12` · `main.rs:12:5` 의 줄 · 열을 뗀다. 끝이 `:` 숫자가 아니면 그대로다(`a:b` 는 파일 이름일 수 있다). 줄 · 열은 1 이상만
 * 받는다 — `main.rs:0` 은 줄이 아니라 글자로 본다.
 */
export function parseFileQuery(input: string): FileQuery {
  const m = /^(.*?):(\d+)(?::(\d+))?\s*$/.exec(input);
  const line = m ? Number(m[2]) : 0;
  if (!m || line < 1) return { text: input.slice(0, MAX_QUERY_CHARS) };
  const column = m[3] === undefined ? undefined : Number(m[3]);
  return {
    text: m[1]!.slice(0, MAX_QUERY_CHARS),
    line,
    ...(column !== undefined && column >= 1 && { column }),
  };
}

function subsequence(text: string, term: string, offset: number): number[] | undefined {
  // 서버와 같이 대소문자는 질의에 대문자가 있을 때만 가린다.
  const fold = term === term.toLowerCase();
  const t = fold ? text.toLowerCase() : text;
  const out: number[] = [];
  let from = 0;
  for (const ch of term) {
    const at = t.indexOf(ch, from);
    if (at < 0) return undefined;
    out.push(offset + at);
    from = at + ch.length;
  }
  return out;
}

/**
 * 최근 파일(서버 결과에 없을 수 있다)이 질의에 맞는지와 그 일치 위치(`relative` 안의 UTF-16 위치, 오름차순). 낱말마다 파일 이름에서
 * 먼저, 안 되면 경로 전체에서 앞에서부터 차례로 맞춘다. 안 맞으면 `undefined`. 빈 질의는 맞고 위치가 없다.
 */
export function matchRecent(relative: string, query: string): number[] | undefined {
  const nameAt = relative.lastIndexOf("/") + 1;
  const name = relative.slice(nameAt);
  const positions = new Set<number>();
  for (const term of query.split(/\s+/).filter(Boolean)) {
    const hit = subsequence(name, term, nameAt) ?? subsequence(relative, term, 0);
    if (!hit) return undefined;
    for (const p of hit) positions.add(p);
  }
  return [...positions].sort((a, b) => a - b);
}
