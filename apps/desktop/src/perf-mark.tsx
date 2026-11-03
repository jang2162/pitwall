// 성능 측정의 첫 프레임 표시. 측정 모드로 뜬 앱만 프로젝트 창 주소에 `perf=1` 을 붙인다(src-tauri/src/perf.rs) — 그 밖에는
// 쿼리 하나만 읽고 이 컴포넌트를 그리지 않는다.
//
// 「첫 화면」 = 첫 React 커밋 뒤 두 번째 애니메이션 프레임. 형제 컴포넌트의 effect 는 같은 커밋(창 내용 전체)이 끝난 뒤에
// 돌고, 첫 rAF 콜백은 그 커밋을 담은 프레임을 그리기 전에 불리므로 한 프레임 더 기다린다. 화면이 잠겼거나 창이 가려지면
// WebKit 이 rAF 를 미루고 메모리를 줄인다 — 첫 프레임 전에 문서가 한 번이라도 가려졌으면(`visibilitychange`) 표시에
// `hidden` 을 싣고, 그 뒤에도 가려질 때마다 `onHidden` 을 불러 측정하는 쪽이 유휴 표본 전에 가려진 회를 기준선에서 뺀다.
// 그래서 리스너는 첫 프레임 뒤에도 떼지 않는다(측정 모드에서만 그려지는 컴포넌트다). 다시 보이면 `onVisible`, 처음부터 가려져
// 있으면 첫 프레임을 기다리지 않고 바로 `onHidden` 을 부른다 — 프레임이 멈춰 회가 실패했을 때 측정하는 쪽이 그때 창이
// 가려져 있었는지 표시 파일만으로 가른다.

import { useEffect } from "react";

export function perfMarkRequested(search: string): boolean {
  return new URLSearchParams(search).get("perf") === "1";
}

/** 아는 시나리오(src/perf-scenario.ts 의 `SCENARIOS` 와 같다 — 시험이 본다. 그 모듈은 측정 모드에서만 불러오므로 여기 따로 둔다). */
export const PERF_SCENARIOS = [
  "git",
  "db",
  "db-idle",
  "lsp",
  "lsp-typing",
  "lsp-typing-off",
  "lsp-idle",
  "symbols",
  "classes",
] as const;
export type PerfScenario = (typeof PERF_SCENARIOS)[number];

/** 첫 프레임 뒤에 돌릴 측정 시나리오(src/perf-scenario.ts). 측정 모드가 아니거나 모르는 이름이면 없다. */
export function perfScenarioRequested(search: string): PerfScenario | undefined {
  const params = new URLSearchParams(search);
  if (params.get("perf") !== "1") return undefined;
  const name = params.get("perfScenario");
  return PERF_SCENARIOS.find((s) => s === name);
}

/** 문서의 가려짐. 시험은 가짜를 넘긴다. */
export interface Visibility {
  readonly visibilityState: DocumentVisibilityState;
  addEventListener(type: "visibilitychange", listener: () => void): void;
  removeEventListener(type: "visibilitychange", listener: () => void): void;
}

const animationFrame = (callback: () => void) => void window.requestAnimationFrame(callback);

export function FirstFrameMark({
  onFrame,
  onHidden,
  onVisible,
  frame = animationFrame,
  doc = document,
}: {
  onFrame: (mark: { hidden: boolean }) => void;
  onHidden?: () => void;
  onVisible?: () => void;
  frame?: (callback: () => void) => void;
  doc?: Visibility;
}) {
  useEffect(() => {
    let cancelled = false;
    let hidden = doc.visibilityState === "hidden";
    const watch = () => {
      if (cancelled) return;
      if (doc.visibilityState !== "hidden") {
        onVisible?.();
        return;
      }
      hidden = true;
      onHidden?.();
    };
    doc.addEventListener("visibilitychange", watch);
    if (hidden) onHidden?.();
    frame(() =>
      frame(() => {
        if (!cancelled) onFrame({ hidden });
      }),
    );
    // StrictMode(dev)는 effect 를 한 번 걷고 다시 돌린다 — 걷힌 쪽의 표시는 보내지 않는다.
    return () => {
      cancelled = true;
      doc.removeEventListener("visibilitychange", watch);
    };
  }, [onFrame, onHidden, onVisible, frame, doc]);
  return null;
}
