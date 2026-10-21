// 전체에서 찾기(docs/design/12-m1-plan.md M1-20, F3-4)의 검색 한 번. 화면(find-popup.tsx · search-results.tsx)은 이것을 구독해 그린다.
//
// - Host 의 `search.text` 스트림(crates/pw-fs/src/search.rs)을 열고 청크를 받는 대로 쌓는다. 파일은 도착한 순서다 — Host 가 병렬로
//   찾아 순서를 정하지 않고, 받는 동안 줄의 번호가 바뀌지 않아야 고른 줄이 제자리에 있다.
// - 새 검색을 시작하거나 `cancel` 하면 이전 스트림을 곧바로 닫는다(`stream.cancel` 이 Host 에 간다 — Host 는 그 검색의 걷기 ·
//   읽기를 멈춘다). 닫힌 스트림의 늦은 청크는 쌓지 않는다.
// - 결과 상한(Host 기본 1만 줄, `DEFAULT_MAX_RESULTS`)을 넘으면 마지막 청크의 `truncated` 가 켜진다 — 화면의 「더 있음」.

import {
  isBusyError,
  type SearchFile,
  type SearchLine,
  type SearchTextChunk,
  type SearchTextParams,
} from "@pitwall/rpc-client";

import { isWithin, joinPath, separatorOf } from "../file-tree/tree-model";

/** 찾기 칸의 옵션(팝업 · 툴 윈도우가 같이 쓴다). */
export interface FindOptions {
  query: string;
  regex: boolean;
  matchCase: boolean;
  wholeWord: boolean;
  /** 파일 마스크. 쉼표로 나눈 글롭, `!` 로 시작하면 뺀다(`*.rs, !*_test.rs`). */
  mask: string;
  /** 벤더 · 생성 파일을 뺀다(M1-02 판정). */
  excludeVendored: boolean;
  /** 프로젝트 전체, 또는 그 아래 폴더 하나(절대 경로). */
  scope: FindScope;
}

/**
 * `root` 는 `path` 를 고를 때의 트리 루트다. 워크트리가 바뀌어 루트가 달라지면 같은 상대 위치를 새 루트 아래에서 찾는다
 * (`resolveFolder`) — 옛 절대 경로가 다른 체크아웃을 조용히 찾지 않게.
 */
export type FindScope = { kind: "project" } | { kind: "folder"; path: string; root?: string };

export const DEFAULT_OPTIONS: FindOptions = {
  query: "",
  regex: false,
  matchCase: false,
  wholeWord: false,
  mask: "",
  excludeVendored: false,
  scope: { kind: "project" },
};

/** 폴더 범위의 경로를 지금 루트 기준으로 푼다. 고른 루트 밖(절대 경로를 직접 친 것)은 그대로다. */
export function resolveFolder(scope: { path: string; root?: string }, root: string): string {
  if (scope.root === undefined || scope.root === root || !isWithin(scope.path, scope.root))
    return scope.path;
  const sep = separatorOf(scope.root);
  const rest = scope.path.slice(scope.root.length).replace(/^[/\\]+/, "");
  return rest === "" ? root : joinPath(root, rest, separatorOf(root) || sep);
}

/** 파일 마스크 → `include` · `exclude` 글롭. 빈 조각은 버린다. */
export function parseMask(mask: string): { include: string[]; exclude: string[] } {
  const include: string[] = [];
  const exclude: string[] = [];
  for (const part of mask.split(",")) {
    const glob = part.trim();
    if (glob.startsWith("!")) {
      if (glob.length > 1) exclude.push(glob.slice(1));
    } else if (glob) include.push(glob);
  }
  return { include, exclude };
}

/** 찾을 곳: 창의 프로젝트 · 활성 워크트리와 그 안의 프로젝트 루트(파일 트리의 루트와 같은 계산, file-tree/project-view.tsx). */
export interface SearchPlace {
  projectId: string;
  /** 활성 워크트리의 꼭대기. 메인 체크아웃이면 없다. */
  worktree: string | undefined;
  root: string;
}

/**
 * 옵션 → `search.text` 인자. ignore 는 늘 존중하고 숨김 파일은 찾지 않는다(`rg` 의 기본과 같다). 결과 상한은 Host 의 기본값이다.
 * 빈 질의는 Host 가 거절하므로 부르기 전에 거른다(`canSearch`).
 */
export function searchParams(options: FindOptions, place: SearchPlace): SearchTextParams {
  const { include, exclude } = parseMask(options.mask);
  return {
    projectId: place.projectId,
    worktree: place.worktree ?? null,
    paths: [
      options.scope.kind === "folder" ? resolveFolder(options.scope, place.root) : place.root,
    ],
    query: options.query,
    regex: options.regex,
    matchCase: options.matchCase,
    wholeWord: options.wholeWord,
    include,
    exclude,
    respectIgnore: true,
    excludeVendored: options.excludeVendored,
    excludeGenerated: options.excludeVendored,
    hidden: false,
  };
}

