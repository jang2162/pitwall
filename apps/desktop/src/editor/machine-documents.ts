// 다른 머신 문서의 감시 · 끊김 · 다시 붙기(18-m7-plan M7-13, F9-8 — 결정 M7-D8 ①). 창의 체크아웃별 연결(checkout/checkout-hosts.tsx,
// M7-11)과 문서 모델(documents.ts)을 잇는다. 이 머신의 문서는 창의 연결로 따로 맞춘다(layout/project-window.tsx 의
// `useDocumentSync`) — 여기서는 다른 머신만 본다.
//
// - **끊김**: 그 머신의 연결이 쓸 수 없게 되면(링크가 `connected` 가 아님 — 다시 붙는 중 · 끊김 · 머신 다름 — 이거나 멤버 연결이
//   없음) 그 머신의 문서를 `documents.disconnect` 로 알린다. 문서는 남고 편집 · 저장이 막힌다. 「다시 붙는 중」 동안 멤버 연결이
//   살아 있어도(M7-09 — 채널 · 스트림은 시한 안에서 그대로) 끊김으로 본다: 그 사이의 저장은 언제 닿을지 모른다.
// - **다시 붙기**: 끊겼던 머신이 다시 쓸 수 있게 되면 `documents.reconnect` — 열린 문서마다 디스크와 맞춘다(끊긴 동안 놓친
//   `fs.changed` 를 다시 보는 것). 처음 붙는 것(끊긴 적 없음)에는 하지 않는다. 다만 실앱의 `retain` 은 연결 없이 먼저 알리므로
//   (checkout-hosts.tsx) 첫 붙기도 대개 끊김 → 다시 붙기를 지난다 — 그때 문서는 아직 읽는 중이라 표시가 그려지지 않고 맞추기는
//   읽는 중인 문서를 건너뛴다(늘어나는 것은 기껏 `fs.stat` 한 번).
// - **감시**: 그 머신의 멤버 연결이 있는 동안 문서가 열린 워크트리마다 `fs.watch` 를 열어 두고, 그 연결로 그 머신 프로젝트의
//   `fs.changed` 를 구독한다. 이벤트의 경로는 그 머신의 경로라 머신을 붙여(`qualify`) 문서 모델에 넘긴다 — 이 머신의 같은 경로
//   문서에 닿지 않는다. 연결이 바뀌면(정말 끊겼다가 새 멤버 연결) 감시 · 구독을 새 연결로 다시 연다.

import type { RpcClient } from "@pitwall/rpc-client";

import { checkoutOfKey, LOCAL_MACHINE, machineOf, qualify } from "../checkout/checkout";
import type { CheckoutHosts } from "../checkout/checkout-hosts";
import { watchFiles } from "../fs/file-watch";
import { type DocumentStore, FS_CHANGED_TOPIC, type FsChangedEvent } from "./documents";

const LOG = "[editor]";

interface Bound {
  client: RpcClient;
  projectId: string;
  /** 워크트리(그 머신의 경로, 메인 체크아웃은 `null`) → 감시 닫기. */
  watches: Map<string | null, () => void>;
  unsubscribe?: () => Promise<void>;
  disposed: boolean;
}

/** 그 머신의 이벤트를 창 안의 꼴로 — 경로에 머신을 붙인다. */
export function qualifyChanged(machine: string, event: FsChangedEvent): FsChangedEvent {
  return {
    ...event,
    root: qualify(machine, event.root),
    worktree: event.worktree === null ? null : qualify(machine, event.worktree),
    paths: event.paths.map((p) => qualify(machine, p)),
  };
}

/**
 * 다른 머신 문서의 감시 · 끊김 · 다시 붙기를 시작한다(머리 주석). `watch` 는 시험이 바꾼다. 돌려준 함수가 감시 · 구독을 모두 닫는다
 * — 끊김 표시는 문서 모델에 남는다(창이 닫히는 때다).
 */
