// 파일 히스토리의 데이터(13-m2-plan M2-18): 스트림 `git.fileHistory`(M2-05)를 받아 행을 쌓는다. 행은 React 상태가 아니라 이 객체
// 안의 배열이고, 화면은 `version` 이 바뀔 때만 다시 그린다(Git 로그의 log-model.ts 와 같은 꼴).
//
// - 처음 받을 때는 청크가 오는 대로 보인다(첫 청크 = 화면 한 쪽, M2-05).
// - 오류로 끝나면 상태에 그 까닭(Host 오류의 `data`)을 둔다. 처음 받던 중이면 그때까지 온 행은 남긴다.
// - 다시 받을 때(HEAD 가 움직임 — `git.refsChanged`)는 끝까지 받은 뒤 한 번에 바꾼다. 받는 동안 옛 목록과 고른 행이 그대로라 목록이
//   비었다가 다시 차지 않는다. 다시 받다 실패하면 옛 목록을 두고 상태만 오류다.
// - `load` 를 다시 부르거나 `dispose` 하면 도는 스트림을 끊는다(`AbortController` — Host 의 걷기도 멈춘다).

import { type FileHistoryRow, type RpcClient, RpcCallError } from "@pitwall/rpc-client";

export type HistoryStatus =
  | { kind: "loading" }
  | { kind: "done" }
  /** `submodule` 이 있으면 그 서브모듈 안의 파일이라 Host 가 거절했다(`git.fileHistory` 의 `data.submodule`). */
  | { kind: "error"; message: string; submodule?: string };

/** 오류의 까닭: Host 오류는 `data`(글이면 그것, `{ detail }` 이면 그것)가 이유다 — 메시지(`invalid params`)만으로는 모른다. */
export function errorStatus(e: unknown): Extract<HistoryStatus, { kind: "error" }> {
  if (e instanceof RpcCallError) {
    const data = e.data;
    if (typeof data === "string") return { kind: "error", message: data };
    if (data && typeof data === "object" && !Array.isArray(data)) {
      const { detail, submodule } = data as { detail?: unknown; submodule?: unknown };
      return {
        kind: "error",
        message: typeof detail === "string" ? detail : e.message,
        ...(typeof submodule === "string" && { submodule }),
      };
    }
  }
  return { kind: "error", message: e instanceof Error ? e.message : String(e) };
}

/** 히스토리를 부를 곳. */
export interface HistorySource {
  client: RpcClient;
  projectId: string;
  /** 탭을 연 워크트리의 꼭대기. 메인 체크아웃이면 없다. */
  worktree?: string;
}

export class HistoryModel {
  /** 받은 행(새것부터 — `git log --follow` 의 차례). */
  rows: FileHistoryRow[] = [];
  status: HistoryStatus = { kind: "loading" };
  private abort: AbortController | undefined;
  private listeners = new Set<() => void>();
  private version = 0;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getVersion = (): number => this.version;

  /** `path`(저장소 꼭대기 기준)의 히스토리를 HEAD 부터 받는다. 도는 것은 끊는다. */
  load(source: HistorySource, path: string): Promise<void> {
    this.abort?.abort();
    const abort = new AbortController();
    this.abort = abort;
    // 처음이면 오는 대로 보이고, 다시 받는 것이면 다 받은 뒤 바꾼다.
    const progressive = this.rows.length === 0;
    const next: FileHistoryRow[] = [];
    this.status = { kind: "loading" };
    this.changed();
    return (async () => {
      try {
        const params = {
          projectId: source.projectId,
          worktree: source.worktree ?? null,
          path,
          rev: null,
        };
        for await (const chunk of source.client.stream("git.fileHistory", params, {
          signal: abort.signal,
        })) {
          if (abort.signal.aborted) return;
          next.push(...chunk.rows);
          if (progressive) {
            this.rows = next.slice();
            this.changed();
          }
        }
        if (abort.signal.aborted) return;
        this.rows = next;
        this.status = { kind: "done" };
      } catch (e) {
        if (abort.signal.aborted) return;
        // 처음 받던 중이면 그때까지 온 행은 이미 보이고 있다(UTF-8 이 아닌 경로가 나오면 Host 가 그 앞까지 보낸 뒤 끝낸다).
        this.status = errorStatus(e);
      }
      this.changed();
    })();
  }

  /** 도는 스트림을 끊는다(탭이 닫힐 때). */
  dispose(): void {
    this.abort?.abort();
    this.abort = undefined;
  }

  private changed() {
    this.version++;
    for (const l of this.listeners) l();
  }
}
