// 알림(docs/design/10-ui.md §2.8 「빈 상태 · 오류 상태」, F11-5): 토스트로 잠깐 띄우고 알림 툴 윈도우에 기록으로 남긴다.
// 오류는 알림 툴 윈도우를 볼 때까지 상태바의 알림 아이콘을 강조한다. 기록은 창 하나의 메모리에만 있다 — 창을 닫으면 사라진다.
//
// 알림을 내는 법: 창 안의 컴포넌트에서 `const notify = useNotify();` 를 받아 `notify({ level: "error", title, body })`.
// 문장은 그 기능의 문자열(Fluent)로 만들어 넘긴다. `body` 에는 오류 원문(RPC 오류 메시지 등)을 그대로 둔다.
//
// Host 가 내는 알림(감시 한도 초과 등)은 버스 토픽 `notification.show`(crates/pw-core/src/notification.rs)로 온다. 창은
// `HostNotifications` 가 Host 범위와 자기 프로젝트 범위를 구독해 같은 `notify` 로 띄운다. 제목은 Host 가 보낸 Fluent 메시지 id 를
// 이 창의 언어로 만든다.
//
// 토스트는 폭주해도 쌓이지 않는다(`ToastGate`): 같은 등급 · 같은 제목은 떠 있는 토스트 하나를 덮어쓰며 횟수를 붙이고, 동시에
// 떠 있는 토스트는 `TOAST_LIMIT` 개까지다 — 넘는 알림은 기록에만 남는다. sonner 는 보이는 수(`visibleToasts`)만 줄일 뿐
// 받은 토스트를 모두 DOM 과 타이머로 들고 있기 때문이다.

import { useLocalization } from "@fluent/react";
import type { EventMessage, RpcClient } from "@pitwall/rpc-client";
import type { IDockviewPanelProps } from "dockview-react";
import { Bell, CircleAlert, Info, TriangleAlert } from "lucide-react";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";
import { createStore, type StoreApi, useStore } from "zustand";

import { useKeymap } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useHost } from "../rpc/host";

export type NotificationLevel = "info" | "warning" | "error";

export interface NotificationInput {
  level: NotificationLevel;
  title: string;
  body?: string;
}

export interface AppNotification extends NotificationInput {
  id: number;
  /** 낸 시각(epoch ms). */
  at: number;
}

/** 기록에 남기는 최대 개수. 넘으면 오래된 것부터 버린다. */
export const NOTIFICATION_LIMIT = 200;

interface NotificationState {
  /** 새것이 앞이다. */
  items: readonly AppNotification[];
  /** 알림 툴 윈도우를 마지막으로 본 뒤에 온 오류 수. 상태바 강조가 이것을 본다. */
  unseenErrors: number;
  push(input: NotificationInput, at?: number): AppNotification;
  /** 알림 툴 윈도우가 보이는 동안 부른다. */
  markSeen(): void;
  clear(): void;
}

export type NotificationStore = StoreApi<NotificationState>;

export function createNotificationStore(): NotificationStore {
  let nextId = 1;
  return createStore<NotificationState>()((set) => ({
    items: [],
    unseenErrors: 0,
    push(input, at = Date.now()) {
      const item = { ...input, id: nextId++, at };
      set((s) => ({
        items: [item, ...s.items].slice(0, NOTIFICATION_LIMIT),
        unseenErrors: s.unseenErrors + (input.level === "error" ? 1 : 0),
      }));
      return item;
    },
    markSeen: () => set((s) => (s.unseenErrors === 0 ? s : { unseenErrors: 0 })),
    clear: () => set({ items: [], unseenErrors: 0 }),
  }));
}

/** 동시에 떠 있는 토스트의 최대 수. sonner 의 기본 보이는 수(3)와 같다. */
export const TOAST_LIMIT = 3;

/** 떠 있는 토스트(키 → 합친 횟수). 토스트가 닫히면(자동 · 손) 빠진다. 창 하나에 하나다. */
export class ToastGate {
  private readonly open = new Map<string, number>();

  /** 띄울 수 있으면 이 키로 합친 횟수(처음이면 1), 상한에 걸려 띄우지 않으면 `undefined`. */
  admit(key: string): number | undefined {
    const count = this.open.get(key);
    if (count === undefined && this.open.size >= TOAST_LIMIT) return undefined;
    const next = (count ?? 0) + 1;
    this.open.set(key, next);
    return next;
  }

  release(key: string): void {
    this.open.delete(key);
  }
}

