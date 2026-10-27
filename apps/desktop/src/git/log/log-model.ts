// Git 로그 툴 윈도우의 데이터(docs/design/13-m2-plan.md M2-14): 스트림 `git.log`(M2-03)를 받아 행을 쌓는다. 행은 React 상태가
// 아니라 이 객체 안의 배열이고(10-ui.md 「성능 규칙」), 화면은 `version` 이 바뀔 때만 다시 그린다. 레인은 lanes.ts 가 잇는다.
//
// - 질의(`LogQuery`)가 바뀌거나 `reload` 하면 도는 스트림을 끊고(`AbortController` — Host 의 걷기도 멈춘다) 처음부터 다시 받는다.
//   앞 스트림의 늦은 청크는 버린다.
// - 필터 바의 값을 RPC 의 `refs` · `filter` 로 옮기는 것은 `logParams` 하나다 — 필터 결과가 M2-03 과 같다는 것은 이 변환이 값을
//   그대로 넘긴다는 것이다(브랜치 밖의 필터가 켜지면 Host 가 그래프 없이 맞는 커밋만 보낸다, 결정 M2-D2).
// - 텍스트 칸이 해시 접두인지는 글자 모양만으로 정하지 않는다 — `added` · `2024` 같은 낱말도 16진이다. 16진 7자 이상이고 같은
//   브랜치 범위에 그 접두로 시작하는 커밋이 있을 때만(`git.logPage` 한 행으로 묻는다) 해시로, 아니면 메시지로 찾는다.

import type { GitLogParams, LogFilter, LogRow, RpcClient } from "@pitwall/rpc-client";

import { LaneIndex } from "./lanes";

/** 필터 바의 값. 빈 칸은 「거르지 않음」 이다. */
export interface LogQuery {
  /** 브랜치 필터(브랜치 패널 · 브랜치 ▾). 비면 모든 브랜치 · 원격 · 태그 · HEAD. */
  refs: readonly string[];
  /** 텍스트 칸. 해시 접두로 풀리면 해시(`isHashText` · `LogModel.load`), 아니면 메시지(`--grep`). */
  text: string;
  regex: boolean;
  ignoreCase: boolean;
  author: string;
  /** `YYYY-MM-DD`(그날 0시부터, 이 기기의 시간대). 비면 없음. */
  since: string;
  /** `YYYY-MM-DD`(그날 끝까지). */
  until: string;
  /** 저장소 꼭대기 기준 경로. 쉼표 · 줄바꿈으로 여럿, 쉼표가 든 경로는 큰따옴표로 감싼다(`splitPaths`). */
  paths: string;
}

export const EMPTY_QUERY: LogQuery = {
  refs: [],
  text: "",
  regex: false,
  ignoreCase: false,
  author: "",
  since: "",
  until: "",
  paths: "",
};

// 7자는 git 의 짧은 해시 기본 길이다. 64 는 SHA-256 저장소의 전체 해시.
const HASH = /^[0-9a-f]{7,64}$/i;

/** 텍스트 칸이 해시 접두일 수 있는가(모양만). 실제로 해시로 찾을지는 커밋이 있는지 물어 정한다(`LogModel.load`). */
export function isHashText(text: string): boolean {
  return HASH.test(text.trim());
}

/** `YYYY-MM-DD` 의 그날 시작(`end` 면 끝) 유닉스 초. 날짜가 아니면 `null`. */
export function dayToUnix(day: string, end: boolean): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day.trim());
  if (!m) return null;
  const date = end
    ? new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), 23, 59, 59)
    : new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), 0, 0, 0);
  return Number.isNaN(date.getTime()) ? null : Math.floor(date.getTime() / 1000);
}

/**
 * 경로 칸 → 경로 목록. 쉼표 · 줄바꿈으로 가르고 앞뒤 공백을 걷는다. 큰따옴표 안의 쉼표 · 공백은 경로의 글자다(`"a, b.txt", docs`
 * → `a, b.txt` · `docs`). 닫지 않은 따옴표는 끝까지 한 경로다. 빈 것은 뺀다.
 */
export function splitPaths(text: string): string[] {
  const paths: string[] = [];
  // 따옴표 밖 글자는 앞뒤 공백을 걷고, 따옴표 안 글자는 그대로 붙인다.
  let path = "";
  let loose = "";
  let quoted = false;
  const flushLoose = (trimEnd: boolean) => {
    const part = path === "" ? loose.trimStart() : loose;
    path += trimEnd ? part.trimEnd() : part;
    loose = "";
  };
  const end = () => {
    flushLoose(true);
    if (path !== "") paths.push(path);
    path = "";
  };
  for (const ch of text) {
    if (ch === '"') {
      if (!quoted) flushLoose(false);
      quoted = !quoted;
    } else if (quoted) path += ch;
    else if (ch === "," || ch === "\n") end();
    else loose += ch;
  }
  end();
  return paths;
}