export function canSearch(options: FindOptions): boolean {
  return options.query.length > 0;
}

/** 목록의 한 줄: 파일과 그 안의 일치한 줄. */
export interface FindRow {
  file: SearchFile;
  line: SearchLine;
}

export type FindStatus = "idle" | "searching" | "done" | "cancelled" | "failed";

export interface FindSnapshot {
  readonly status: FindStatus;
  /** 지금 결과를 낸 검색의 인자. 결과에서 파일을 열 때 워크트리를 함께 싣는다. */
  readonly params: SearchTextParams | undefined;
  readonly files: readonly SearchFile[];
  /** 파일 순서대로 펼친 줄. */
  readonly rows: readonly FindRow[];
  /** 상한을 넘는 결과가 더 있다. */
  readonly truncated: boolean;
  readonly error: string | undefined;
  /** 실패가 Host 의 동시 검색 상한(「바쁨」, crates/pw-fs/src/search.rs)이다. 판정은 오류 객체로 한다(rpc-client `isBusyError`). */
  readonly busy: boolean;
}

export type SearchRunner = (
  params: SearchTextParams,
  signal: AbortSignal,
) => AsyncIterable<SearchTextChunk>;

const IDLE: FindSnapshot = {
  status: "idle",
  params: undefined,
  files: [],
  rows: [],
  truncated: false,
  error: undefined,
  busy: false,
};

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export class FindSession {
  private snapshot: FindSnapshot = IDLE;
  private abort: AbortController | undefined;
  /** `interrupt` 로 닫았고 새 검색이 아직 안 갔다. 그 사이에 고정 · 닫기가 오면 호출한 쪽이 이어 주어야 한다. */
  private due = false;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly run: SearchRunner) {}

  getSnapshot = (): FindSnapshot => this.snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private set(next: Partial<FindSnapshot>) {
    this.snapshot = { ...this.snapshot, ...next };
    for (const l of this.listeners) l();
  }

  /** 이전 검색을 닫고 `params` 로 새로 찾는다. 끝나면(다 받았거나 · 실패 · 취소) 돌아온다. */
  async start(params: SearchTextParams): Promise<void> {
    this.abort?.abort();
    this.due = false;
    const abort = new AbortController();
    this.abort = abort;
    const files: SearchFile[] = [];
    const rows: FindRow[] = [];
    this.set({
      status: "searching",
      params,
      files,
      rows,
      truncated: false,
      error: undefined,
      busy: false,
    });
    try {
      for await (const chunk of this.run(params, abort.signal)) {
        if (abort.signal.aborted) return;
        if (chunk.kind === "files") {
          for (const file of chunk.files) {
            files.push(file);
            for (const line of file.lines) rows.push({ file, line });
          }
          // 배열은 그대로 두고 스냅숏만 새로 만든다 — 구독하는 화면이 바뀐 것을 안다.
          this.set({ files, rows });
        } else {
          this.set({ status: "done", truncated: chunk.truncated });
        }
      }
      if (abort.signal.aborted) return;
      if (this.snapshot.status === "searching") this.set({ status: "done" });
    } catch (e) {
      if (!abort.signal.aborted)
        this.set({ status: "failed", error: errorText(e), busy: isBusyError(e) });
    } finally {
      if (this.abort === abort) this.abort = undefined;
    }
  }

  /** 도는 검색을 멈춘다. 받은 결과는 남는다. */
  cancel(): void {
    this.due = false;
    this.abort?.abort();
    this.abort = undefined;
    // 스트림이 없어도(`interrupt` 뒤 새 검색이 가기 전) 「찾는 중」 에 남지 않게 한다.
    if (this.snapshot.status === "searching") this.set({ status: "cancelled" });
  }

  /** 도는 검색을 닫되 상태는 그대로 둔다 — 곧 새 검색이 뒤따를 때(칸을 고치는 중). */
  interrupt(): void {
    this.abort?.abort();
    this.abort = undefined;
    this.due = true;
  }

  /** 닫았지만 새 검색이 아직 가지 않았다(`interrupt` 뒤 · `start`/`cancel`/`clear` 전). */
  get searchDue(): boolean {
    return this.due;
  }

  /** 검색을 닫고 결과를 비운다(빈 질의). */
  clear(): void {
    this.due = false;
    this.abort?.abort();
    this.abort = undefined;
    this.set(IDLE);
  }
}
