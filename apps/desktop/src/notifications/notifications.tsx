// 알림(docs/design/10-ui.md §2.8 「빈 상태 · 오류 상태」, F11-5): 토스트로 잠깐 띄우고 알림 툴 윈도우에 기록으로 남긴다.
// 오류는 알림 툴 윈도우를 볼 때까지 상태바의 알림 아이콘을 강조한다. 기록은 창 하나의 메모리에만 있다 — 창을 닫으면 사라진다.
//
// 알림을 내는 법: 창 안의 컴포넌트에서 `const notify = useNotify();` 를 받아 `notify({ level: "error", title, body })`.
// 문장은 그 기능의 문자열(Fluent)로 만들어 넘긴다. `body` 에는 오류 원문(RPC 오류 메시지 등)을 그대로 둔다.

import type { IDockviewPanelProps } from "dockview-react";
import { Bell, CircleAlert, Info, TriangleAlert } from "lucide-react";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useState } from "react";
import { toast } from "sonner";
import { createStore, type StoreApi, useStore } from "zustand";

import { useKeymap } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";

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

const NotificationContext = createContext<NotificationStore | undefined>(undefined);

/** 창 하나의 알림 기록. `store` 는 시험이 넘긴다 — 없으면 새로 만든다. */
export function NotificationProvider({
  store: given,
  children,
}: {
  store?: NotificationStore;
  children: ReactNode;
}) {
  const [store] = useState(() => given ?? createNotificationStore());
  return <NotificationContext value={store}>{children}</NotificationContext>;
}

function useNotificationStore(): NotificationStore {
  const store = useContext(NotificationContext);
  if (!store) throw new Error("NotificationProvider 밖에서 알림을 썼다");
  return store;
}

export function useNotifications<T>(selector: (state: NotificationState) => T): T {
  return useStore(useNotificationStore(), selector);
}

const TOASTS = { info: toast.info, warning: toast.warning, error: toast.error } as const;

/** 알림을 기록하고 토스트로 띄우는 함수. 함수는 창이 사는 동안 같다. */
export function useNotify(): (input: NotificationInput) => void {
  const store = useNotificationStore();
  return useCallback(
    (input: NotificationInput) => {
      const item = store.getState().push(input);
      TOASTS[input.level](input.title, { id: item.id, description: input.body });
    },
    [store],
  );
}

const ICONS = { info: Info, warning: TriangleAlert, error: CircleAlert } as const;

function time(at: number) {
  return new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** 패널이 화면에 보이는가. 탭이 뒤에 있거나 dock 이 접혀 있으면 아니다. 패널이 다른 그룹으로 옮겨지면 그 그룹을 본다. */
function usePanelVisible(api: IDockviewPanelProps["api"]): boolean {
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
