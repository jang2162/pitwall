// 백그라운드 작업 표시(F11-5): Host 의 `Task`(crates/pw-core/src/task.rs)를 상태바에 진행률로 보이고 취소한다.
//
// 누가 띄운 작업이든 같은 길로 본다 — 진행 · 종료 알림은 이벤트 버스의 `task.progress` · `task.finished` 로 오고, 구독하기 전에
// 시작한 작업은 `task.list` 로 읽는다. 취소는 `task.cancel`(작업 id). Host 범위 작업을 늘 받고, 창이 프로젝트 id 를 알면 그
// 프로젝트 범위 작업도 받는다. 실패로 끝난 작업은 오류 알림을 낸다(notifications/notifications.tsx).

import type { Action } from "@pitwall/keymap";
import type {
  DemoTaskParams,
  EventMessage,
  RpcClient,
  TaskInfo,
  TaskProgress,
  TaskStatus,
} from "@pitwall/rpc-client";
import { X } from "lucide-react";
import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createStore, type StoreApi, useStore } from "zustand";

import { useRegisterActions } from "../keymap/keymap-provider";
import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { useNotify } from "../notifications/notifications";
import { useHost } from "../rpc/host";

/** 버스 토픽(task.rs 의 `TOPIC_PROGRESS` · `TOPIC_FINISHED`). */
export const TASK_TOPICS = ["task.progress", "task.finished"];

/** `task.progress` 의 payload(`TaskProgressEvent`). 이벤트 payload 는 RPC 생성물에 없어 여기 적는다. */
export interface TaskProgressEvent {
  taskId: number;
  title: string;
  progress: TaskProgress;
}

/** `task.finished` 의 payload(`TaskFinishedEvent`). 상태가 펼쳐져 온다. */
export type TaskFinishedEvent = { taskId: number; title: string } & TaskStatus;

interface TaskState {
  /** 끝나지 않은 작업. id 순(먼저 띄운 것이 앞)이다. */
  tasks: readonly TaskInfo[];
  progressed(event: TaskProgressEvent, projectId: string | null): void;
  finished(taskId: number): void;
  /** `task.list` 의 응답을 합친다. 그 응답보다 먼저 끝남을 받은 작업은 넣지 않는다. */
  listed(tasks: readonly TaskInfo[]): void;
}

export type TaskStore = StoreApi<TaskState>;

const byId = (a: TaskInfo, b: TaskInfo) => a.taskId - b.taskId;

/** `task.list` 응답 전에 기억하는 끝난 작업 id 의 최대 수. 넘으면 오래된 것부터 잊는다. */
export const ENDED_LIMIT = 1000;

export function createTaskStore(): TaskStore {
  // 끝남을 본 작업 id. `task.list` 응답이 끝남 알림보다 늦게 와도 끝난 작업을 되살리지 않게 한다. 응답을 합친 뒤에는 쓸
  // 일이 없어 비운다 — 그 뒤 같은 작업의 알림은 버스 순서상 끝남보다 먼저 온다. 응답이 오지 않는 동안에도 끝없이 자라지 않게
  // 상한을 둔다.
  const ended = new Set<number>();
  return createStore<TaskState>()((set) => ({
    tasks: [],
    progressed({ taskId, title, progress }, projectId) {
      if (ended.has(taskId)) return;
      set((s) => ({
        tasks: [
          ...s.tasks.filter((t) => t.taskId !== taskId),
          { taskId, title, projectId, progress },
        ].sort(byId),
      }));
    },
    finished(taskId) {
      ended.add(taskId);
      if (ended.size > ENDED_LIMIT) ended.delete(ended.values().next().value!);
      set((s) => ({ tasks: s.tasks.filter((t) => t.taskId !== taskId) }));
    },
    listed(tasks) {
      set((s) => {
        const known = new Set(s.tasks.map((t) => t.taskId));
        const fresh = tasks.filter((t) => !known.has(t.taskId) && !ended.has(t.taskId));
        ended.clear();
        return fresh.length === 0 ? s : { tasks: [...s.tasks, ...fresh].sort(byId) };
      });
    },
  }));
}

/**
 * 작업 알림을 구독하고 지금 도는 작업을 읽어 `store` 에 채운다. 끝난 작업마다 `onFinished` 를 부른다. 구독 · 목록 읽기가 실패하면
 * 경고만 남긴다 — 상태바에 작업이 보이지 않을 뿐 창은 동작한다.
 */
