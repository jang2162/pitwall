// 작업 표: 진행 알림 · 끝남 알림 · `task.list` 응답을 합치는 규칙.

import type { TaskInfo } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { createTaskStore, fraction } from "./tasks";

const info = (taskId: number, title = `t${taskId}`): TaskInfo => ({
  taskId,
  title,
  projectId: null,
  progress: { done: 0, total: null, message: null },
});

const ids = (store: ReturnType<typeof createTaskStore>) =>
  store.getState().tasks.map((t) => t.taskId);

describe("task store", () => {
  it("adds a task on its first progress and updates it in place", () => {
    const store = createTaskStore();
    store.getState().progressed({ taskId: 2, title: "b", progress: { done: 1, total: 4 } }, null);
    store.getState().progressed({ taskId: 1, title: "a", progress: { done: 0 } }, "p1");
    store.getState().progressed({ taskId: 2, title: "b", progress: { done: 3, total: 4 } }, null);
    expect(ids(store)).toEqual([1, 2]);
    expect(store.getState().tasks[0]!.projectId).toBe("p1");
    expect(store.getState().tasks[1]!.progress.done).toBe(3);
  });

  it("does not bring back a task that finished before the list answer arrived", () => {
    const store = createTaskStore();
    store.getState().finished(1);
    store.getState().listed([info(1), info(2)]);
    expect(ids(store)).toEqual([2]);
  });

  it("keeps the newer progress over a list answer for the same task", () => {
    const store = createTaskStore();
    store.getState().progressed({ taskId: 1, title: "a", progress: { done: 9, total: 10 } }, null);
    store.getState().listed([info(1), info(3)]);
    expect(ids(store)).toEqual([1, 3]);
    expect(store.getState().tasks[0]!.progress.done).toBe(9);
  });

  it("ignores progress that arrives after the task finished", () => {
    const store = createTaskStore();
    store.getState().progressed({ taskId: 1, title: "a", progress: { done: 1 } }, null);
    store.getState().finished(1);
    store.getState().progressed({ taskId: 1, title: "a", progress: { done: 2 } }, null);
    expect(ids(store)).toEqual([]);
  });
});

describe("fraction", () => {
  it("is undefined when the total is unknown, and clamped otherwise", () => {
    expect(fraction({ done: 3 })).toBeUndefined();
    expect(fraction({ done: 3, total: null })).toBeUndefined();
    expect(fraction({ done: 0, total: 0 })).toBeUndefined();
    expect(fraction({ done: 5, total: 50 })).toBe(0.1);
    expect(fraction({ done: 60, total: 50 })).toBe(1);
  });
});