interface NotificationContextValue {
  store: NotificationStore;
  gate: ToastGate;
}

const NotificationContext = createContext<NotificationContextValue | undefined>(undefined);

/** 창 하나의 알림 기록. `store` 는 시험이 넘긴다 — 없으면 새로 만든다. */
export function NotificationProvider({
  store: given,
  children,
}: {
  store?: NotificationStore;
  children: ReactNode;
}) {
  const [value] = useState(() => ({
    store: given ?? createNotificationStore(),
    gate: new ToastGate(),
  }));
  return <NotificationContext value={value}>{children}</NotificationContext>;
}

function useNotificationContext(): NotificationContextValue {
  const value = useContext(NotificationContext);
  if (!value) throw new Error("NotificationProvider 밖에서 알림을 썼다");
  return value;
}

function useNotificationStore(): NotificationStore {
  return useNotificationContext().store;
}

export function useNotifications<T>(selector: (state: NotificationState) => T): T {
  return useStore(useNotificationStore(), selector);
}

const TOASTS = { info: toast.info, warning: toast.warning, error: toast.error } as const;

/** 알림을 기록하고 토스트로 띄우는 함수. 함수는 창이 사는 동안 같다. 토스트를 합치고 막는 규칙은 머리 주석. */
export function useNotify(): (input: NotificationInput) => void {
  const { store, gate } = useNotificationContext();
  const repeated = useProjectStrings().notifications.repeated;
  const repeatedRef = useRef(repeated);
  repeatedRef.current = repeated;
  return useCallback(
    (input: NotificationInput) => {
      store.getState().push(input);
      const key = `${input.level}:${input.title}`;
      const count = gate.admit(key);
      if (count === undefined) return;
      const release = () => gate.release(key);
      TOASTS[input.level](count > 1 ? repeatedRef.current(input.title, count) : input.title, {
        id: key,
        description: input.body,
        onDismiss: release,
        onAutoClose: release,
      });
    },
    [store, gate],
  );
}

/** Host 알림의 토픽(crates/pw-core/src/notification.rs 의 `TOPIC_SHOW`). */
export const HOST_NOTIFICATION_TOPIC = "notification.show";

/** Host 알림의 내용(crates/pw-core/src/notification.rs 의 `Notification`). */
export interface HostNotification {
  level: NotificationLevel;
  /** 제목의 Fluent 메시지 id. */
  message: string;
  args?: Record<string, string>;
  /** 원문 그대로 보이는 본문. */
  body?: string;
}

const LEVELS: readonly string[] = ["info", "warning", "error"] satisfies NotificationLevel[];

/**
 * Host 범위와, `projectId` 가 있으면 그 프로젝트 범위의 Host 알림을 구독해 `show` 로 넘긴다. 구독이 실패하면 경고만 남긴다 — 그
 * 창에 Host 알림이 오지 않을 뿐이다. 돌려준 함수가 구독을 걷는다.
 */
export function connectHostNotifications(
  client: RpcClient,
  projectId: string | null,
  show: (notification: HostNotification) => void,
): () => void {
  let disposed = false;
  const onEvent = (event: EventMessage) => {
    if (disposed || event.topic !== HOST_NOTIFICATION_TOPIC) return;
    const payload = event.payload as unknown as HostNotification;
    if (typeof payload?.message !== "string") return;
    show({ ...payload, level: LEVELS.includes(payload.level) ? payload.level : "info" });
  };
  const topics = [HOST_NOTIFICATION_TOPIC];
  const subscriptions = [
    client.subscribe({ topics }, onEvent),
    ...(projectId === null ? [] : [client.subscribe({ projectId, topics }, onEvent)]),
  ];
  for (const opened of subscriptions) {
    opened.catch((error: unknown) =>
      console.warn("[notifications] could not follow host notifications", error),
    );
  }
  return () => {
    disposed = true;
    for (const opened of subscriptions) {
      opened.then((s) => s.unsubscribe()).catch(() => {});
    }
  };
}

/** `client` 의 Host 알림을 이 창의 알림으로 띄운다. `NotificationProvider` 안에서 부른다. */
export function useHostNotifications(client: RpcClient | undefined, projectId?: string): void {
  const notify = useNotify();
  const { l10n } = useLocalization();
  const l10nRef = useRef(l10n);
  l10nRef.current = l10n;
  useEffect(() => {
    if (!client) return;
    return connectHostNotifications(client, projectId ?? null, ({ level, message, args, body }) =>
      notify({
        level,
        title: l10nRef.current.getString(message, args ?? null),
        ...(body !== undefined && { body }),
      }),
    );
  }, [client, projectId, notify]);
}