export function connectTasks(
  client: RpcClient,
  store: TaskStore,
  projectId: string | null,
  onFinished: (event: TaskFinishedEvent) => void,
): () => void {
  let disposed = false;
  const onEvent = (event: EventMessage) => {
    if (disposed) return;
    const { progressed, finished } = store.getState();
    if (event.topic === "task.progress") {
      progressed(event.payload as unknown as TaskProgressEvent, event.projectId);
    } else if (event.topic === "task.finished") {
      const payload = event.payload as unknown as TaskFinishedEvent;
      finished(payload.taskId);
      onFinished(payload);
    }
  };
  const subscriptions = [
    client.subscribe({ topics: TASK_TOPICS }, onEvent),
    ...(projectId === null ? [] : [client.subscribe({ projectId, topics: TASK_TOPICS }, onEvent)]),
  ];
  // 구독이 열린 뒤에 읽는다 — 그 전에 읽으면 사이에 시작한 작업을 놓친다.
  Promise.all(subscriptions)
    .then(() => client.call("task.list", { projectId }))
    .then((tasks) => {
      if (!disposed) store.getState().listed(tasks);
    })
    .catch((error: unknown) => console.warn("[tasks] could not follow background tasks", error));
  return () => {
    disposed = true;
    for (const opened of subscriptions) {
      opened.then((s) => s.unsubscribe()).catch(() => {});
    }
  };
}

const TaskContext = createContext<TaskStore | undefined>(undefined);

function useTaskStore(): TaskStore {
  const store = useContext(TaskContext);
  if (!store) throw new Error("TaskProvider 밖에서 작업을 썼다");
  return store;
}

export function useTasks<T>(selector: (state: TaskState) => T): T {
  return useStore(useTaskStore(), selector);
}

/**
 * 연결 하나의 작업 표. 연결(`client`)이 바뀌면 새 표다 — 작업 id 는 Host 마다 1부터라, 프로젝트가 다른 Host 로 다시 묶이면
 * 옛 Host 의 작업이 남아 새 Host 의 id 와 겹친다(03-architecture §3).
 */
export function useTaskStoreFor(client: RpcClient | undefined): TaskStore {
  const [owned, setOwned] = useState(() => ({ client, store: createTaskStore() }));
  if (owned.client === client) return owned.store;
  // 렌더 중에 상태를 맞춘다(React 「props 가 바뀔 때 state 조정」). 이번 렌더부터 새 표를 쓴다.
  const next = { client, store: createTaskStore() };
  setOwned(next);
  return next.store;
}

/** 창 하나의 작업 표. `NotificationProvider` 안에 둔다 — 실패한 작업을 알린다. */
export function TaskProvider({
  projectId,
  children,
}: {
  /** 창의 프로젝트. 있으면 그 프로젝트 범위 작업도 보인다. */
  projectId?: string;
  children: ReactNode;
}) {
  const { client } = useHost();
  const store = useTaskStoreFor(client);
  const notify = useNotify();
  const strings = useProjectStrings().tasks;
  const failed = useRef(strings.failed);
  failed.current = strings.failed;
  useEffect(() => {
    if (!client) return;
    return connectTasks(client, store, projectId ?? null, (event) => {
      if (event.status === "failed")
        notify({ level: "error", title: failed.current(event.title), body: event.error });
    });
  }, [client, store, projectId, notify]);
  return <TaskContext value={store}>{children}</TaskContext>;
}

/** 진행률(0~1). 전체 양을 모르면 `undefined` — 불확정 막대를 그린다. */
export function fraction(progress: TaskProgress): number | undefined {
  const { done, total } = progress;
  if (total === undefined || total === null || total <= 0) return undefined;
  return Math.min(1, Math.max(0, done / total));
}

function ProgressBar({ progress, label }: { progress: TaskProgress; label: string }) {
  const value = fraction(progress);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value === undefined ? undefined : Math.round(value * 100)}
      className="h-1 w-16 flex-none overflow-hidden rounded-full bg-ui-border"
    >
      <div
        className={`h-full bg-ui-accent ${value === undefined ? "w-1/3 animate-pulse" : ""}`}
        style={value === undefined ? undefined : { width: `${value * 100}%` }}
      />
    </div>
  );
}

