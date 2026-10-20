// 프로젝트 창의 상태바(docs/design/10-ui.md §2.4 「상태바」 행): 왼쪽 · 오른쪽 두 줄의 항목 슬롯. 상태바는 항목을 모를 뿐
// 자리와 순서만 정한다 — 항목은 그 기능의 코드가 등록한다.
//
// 항목을 더하는 법:
//   1. 항목 컴포넌트를 모듈 수준 함수로 만든다. props 는 없다 — 보일 값은 그 컴포넌트가 hook 으로 직접 읽는다(RPC 쿼리,
//      기능의 store). 그릴 것이 없으면 `null` 을 돌려준다 — 그때는 자리도 차지하지 않는다.
//   2. 자리는 아래 `STATUS_SLOTS` 에 한 줄로 정한다(쪽 · 순서). 한곳에 모아 두어 항목끼리의 순서를 한눈에 본다.
//   3. 프로젝트 창 안(`StatusBarProvider` 아래)에서 `useStatusBarItem` 을 부른다. 부른 컴포넌트가 마운트돼 있는 동안 보인다.
//      id 는 한 곳에서만 등록한다 — 같은 id 를 두 곳에서 등록하면 나중 것이 앞의 것을 지우고, 나중 것이 빠질 때 앞의 것도
//      되살아나지 않는다. 개발 빌드는 그때 경고를 남긴다.
//
//      function TrustStatus() { ... return trusted ? null : <span>…</span>; }
//      useStatusBarItem({ id: "trust", ...STATUS_SLOTS.trust, component: TrustStatus });
//
// 항목 컴포넌트는 상태바 자리에서 그려지므로 창의 context(Host 연결 · 키맵 · 언어 · 테마)를 쓸 수 있다. 다른 툴 윈도우를
// 여는 등 창의 동작은 키맵 액션으로 부른다(`useKeymap().registry.run(id)`) — 항목이 도킹 api 를 따로 받지 않는다.

import {
  type ComponentType,
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useState,
} from "react";
import { createStore, type StoreApi, useStore } from "zustand";

export type StatusBarSide = "left" | "right";

export interface StatusBarSlot {
  side: StatusBarSide;
  /** 같은 쪽 안의 순서. 작은 것이 왼쪽이다. 같으면 등록한 순서. */
  order: number;
}

/**
 * 항목 자리. 10-ui §2.4 의 상태바 그림 순서를 따른다 — 왼쪽은 프로젝트 · 연결 상태, 오른쪽은 작업 · 에디터 정보 · 창 도구.
 * 값 사이를 띄워 두어 새 항목이 기존 값을 바꾸지 않고 끼어든다. 아직 없는 항목의 자리도 미리 둔다(M1 계획의 행).
 */
export const STATUS_SLOTS = {
  host: { side: "left", order: 100 },
  /** 프로젝트 신뢰 · 「런타임 환경 미적용」(M1-05). */
  trust: { side: "left", order: 200 },
  /** 런타임 제공자 상태 · 경고(M1-16). */
  runtime: { side: "left", order: 300 },
  /** 백그라운드 작업(F11-5). */
  tasks: { side: "right", order: 100 },
  /** 커서 줄:열 · 선택 길이(M1-18). */
  cursor: { side: "right", order: 200 },
  /** 줄바꿈(M1-18). */
  lineEnding: { side: "right", order: 300 },
  /** 인코딩(M1-18). */
  encoding: { side: "right", order: 400 },
  /** 언어(M1-18). */
  language: { side: "right", order: 500 },
  appearance: { side: "right", order: 700 },
  /** 「외부 도구로 열기」 — 툴바가 생기면 옮긴다(10-ui §2.4 「타이틀바/툴바」 행). */
  openWith: { side: "right", order: 800 },
  notifications: { side: "right", order: 900 },
  theme: { side: "right", order: 1000 },
} as const satisfies Record<string, StatusBarSlot>;

export interface StatusBarItem extends StatusBarSlot {
  /** 같은 id 를 다시 등록하면 앞의 것을 바꾼다. 화면의 `data-status-item` 이 된다. */
  id: string;
  /** 모듈 수준 함수로 넘긴다. 렌더마다 새 함수를 주면 렌더마다 다시 등록되고 항목의 상태가 사라진다. */
  component: ComponentType;
}

interface StatusBarState {
  items: readonly StatusBarItem[];
  /** 항목을 더한다. 돌려준 함수로 뺀다 — 그사이 같은 id 로 바뀐 항목은 빼지 않는다. */
  register(item: StatusBarItem): () => void;
}

export type StatusBarStore = StoreApi<StatusBarState>;

export function createStatusBarStore(): StatusBarStore {
  return createStore<StatusBarState>()((set, get) => ({
    items: [],
    register(item) {
      if (import.meta.env.DEV && get().items.some((i) => i.id === item.id))
        console.warn(
          `[status-bar] item "${item.id}" registered twice — register an id in one place`,
        );
      set((s) => ({ items: [...s.items.filter((i) => i.id !== item.id), item] }));
      return () => set((s) => ({ items: s.items.filter((i) => i !== item) }));
    },
  }));
}

const StatusBarContext = createContext<StatusBarStore | undefined>(undefined);

function useStatusBarStore(): StatusBarStore {
  const store = useContext(StatusBarContext);
  if (!store) throw new Error("StatusBarProvider 밖에서 상태바를 썼다");
  return store;
}

/** 창 하나의 상태바 항목 표. 항목을 등록하는 컴포넌트와 `StatusBar` 를 모두 감싼다. */
export function StatusBarProvider({ children }: { children: ReactNode }) {
  const [store] = useState(createStatusBarStore);
  return <StatusBarContext value={store}>{children}</StatusBarContext>;
}

/** 이 컴포넌트가 마운트돼 있는 동안 상태바에 항목을 둔다. 사용법은 머리 주석. */
export function useStatusBarItem({ id, side, order, component }: StatusBarItem) {
  const store = useStatusBarStore();
  useEffect(
    () => store.getState().register({ id, side, order, component }),
    [store, id, side, order, component],
  );
}

/** 한 쪽의 항목. 순서가 같으면 등록한 순서다(`Array.prototype.sort` 는 안정 정렬이다). */
export function itemsOn(items: readonly StatusBarItem[], side: StatusBarSide): StatusBarItem[] {
  return items.filter((i) => i.side === side).sort((a, b) => a.order - b.order);
}

function Items({ items }: { items: readonly StatusBarItem[] }) {
  return items.map(({ id, component: Item }) => (
    <div key={id} data-status-item={id} className="flex items-center empty:hidden">
      <Item />
    </div>
  ));
}

export function StatusBar({ label }: { label: string }) {
  const items = useStore(useStatusBarStore(), (s) => s.items);
  return (
    <footer
      aria-label={label}
      className="flex h-[22px] flex-none items-center gap-3 border-t border-ui-border bg-ui-statusbar px-2 text-[11px] text-ui-statusbar-foreground"
    >
      <Items items={itemsOn(items, "left")} />
      <span className="flex-1" />
      <Items items={itemsOn(items, "right")} />
    </footer>
  );
}