/** 필터 바의 값 → `git.log` 의 `refs` · `filter`. `textAsHash` 면 텍스트 칸을 해시 접두로(모양이 해시일 때만). */
export function logParams(
  query: LogQuery,
  textAsHash = false,
): Pick<GitLogParams, "refs" | "filter"> & { filter: LogFilter } {
  const text = query.text.trim();
  const hash = textAsHash && isHashText(text);
  const paths = splitPaths(query.paths);
  return {
    refs: query.refs.length > 0 ? [...query.refs] : null,
    filter: {
      author: query.author.trim() || null,
      message: !hash && text !== "" ? text : null,
      regex: query.regex,
      ignoreCase: query.ignoreCase,
      since: dayToUnix(query.since, false),
      until: dayToUnix(query.until, true),
      paths,
      hash: hash ? text.toLowerCase() : null,
    },
  };
}

/** 브랜치 밖의 필터가 켜졌는가 — 그러면 Host 는 그래프 없이 맞는 커밋만 보낸다(결정 M2-D2). */
export function listFiltered(filter: LogFilter): boolean {
  return (
    !!filter.author ||
    !!filter.message ||
    filter.since != null ||
    filter.until != null ||
    (filter.paths?.length ?? 0) > 0 ||
    !!filter.hash
  );
}

export type LogStatus =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "done" }
  | { kind: "error"; message: string };

/** 로그를 부를 곳. */
export interface LogSource {
  client: RpcClient;
  projectId: string;
  /** 활성 워크트리의 꼭대기. 메인 체크아웃이면 없다. */
  worktree?: string;
}

export class LogModel {
  /** 받은 행(로그 차례). */
  rows: LogRow[] = [];
  lanes = new LaneIndex();
  /** 그래프 목록의 전체 행 수(첫 청크가 알린다). 걸러진 목록은 `null`. */
  total: number | null = null;
  status: LogStatus = { kind: "idle" };
  /** 그래프 없이 맞는 커밋만 받는 목록인가(브랜치 밖의 필터). */
  filtered = false;
  /** 텍스트 칸을 해시 접두로 찾는 중인가(상태 줄이 알린다). */
  textAsHash = false;
  private byHash = new Map<string, number>();
  private abort: AbortController | undefined;
  private listeners = new Set<() => void>();
  private version = 0;
  /** 첫 청크를 받은 뒤 한 번 부른다(성능 측정 · 시험). */
  onFirstChunk: (() => void) | undefined;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getVersion = (): number => this.version;

  /** 커밋의 행 번호. 받지 않았으면 -1. */
  indexOf(hash: string): number {
    return this.byHash.get(hash) ?? -1;
  }

  /** `source` 의 로그를 `query` 로 처음부터 받는다. 도는 것은 끊는다. */
  load(source: LogSource, query: LogQuery): Promise<void> {
    this.abort?.abort();
    const abort = new AbortController();
    this.abort = abort;
    this.reset();
    this.filtered = listFiltered(logParams(query).filter);
    this.status = { kind: "loading" };
    this.changed();
    return (async () => {
      let first = true;
      try {
        const textAsHash = await resolvesAsHash(source, query, abort.signal);
        if (abort.signal.aborted) return;
        const { refs, filter } = logParams(query, textAsHash);
        this.textAsHash = textAsHash;
        this.filtered = listFiltered(filter);
        const params: GitLogParams = {
          projectId: source.projectId,
          worktree: source.worktree ?? null,
          refs,
          filter,
          order: "topo",
        };
        for await (const chunk of source.client.stream("git.log", params, {
          signal: abort.signal,
        })) {
          if (abort.signal.aborted) return;
          if (first) this.total = chunk.total;
          this.append(chunk.rows);
          this.changed();
          if (first) {
            first = false;
            this.onFirstChunk?.();
          }
        }
        if (abort.signal.aborted) return;
        this.status = { kind: "done" };
      } catch (e) {
        if (abort.signal.aborted) return;
        this.status = { kind: "error", message: e instanceof Error ? e.message : String(e) };
      }
      this.changed();
    })();
  }

  /** 도는 스트림을 끊는다(툴 윈도우가 사라질 때). */
  dispose(): void {
    this.abort?.abort();
    this.abort = undefined;
  }

  private reset() {
    this.rows = [];
    this.lanes = new LaneIndex();
    this.byHash = new Map();
    this.total = null;
    this.filtered = false;
    this.textAsHash = false;
  }

  private append(rows: readonly LogRow[]) {
    for (const row of rows) {
      this.byHash.set(row.hash, this.rows.length);
      this.rows.push(row);
    }
    this.lanes.append(rows.map((r) => r.graph));
  }

  private changed() {
    this.version++;
    for (const l of this.listeners) l();
  }
}

/** 텍스트 칸이 해시 모양이고 `query.refs` 범위에 그 접두로 시작하는 커밋이 있는가. 묻지 못하면 거짓(메시지로 찾는다). */
async function resolvesAsHash(
  source: LogSource,
  query: LogQuery,
  signal: AbortSignal,
): Promise<boolean> {
  if (!isHashText(query.text)) return false;
  // 다른 필터는 빼고 브랜치 범위와 해시만 — 「그 커밋이 있는가」 만 묻는다.
  const { refs, filter } = logParams({ ...EMPTY_QUERY, refs: query.refs, text: query.text }, true);
  try {
    const page = await source.client.call(
      "git.logPage",
      {
        projectId: source.projectId,
        worktree: source.worktree ?? null,
        refs,
        filter,
        order: "topo",
        limit: 1,
      },
      { signal },
    );
    return page.rows.length > 0;
  } catch (e) {
    if (!signal.aborted) console.warn("[git] could not check the hash prefix", e);
    return false;
  }
}
