// 파일로 이동의 `search.files` 요청. 키 입력마다 요청하되 **한 번에 하나만** 보낸다 — 응답을 기다리는 동안 친 글자는 마지막 것만
// 남겨 두었다가 응답이 오면 보낸다. Host 의 동시 상한(crates/pw-fs/src/index/mod.rs `MAX_SEARCHES`, 넘으면 「바쁨」)을 한 창이 다
// 쓰지 않고, 늦게 온 옛 질의의 응답이 새 결과를 덮지 않는다. 결과는 늘 마지막으로 받은 질의의 것이다.

import type { FileMatch, SearchFilesParams } from "@pitwall/rpc-client";

/** 한 번에 받는 결과 수. 목록은 이만큼 보인다. */
export const FILE_LIMIT = 50;
/** 「바쁨」 이면 이만큼 뒤에 마지막 질의를 다시 보낸다. */
export const BUSY_RETRY_MS = 250;

export interface FileSearchState {
  /** 결과가 가리키는 질의. 아직 받은 것이 없으면 `undefined`. */
  readonly query?: string;
  readonly files: readonly FileMatch[];
  /** 맞은 수 · 목록의 파일 수(상한을 넘었는지 보인다). */
  readonly matched: number;
  readonly indexed: number;
  /** 마지막 질의의 응답을 기다린다. */
  readonly pending: boolean;
  /** 마지막 요청의 실패 문장. 다음 응답이 오면 지운다. */
  readonly error?: string;
  readonly busy: boolean;
}

export type FileSearchCall = (params: SearchFilesParams) => Promise<{
  files: FileMatch[];
  matched: number;
  indexed: number;
}>;

const EMPTY: FileSearchState = { files: [], matched: 0, indexed: 0, pending: false, busy: false };

function isBusy(e: unknown): boolean {
  const message = e instanceof Error ? e.message : (e as { message?: string } | null)?.message;
  return message?.startsWith("file search is busy") ?? false;
}

export class FileSearch {
  private state: FileSearchState = EMPTY;
  private readonly listeners = new Set<() => void>();
  /** 보낼 질의(마지막으로 받은 것). */
  private wanted?: { query: string; scope: Scope };
  private inFlight = false;
  private retry?: ReturnType<typeof setTimeout>;
  private disposed = false;
  /** `forget` 마다 오른다. 그 전에 보낸 요청의 답은 버린다. */
  private generation = 0;

  constructor(private readonly call: FileSearchCall) {}

  /** 지금 상태. React `useSyncExternalStore` 용. */
  get = (): FileSearchState => this.state;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** `query` 를 찾는다. 받은 결과가 이미 그 질의 · 범위의 것이면 다시 보내지 않는다. */
  request(query: string, scope: Scope) {
    const same =
      this.wanted?.query === query && sameScope(this.wanted.scope, scope) && !this.state.error;
    this.wanted = { query, scope };
    if (same) return;
    this.set({ pending: true });
    this.pump();
  }

  /** 받은 결과와 기다리는 질의를 버린다. 도는 요청의 답도 버리고, 다음 `request` 는 다시 보낸다. */
  forget() {
    this.generation++;
    clearTimeout(this.retry);
    this.retry = undefined;
    this.wanted = undefined;
    this.state = EMPTY;
  }

  dispose() {
    this.disposed = true;
    clearTimeout(this.retry);
    this.listeners.clear();
  }

  private pump() {
    if (this.inFlight || this.disposed || !this.wanted) return;
    const { query, scope } = this.wanted;
    const generation = this.generation;
    const stale = () => generation !== this.generation;
    this.inFlight = true;
    clearTimeout(this.retry);
    this.retry = undefined;
    this.call({ ...scope, query, limit: FILE_LIMIT })
      .then(
        (r) => {
          if (stale()) return;
          const latest = this.isWanted(query, scope);
          this.set({
            query,
            files: r.files,
            matched: r.matched,
            indexed: r.indexed,
            pending: !latest,
            error: undefined,
            busy: false,
          });
        },
        (e: unknown) => {
          if (stale()) return;
          const busy = isBusy(e);
          // 결과는 그대로 두고 문장만 보인다. 「바쁨」 이면 조금 뒤 마지막 질의를 다시 보낸다.
          this.set({ pending: busy, busy, error: e instanceof Error ? e.message : String(e) });
          if (busy && !this.disposed) {
            this.retry = setTimeout(() => {
              this.retry = undefined;
              this.pump();
            }, BUSY_RETRY_MS);
            return "retry" as const;
          }
          if (this.isWanted(query, scope)) this.wanted = undefined;
          return undefined;
        },
      )
      .then((outcome) => {
        this.inFlight = false;
        if (outcome === "retry" || !this.wanted || this.isWanted(query, scope)) return;
        this.pump();
      });
  }

  private isWanted(query: string, scope: Scope): boolean {
    return this.wanted?.query === query && sameScope(this.wanted.scope, scope);
  }

  private set(change: Partial<FileSearchState>) {
    if (this.disposed) return;
    this.state = { ...this.state, ...change };
    for (const l of this.listeners) l();
  }
}

/** 찾는 곳: 프로젝트와 창의 활성 워크트리 꼭대기. */
export type Scope = Pick<SearchFilesParams, "projectId" | "worktree">;

function sameScope(a: Scope, b: Scope): boolean {
  return a.projectId === b.projectId && (a.worktree ?? null) === (b.worktree ?? null);
}
