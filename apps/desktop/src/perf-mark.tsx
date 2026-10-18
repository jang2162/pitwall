// 성능 측정의 첫 프레임 표시. 측정 모드로 뜬 앱만 프로젝트 창 주소에 `perf=1` 을 붙인다(src-tauri/src/perf.rs) — 그 밖에는
// 쿼리 하나만 읽고 이 컴포넌트를 그리지 않는다.
//
// 「첫 화면」 = 첫 React 커밋 뒤 두 번째 애니메이션 프레임. 형제 컴포넌트의 effect 는 같은 커밋(창 내용 전체)이 끝난 뒤에
// 돌고, 첫 rAF 콜백은 그 커밋을 담은 프레임을 그리기 전에 불리므로 한 프레임 더 기다린다. 화면이 잠겼거나 창이 가려지면
// WebKit 이 rAF 를 미뤄 표시가 늦게 오거나 오지 않는다 — 그 전에 문서가 한 번이라도 가려졌으면(`visibilitychange`) 표시에
// `hidden` 을 실어 측정하는 쪽이 그 회를 기준선에서 뺀다.

import { useEffect } from "react";

export function perfMarkRequested(search: string): boolean {
  return new URLSearchParams(search).get("perf") === "1";
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
  frame = animationFrame,
  doc = document,
}: {
  onFrame: (mark: { hidden: boolean }) => void;
  frame?: (callback: () => void) => void;
  doc?: Visibility;
}) {
  useEffect(() => {
    let cancelled = false;
    let hidden = doc.visibilityState === "hidden";
    const watch = () => {
      if (doc.visibilityState === "hidden") hidden = true;
    };
    doc.addEventListener("visibilitychange", watch);
    frame(() =>
      frame(() => {
        doc.removeEventListener("visibilitychange", watch);
        if (!cancelled) onFrame({ hidden });
      }),
    );
    // StrictMode(dev)는 effect 를 한 번 걷고 다시 돌린다 — 걷힌 쪽의 표시는 보내지 않는다.
    return () => {
      cancelled = true;
      doc.removeEventListener("visibilitychange", watch);
    };
  }, [onFrame, frame, doc]);
  return null;
}