export function startMachineDocumentSync(
  hosts: CheckoutHosts,
  documents: DocumentStore,
  watch: typeof watchFiles = watchFiles,
): () => void {
  /** 머신 → 지금 쓸 수 있는가(마지막으로 본 값). */
  const online = new Map<string, boolean>();
  /** 끊긴 채인 머신 — 쓰는 것이 없어져도 기억한다(다시 쓰기 시작해 붙으면 맞춘다). */
  const offline = new Set<string>();
  const bound = new Map<string, Bound>();
  let stopped = false;

  const belongs = (machine: string) => (path: string, worktree: string | undefined) =>
    machineOf(path, worktree) === machine;

  function unbind(machine: string) {
    const b = bound.get(machine);
    if (!b) return;
    bound.delete(machine);
    b.disposed = true;
    for (const stop of b.watches.values()) stop();
    void b.unsubscribe?.().catch(() => {});
  }

  function bind(machine: string, client: RpcClient, projectId: string): Bound {
    const b: Bound = { client, projectId, watches: new Map(), disposed: false };
    bound.set(machine, b);
    client
      .subscribe({ projectId, topics: [FS_CHANGED_TOPIC] }, (event) => {
        if (b.disposed || event.topic !== FS_CHANGED_TOPIC) return;
        const changed = qualifyChanged(machine, event.payload as unknown as FsChangedEvent);
        void documents.changed(changed);
      })
      .then((subscription) => {
        b.unsubscribe = () => subscription.unsubscribe();
        if (b.disposed) void b.unsubscribe().catch(() => {});
      })
      .catch((e: unknown) => console.warn(LOG, "fs.changed subscription failed", machine, e));
    return b;
  }

  /** 그 머신의 문서가 열린 워크트리(그 머신의 경로, 메인 체크아웃은 `null`). */
  function worktreesOf(machine: string): Set<string | null> {
    const out = new Set<string | null>();
    for (const { path, worktree } of documents.opened())
      if (machineOf(path, worktree) === machine) out.add(checkoutOfKey(worktree).worktree ?? null);
    return out;
  }

  function update() {
    if (stopped) return;
    const { remote } = hosts.store.getState();
    for (const machine of Object.keys(remote)) {
      if (machine === LOCAL_MACHINE) continue;
      const c = hosts.connection(machine);
      const usable =
        c.client !== undefined &&
        c.projectId !== undefined &&
        (c.status === undefined || c.status.state === "connected");
      if (online.get(machine) !== usable) {
        online.set(machine, usable);
        if (!usable) {
          offline.add(machine);
          documents.disconnect(machine, belongs(machine));
        } else if (offline.delete(machine)) void documents.reconnect(machine);
      }
      // 감시 · 구독은 멤버 연결에 묶는다 — 다시 붙는 중에도 연결이 살아 있으면 그대로 둔다(M7-09 — 스트림이 이어진다).
      let b = bound.get(machine);
      if (b && (b.client !== c.client || b.projectId !== c.projectId)) {
        unbind(machine);
        b = undefined;
      }
      if (!c.client || c.projectId === undefined) continue;
      b ??= bind(machine, c.client, c.projectId);
      const wanted = worktreesOf(machine);
      for (const [worktree, stop] of b.watches)
        if (!wanted.has(worktree)) {
          stop();
          b.watches.delete(worktree);
        }
      for (const worktree of wanted)
        if (!b.watches.has(worktree))
          b.watches.set(worktree, watch(b.client, b.projectId, worktree ?? undefined));
    }
    for (const machine of bound.keys()) if (!(machine in remote)) unbind(machine);
    for (const machine of online.keys()) if (!(machine in remote)) online.delete(machine);
  }

  const offHosts = hosts.store.subscribe(update);
  const offDocs = documents.store.subscribe(update);
  update();
  return () => {
    stopped = true;
    offHosts();
    offDocs();
    for (const machine of bound.keys()) unbind(machine);
  };
}
