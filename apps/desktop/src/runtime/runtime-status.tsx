// 상태바의 런타임 항목(12-m1-plan.md M1-16, 06-roadmap.md 「리스크」 표의 외부 런타임 도구 행): 이 프로젝트의 런타임 환경을 어느
// 제공자가 만들었고 물러났는지(`runtime.get`, crates/pw-runtime). mise 가 환경을 만들면 「mise」, 제공자나 로그인 셸이 실패해 그 아래
// 환경으로 물러났으면 경고(mise 없음 · mise 미신뢰 · mise 실패 · 로그인 셸 못 읽음)와 원문 이유를 보인다. 로그인 셸만 쓰는 보통의
// 경우는 그리지 않는다 — 신뢰하지 않은 프로젝트는 신뢰 항목(trust/trust.tsx)이 따로 알린다. 도구 버전 표시(`node 22 · java 21`)는
// F14-4(P1)다.
//
// Host 가 환경을 다시 계산하면(`runtime.changed` — 「환경 다시 읽기」, 신뢰, 버전 파일 변경, 로그인 셸 회복) 다시 읽는다. 누르면
// 「환경 다시 읽기」 액션이다.

import type { RuntimeInfo } from "@pitwall/rpc-client";
import { Layers, TriangleAlert } from "lucide-react";
import { useEffect } from "react";
import { createStore, useStore } from "zustand";

import { useKeymap } from "../keymap/keymap-provider";
import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { useHost, useRpcQuery } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";

/** 「환경 다시 읽기」 액션 id(runtime-actions.ts). */
const RELOAD_ACTION = "runtime.reload";

/** 이 창의 프로젝트. 창마다 웹뷰가 따로라 모듈 상태로 둔다(항목 컴포넌트는 props 를 받지 않는다, status-bar.tsx 머리 주석). */
const runtimeState = createStore<{ projectId: string | undefined }>()(() => ({
  projectId: undefined,
}));

/** 상태바에 그릴 것. 그리지 않으면 `undefined`. */
export function describeRuntime(
  info: RuntimeInfo,
  T: ProjectStrings["runtime"],
): { warning: boolean; label: string; hint: string } | undefined {
  if (info.status.kind === "fallback")
    return {
      warning: true,
      label: T.fallback(info.status.problem),
      hint: T.fallbackHint(info.status.problem, info.status.reason),
    };
  if (info.provider === "mise")
    return {
      warning: false,
      label: T.mise,
      hint: T.miseHint(info.source.kind === "configFiles" ? info.source.files.join("\n") : ""),
    };
  return undefined;
}

function RuntimeStatusFor({ projectId }: { projectId: string }) {
  const T = useProjectStrings().runtime;
  const { registry } = useKeymap();
  const { client } = useHost();
  const info = useRpcQuery("runtime.get", { projectId, dir: null }, { retry: false });
  const { refetch } = info;

  // 프로젝트 범위 이벤트는 창이 직접 구독한다(Host 연결은 프로젝트 id 없이 열려 쿼리 무효화 규칙이 프로젝트 범위를 받지 않는다).
  useEffect(() => {
    if (!client) return;
    const opened = client.subscribe({ projectId, topics: ["runtime.changed"] }, () => {
      void refetch();
    });
    // 구독이 열린 뒤 한 번 더 읽는다 — 그 전에 받은 응답은 사이의 변경을 놓쳤을 수 있다.
    opened.then(
      () => void refetch(),
      (e: unknown) => console.warn("[runtime] cannot subscribe to runtime.changed", e),
    );
    return () => {
      opened.then((s) => s.unsubscribe()).catch(() => {});
    };
  }, [client, projectId, refetch]);

  const view = info.data ? describeRuntime(info.data, T) : undefined;
  if (!view) return null;
  const Icon = view.warning ? TriangleAlert : Layers;
  return (
    <button
      type="button"
      data-testid="status-runtime"
      data-warning={view.warning || undefined}
      title={view.hint}
      onClick={() => registry.run(RELOAD_ACTION)}
      className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
    >
      <Icon size={11} aria-hidden />
      {view.label}
    </button>
  );
}

/** 상태바 항목: 런타임 제공자 상태와 경고. */
export function RuntimeStatusItem() {
  const projectId = useStore(runtimeState, (s) => s.projectId);
  return projectId === undefined ? null : <RuntimeStatusFor projectId={projectId} />;
}

/** 이 창의 프로젝트로 런타임 항목을 둔다. `StatusBarProvider` 안에 둔다. */
export function RuntimeStatus({ projectId }: { projectId: string }) {
  useEffect(() => {
    runtimeState.setState({ projectId });
    return () => runtimeState.setState({ projectId: undefined });
  }, [projectId]);
  useStatusBarItem({ id: "runtime", ...STATUS_SLOTS.runtime, component: RuntimeStatusItem });
  return null;
}
