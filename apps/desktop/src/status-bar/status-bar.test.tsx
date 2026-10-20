// @vitest-environment happy-dom
// 상태바 항목 슬롯: 쪽 · 순서대로 놓이고, 등록한 컴포넌트가 사라지면 빠지고, 같은 id 는 바뀐다.

import { act, StrictMode, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { StatusBar, type StatusBarItem, StatusBarProvider, useStatusBarItem } from "./status-bar";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  vi.restoreAllMocks();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

const A = () => <span>a</span>;
const B = () => <span>b</span>;
const C = () => <span>c</span>;
const D = () => <span>d</span>;
const Nothing = () => null;

function Register({ item }: { item: StatusBarItem }) {
  useStatusBarItem(item);
  return null;
}

let show: (items: StatusBarItem[]) => void = () => {};

function Harness({ initial }: { initial: StatusBarItem[] }) {
  const [items, setItems] = useState(initial);
  show = setItems;
  return (
    <StatusBarProvider>
      {/* 등록 하나가 컴포넌트 하나다 — 목록에서 빠진 등록만 언마운트되게 key 를 등록마다 다르게 둔다. */}
      {items.map((item) => (
        <Register key={`${item.id}:${item.side}:${item.order}`} item={item} />
      ))}
      <StatusBar label="status" />
    </StatusBarProvider>
  );
}

async function mount(initial: StatusBarItem[]) {
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () => root!.render(<Harness initial={initial} />));
}

/** 상태바의 항목 id 를 화면 순서로. 왼쪽과 오른쪽 사이에 `|`. */
function layout(): string[] {
  const footer = document.querySelector("footer")!;
  return [...footer.children].map((el) => (el as HTMLElement).dataset.statusItem ?? "|");
}

it("places items by side and order, whatever order they register in", async () => {
  await mount([
    { id: "notifications", side: "right", order: 900, component: C },
    { id: "trust", side: "left", order: 200, component: B },
    { id: "tasks", side: "right", order: 100, component: D },
    { id: "host", side: "left", order: 100, component: A },
  ]);
  expect(layout()).toEqual(["host", "trust", "|", "tasks", "notifications"]);
  expect(document.querySelector('[data-status-item="trust"]')!.textContent).toBe("b");
});

it("drops an item when the component that registered it goes away", async () => {
  const host: StatusBarItem = { id: "host", side: "left", order: 100, component: A };
  const tasks: StatusBarItem = { id: "tasks", side: "right", order: 100, component: D };
  await mount([host, tasks]);
  await act(async () => show([tasks]));
  expect(layout()).toEqual(["|", "tasks"]);
});

it("replaces an item registered again with the same id, and keeps the newer one when the older leaves", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  const older: StatusBarItem = { id: "x", side: "left", order: 100, component: A };
  const newer: StatusBarItem = { id: "x", side: "right", order: 100, component: B };
  await mount([older, newer]);
  // 중복 등록은 실수다 — 개발 빌드(시험 포함)에서 경고한다.
  expect(warn).toHaveBeenCalledWith(expect.stringContaining('item "x" registered twice'));
  expect(layout()).toEqual(["|", "x"]);
  expect(document.querySelector('[data-status-item="x"]')!.textContent).toBe("b");
  await act(async () => show([newer]));
  expect(layout()).toEqual(["|", "x"]);
});

it("leaves an item that draws nothing empty", async () => {
  await mount([{ id: "idle", side: "right", order: 100, component: Nothing }]);
  expect(document.querySelector('[data-status-item="idle"]')!.childElementCount).toBe(0);
});

it("does not warn when StrictMode registers, unregisters and registers again", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <StrictMode>
        <Harness initial={[{ id: "host", side: "left", order: 100, component: A }]} />
      </StrictMode>,
    ),
  );
  expect(layout()).toEqual(["host", "|"]);
  expect(warn).not.toHaveBeenCalled();
});