function TaskRow({ task }: { task: TaskInfo }) {
  const T = useProjectStrings().tasks;
  const { client } = useHost();
  const notify = useNotify();
  const value = fraction(task.progress);
  const cancel = () => {
    client?.call("task.cancel", { taskId: task.taskId }).catch((error: unknown) =>
      notify({
        level: "error",
        title: T.cancelFailed,
        body: error instanceof Error ? error.message : String(error),
      }),
    );
  };
  return (
    <div data-task-id={task.taskId} className="flex min-w-0 items-center gap-1">
      <span className="max-w-48 truncate" title={task.progress.message ?? task.title}>
        {task.title}
      </span>
      <ProgressBar progress={task.progress} label={task.title} />
      {value !== undefined && <span>{Math.round(value * 100)}%</span>}
      <button
        type="button"
        title={T.cancel(task.title)}
        aria-label={T.cancel(task.title)}
        onClick={cancel}
        className="flex items-center rounded-sm hover:bg-ui-hover"
      >
        <X size={11} aria-hidden />
      </button>
    </div>
  );
}

/** 상태바 항목: 가장 최근에 띄운 작업 하나와 나머지 수. 수를 누르면 모든 작업을 펼친다. 작업이 없으면 그리지 않는다. */
export function TasksStatusItem() {
  const T = useProjectStrings().tasks;
  const tasks = useTasks((s) => s.tasks);
  const [expanded, setExpanded] = useState(false);
  const latest = tasks.at(-1);
  if (!latest) return null;
  const others = tasks.length - 1;
  return (
    <div className="relative flex items-center gap-1">
      <TaskRow task={latest} />
      {others > 0 && (
        <button
          type="button"
          aria-expanded={expanded}
          aria-label={T.label}
          onClick={() => setExpanded((e) => !e)}
          className="rounded-sm px-1 hover:bg-ui-hover"
        >
          {T.more(others)}
        </button>
      )}
      {expanded && others > 0 && (
        <div
          role="list"
          aria-label={T.label}
          className="absolute right-0 bottom-full mb-1 flex flex-col gap-1 rounded-sm border border-ui-border bg-ui-panel p-1.5 text-ui-foreground shadow-md"
        >
          {tasks.map((task) => (
            <div role="listitem" key={task.taskId}>
              <TaskRow task={task} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** 데모 작업의 길이: 200 ms 마다 한 걸음, 50 걸음(10초). 사람이 진행과 취소를 눈으로 볼 만큼. */
export const DEMO_TASK: DemoTaskParams = { steps: 50, intervalMs: 200 };

/**
 * `host.demoTask` 를 띄우는 액션(개발 빌드 전용, M1-04 완료 기준을 손으로 확인하는 길). 진행은 상태바가 버스 알림으로 본다 —
 * 여기서는 스트림을 끝까지 읽기만 한다(읽지 않으면 credit 이 없어 Host 쪽 핸들러가 멈춘다). 시작이 실패하면(잘못된 인자 등)
 * 오류 알림을 낸다 — 「잘못된 인자」 액션은 그 길을 일부러 탄다.
 */
export function demoTaskActions(deps: {
  client: () => RpcClient | undefined;
  strings: ProjectStrings["tasks"];
  notify: (input: { level: "error"; title: string; body: string }) => void;
}): Action[] {
  const run = (params: DemoTaskParams) => async () => {
    const client = deps.client();
    if (!client) return;
    try {
      for await (const _ of client.stream("host.demoTask", params)) {
        // 진행은 상태바가 버스로 받는다.
      }
    } catch (error) {
      deps.notify({
        level: "error",
        title: deps.strings.startFailed,
        body: error instanceof Error ? error.message : String(error),
      });
    }
  };
  return [
    { id: "task.demo", label: deps.strings.demo, keywords: ["Demo Task"], run: run(DEMO_TASK) },
    {
      id: "task.demoInvalid",
      label: deps.strings.demoInvalid,
      keywords: ["Demo Task"],
      run: run({ ...DEMO_TASK, steps: 0 }),
    },
  ];
}

/** 개발 빌드에서만 데모 작업 액션을 등록한다. */
export function useDemoTaskActions() {
  const { client } = useHost();
  const clientRef = useRef(client);
  clientRef.current = client;
  const strings = useProjectStrings().tasks;
  const notify = useNotify();
  const actions = useMemo(
    () =>
      import.meta.env.DEV
        ? demoTaskActions({ client: () => clientRef.current, strings, notify })
        : [],
    [strings, notify],
  );
  useRegisterActions(actions);
}
