// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import {
  FirstFrameMark,
  PERF_SCENARIOS,
  perfMarkRequested,
  perfScenarioRequested,
} from "./perf-mark";
import { SCENARIOS } from "./perf-scenario";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
});

it("only the measurement query asks for the mark", () => {
  expect(perfMarkRequested("?window=project&project=a&perf=1")).toBe(true);
  expect(perfMarkRequested("?window=project&project=a")).toBe(false);
  expect(perfMarkRequested("?window=project&perf=0")).toBe(false);
});

it("a scenario runs only in measurement mode and only by a known name", () => {
  expect(perfScenarioRequested("?window=project&perf=1&perfScenario=git")).toBe("git");
  expect(perfScenarioRequested("?window=project&perfScenario=git")).toBeUndefined();
  expect(perfScenarioRequested("?window=project&perf=1&perfScenario=other")).toBeUndefined();
  expect(perfScenarioRequested("?window=project&perf=1&perfScenario=db")).toBe("db");
  expect(perfScenarioRequested("?window=project&perf=1&perfScenario=db-idle")).toBe("db-idle");
  expect(perfScenarioRequested("?window=project&perfScenario=db")).toBeUndefined();
  expect(perfScenarioRequested("?window=project&perf=1")).toBeUndefined();
  // 이 목록은 perf-scenario.ts 의 것과 같아야 한다(그 모듈은 측정 모드에서만 불러와 여기 따로 둔다).
  expect([...PERF_SCENARIOS]).toEqual([...SCENARIOS]);
});

function visibility(initial: DocumentVisibilityState) {
  const listeners = new Set<() => void>();
  return {
    visibilityState: initial,
    addEventListener: (_: string, l: () => void) => void listeners.add(l),
    removeEventListener: (_: string, l: () => void) => void listeners.delete(l),
    set(state: DocumentVisibilityState) {
      this.visibilityState = state;
      for (const l of listeners) l();
    },
    listeners,
  };
}

/**
 * 지금 걸린 프레임을 모두 돌린다(애니메이션 프레임 하나). 시험은 앱처럼 StrictMode 로 그린다 — effect 를 걷었다 다시 건 쪽의
 * 프레임도 줄에 있어(그 쪽은 표시를 보내지 않는다) 하나씩 꺼내면 엉뚱한 콜백을 돌린다.
 */
function tick(frames: (() => void)[]) {
  for (const f of frames.splice(0)) f();
}

it("marks once, on the second frame after the first commit", () => {
  const frames: (() => void)[] = [];
  const onFrame = vi.fn();
  const frame = (callback: () => void) => void frames.push(callback);
  root = createRoot(document.createElement("div"));
  const doc = visibility("visible");
  act(() => root!.render(<FirstFrameMark onFrame={onFrame} frame={frame} doc={doc} />));

  expect(frames.length).toBeGreaterThan(0);
  tick(frames);
  expect(onFrame).not.toHaveBeenCalled();
  expect(frames.length).toBeGreaterThan(0);
  tick(frames);
  expect(onFrame).toHaveBeenCalledTimes(1);
  expect(onFrame).toHaveBeenCalledWith({ hidden: false });
  expect(frames).toHaveLength(0);
});

it("keeps reporting each time the window is hidden after its first frame", () => {
  const frames: (() => void)[] = [];
  const onHidden = vi.fn();
  const doc = visibility("visible");
  root = createRoot(document.createElement("div"));
  act(() =>
    root!.render(
      <FirstFrameMark
        onFrame={() => {}}
        onHidden={onHidden}
        frame={(cb) => void frames.push(cb)}
        doc={doc}
      />,
    ),
  );
  tick(frames);
  tick(frames);
  doc.set("hidden");
  doc.set("visible");
  doc.set("hidden");
  expect(onHidden).toHaveBeenCalledTimes(2);
  act(() => root!.unmount());
  root = undefined;
  expect(doc.listeners.size).toBe(0);
});

it("reports a window that was hidden before its first frame", () => {
  const frames: (() => void)[] = [];
  const onFrame = vi.fn();
  const doc = visibility("visible");
  root = createRoot(document.createElement("div"));
  act(() =>
    root!.render(
      <FirstFrameMark onFrame={onFrame} frame={(cb) => void frames.push(cb)} doc={doc} />,
    ),
  );
  doc.set("hidden");
  doc.set("visible");
  tick(frames);
  tick(frames);
  expect(onFrame).toHaveBeenCalledWith({ hidden: true });
});

it("reports each return to visible, so a stalled run shows whether the window stayed hidden", () => {
  const onHidden = vi.fn();
  const onVisible = vi.fn();
  const doc = visibility("visible");
  root = createRoot(document.createElement("div"));
  act(() =>
    root!.render(
      <FirstFrameMark
        onFrame={() => {}}
        onHidden={onHidden}
        onVisible={onVisible}
        frame={() => {}}
        doc={doc}
      />,
    ),
  );
  doc.set("hidden");
  doc.set("visible");
  doc.set("hidden");
  expect(onHidden).toHaveBeenCalledTimes(2);
  expect(onVisible).toHaveBeenCalledTimes(1);
});

it("reports a window hidden from the start without waiting for a first frame that may never come", () => {
  const onHidden = vi.fn();
  const onFrame = vi.fn();
  root = createRoot(document.createElement("div"));
  act(() =>
    root!.render(
      <FirstFrameMark
        onFrame={onFrame}
        onHidden={onHidden}
        frame={() => {}}
        doc={visibility("hidden")}
      />,
    ),
  );
  expect(onFrame).not.toHaveBeenCalled();
  expect(onHidden).toHaveBeenCalledTimes(1);
});
