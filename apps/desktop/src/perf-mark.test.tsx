// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { FirstFrameMark, perfMarkRequested } from "./perf-mark";

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

it("marks once, on the second frame after the first commit", () => {
  const frames: (() => void)[] = [];
  const onFrame = vi.fn();
  const frame = (callback: () => void) => void frames.push(callback);
  root = createRoot(document.createElement("div"));
  const doc = visibility("visible");
  act(() => root!.render(<FirstFrameMark onFrame={onFrame} frame={frame} doc={doc} />));

  expect(frames).toHaveLength(1);
  frames.shift()!();
  expect(onFrame).not.toHaveBeenCalled();
  expect(frames).toHaveLength(1);
  frames.shift()!();
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
  frames.shift()!();
  frames.shift()!();
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
  frames.shift()!();
  frames.shift()!();
  expect(onFrame).toHaveBeenCalledWith({ hidden: true });
});
