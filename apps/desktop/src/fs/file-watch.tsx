// 파일 감시(docs/design/12-m1-plan.md M1-06): 프로젝트 창이 떠 있는 동안 `fs.watch` 스트림을 열어 둔다. 스트림이 열려 있는 동안 Host 가
// 창의 루트(활성 워크트리)를 감시해 바뀐 경로를 프로젝트 범위 이벤트 `fs.changed` 로 낸다 — 받는 쪽(파일 트리 · 열린 문서)은 그
// 토픽을 따로 구독한다. 창이 닫히면 연결과 함께 스트림이 닫혀 감시가 풀리고, 활성 워크트리가 바뀌면 닫고 새 워크트리로 다시 연다.
//
// 감시 한도를 넘었다는 경고는 Host 가 알림(`notification.show`)으로 한 번 낸다(notifications/notifications.tsx 의
// `HostNotifications`). 알림은 그 순간에 떠 있던 창에만 가므로, 폴링으로 보는 동안은 스트림의 감시 방식으로 상태바 항목을 둔다.

import type { FsWatching, RpcClient } from "@pitwall/rpc-client";
import { TriangleAlert } from "lucide-react";
import { useEffect } from "react";
import { createStore, useStore } from "zustand";

import { useProjectStrings } from "../layout/strings";
import { useActiveWorktree } from "../open-with/active-worktree";
import { useHost } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";

/**
 * `fs.watch` 를 열어 둔다. `onWatching` 은 감시를 건 뒤와 방식이 바뀔 때(한도 초과 → 폴링) 불린다. 스트림이 실패하면 경고만
 * 남긴다 — 그 창에 `fs.changed` 가 오지 않을 뿐이다. 돌려준 함수가 스트림을 닫는다.
 */
export function watchFiles(
  client: RpcClient,
  projectId: string,
  worktree: string | undefined,
  onWatching?: (watching: FsWatching) => void,
): () => void {
  const abort = new AbortController();
  void (async () => {
    try {
      for await (const watching of client.stream(
        "fs.watch",
        { projectId, worktree: worktree ?? null },
        { signal: abort.signal },
      )) {
        if (!abort.signal.aborted) onWatching?.(watching);
      }
    } catch (error) {
      if (!abort.signal.aborted) console.warn("[fs] the file watch stopped", error);
    }
  })();
  return () => abort.abort();
}

/** 이 창의 감시 중 폴링으로 보는 루트(OS 감시 한도 초과). 창마다 웹뷰가 따로라 모듈 상태로 둔다. */
const watchState = createStore<{ polling: readonly string[] }>()(() => ({ polling: [] }));

function reportWatching(watching: FsWatching | undefined): void {
  const polling = (watching?.roots ?? []).filter((r) => r.mode === "polling").map((r) => r.path);
  watchState.setState({ polling });
}

/** `client` 로 `projectId` 를 감시한다. 활성 워크트리가 바뀌면 닫고 다시 연다. */
export function useFileWatch(client: RpcClient | undefined, projectId: string): void {
  const worktree = useActiveWorktree();
  useEffect(() => {
    if (!client) return;
    const stop = watchFiles(client, projectId, worktree, reportWatching);
    return () => {
      stop();
      reportWatching(undefined);
    };
  }, [client, projectId, worktree]);
}

/** 상태바 항목: 폴링으로 보는 루트가 있을 때만 보인다(그 아래 변경이 폴링 간격만큼 늦다). */
export function FileWatchStatus() {
  const T = useProjectStrings().fileWatch;
  const polling = useStore(watchState, (s) => s.polling);
  if (polling.length === 0) return null;
  return (
    <span
      data-testid="status-file-watch"
      title={T.pollingHint(polling.join("\n"))}
      className="flex items-center gap-1"
    >
      <TriangleAlert size={11} aria-hidden />
      {T.polling}
    </span>
  );
}

/** 이 창의 프로젝트를 감시하고, 폴링으로 보는 동안 상태바에 알린다. `StatusBarProvider` 안에 둔다. */
export function FileWatch({ projectId }: { projectId: string }) {
  useFileWatch(useHost().client, projectId);
  useStatusBarItem({ id: "fileWatch", ...STATUS_SLOTS.fileWatch, component: FileWatchStatus });
  return null;
}