/** 이 창에 Host 알림을 띄운다. `NotificationProvider` 안에 둔다. 그리는 것은 없다. */
export function HostNotifications({ projectId }: { projectId?: string }) {
  useHostNotifications(useHost().client, projectId);
  return null;
}

const ICONS = { info: Info, warning: TriangleAlert, error: CircleAlert } as const;

function time(at: number) {
  return new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** 패널이 화면에 보이는가. 탭이 뒤에 있거나 dock 이 접혀 있으면 아니다. 패널이 다른 그룹으로 옮겨지면 그 그룹을 본다. */
/** 툴 윈도우 패널이 지금 보이는가(그 그룹이 활성 탭으로 보이고 dock 이 펼쳐짐). */
export function usePanelVisible(api: IDockviewPanelProps["api"]): boolean {
  const read = useCallback(() => api.isVisible && !api.group.api.isCollapsed(), [api]);
  const [visible, setVisible] = useState(read);
  useEffect(() => {
    const update = () => setVisible(read());
    let collapsed = api.group.api.onDidCollapsedChange(update);
    const group = api.onDidGroupChange(() => {
      collapsed.dispose();
      collapsed = api.group.api.onDidCollapsedChange(update);
      update();
    });
    const visibility = api.onDidVisibilityChange(update);
    update();
    return () => {
      collapsed.dispose();
      group.dispose();
      visibility.dispose();
    };
  }, [api, read]);
  return visible;
}

/**
 * 알림 툴 윈도우. 보이는 동안 온 오류는 본 것으로 친다. dockview 는 뒤에 있는 패널의 컴포넌트도 마운트해 두므로 마운트가
 * 아니라 보임으로 판단한다.
 */
export function NotificationsToolWindow({ api }: IDockviewPanelProps) {
  const T = useProjectStrings().notifications;
  const items = useNotifications((s) => s.items);
  const markSeen = useNotifications((s) => s.markSeen);
  const clear = useNotifications((s) => s.clear);
  const visible = usePanelVisible(api);
  useEffect(() => {
    if (visible) markSeen();
  }, [visible, items, markSeen]);
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-[12px]"
    >
      <div className="flex flex-none justify-end border-b border-ui-border px-2 py-0.5">
        <button
          type="button"
          disabled={items.length === 0}
          onClick={clear}
          className="rounded-sm px-1 text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground disabled:opacity-50"
        >
          {T.clear}
        </button>
      </div>
      {items.length === 0 ? (
        <div className="flex flex-1 items-center justify-center text-ui-muted-foreground">
          {T.empty}
        </div>
      ) : (
        <ul className="min-h-0 flex-1 overflow-auto">
          {items.map((n) => {
            const Icon = ICONS[n.level];
            return (
              <li
                key={n.id}
                data-level={n.level}
                className="flex gap-2 border-b border-ui-border px-2 py-1"
              >
                <Icon
                  size={14}
                  aria-label={T.level(n.level)}
                  className={`mt-0.5 flex-none ${n.level === "error" ? "text-ui-accent" : "text-ui-muted-foreground"}`}
                />
                <div className="min-w-0 flex-1">
                  <div className="flex gap-2">
                    <span className="min-w-0 flex-1 font-medium">{n.title}</span>
                    <time className="flex-none text-ui-muted-foreground">{time(n.at)}</time>
                  </div>
                  {n.body && <div className="break-words text-ui-muted-foreground">{n.body}</div>}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** 상태바의 알림 아이콘. 보지 않은 오류가 있으면 강조하고 수를 보인다. 누르면 알림 툴 윈도우를 연다(닫는다). */
export function NotificationsStatusItem() {
  const T = useProjectStrings().status;
  const unseen = useNotifications((s) => s.unseenErrors);
  const { registry } = useKeymap();
  const label = T.notifications(unseen);
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      data-unseen-errors={unseen}
      onClick={() => registry.run("toolWindow.notifications")}
      className={`flex items-center gap-0.5 rounded-sm px-1 hover:bg-ui-hover ${unseen > 0 ? "bg-ui-accent text-ui-accent-foreground" : ""}`}
    >
      <Bell size={11} aria-hidden />
      {unseen > 0 && <span>{unseen}</span>}
    </button>
  );
}
