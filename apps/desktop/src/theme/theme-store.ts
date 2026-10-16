// 테마 선택(시스템 · 라이트 · 다크)과 지금 적용된 외관. 화면은 zustand 스토어로 읽고, 바뀌면 `apply` 가 CSS 변수를 다시 쓴다.
//
// OS 외관은 두 곳에서 듣는다 — 웹뷰의 `prefers-color-scheme` 과 Tauri 창의 테마 변화. 수동으로 고르면 네이티브 창의
// 테마도 그 값으로 고정해(`NativeWindowTheme.set`) 신호등 버튼 · 스크롤바 · 네이티브 메뉴가 화면과 같은 외관이 되게 한다.
// 고정하는 동안에는 두 출처가 OS 가 아니라 고정한 값을 알리므로 그동안 받은 값을 OS 값으로 믿지 않는다. 시스템으로
// 돌아올 때 고정을 푼 뒤 OS 값을 다시 읽는다.
//
// 한쪽 변형만 있는 테마는 그 변형을 칠하고 네이티브 창도 그 외관으로 고정한다. 선택(`preference`)은 그대로 둔다 — 규칙의
// 정본은 docs/design/07-extensibility.md 「3. 테마」 「형식」 행.

import { createStore, type StoreApi } from "zustand/vanilla";

import { type Appearance, resolveAppearance, type Theme, themeAppearances } from "./tokens";

export type ThemePreference = "system" | Appearance;

const PREFERENCES: readonly ThemePreference[] = ["system", "light", "dark"];

export interface SystemAppearance {
  current(): Appearance;
  /** OS 외관이 바뀔 때마다 부른다. 돌려준 함수로 해제한다. */
  subscribe(listener: (appearance: Appearance) => void): () => void;
}

export interface NativeWindowTheme {
  /** `null` 이면 OS 를 따른다. */
  set(theme: Appearance | null): Promise<void>;
  /** 네이티브 창이 알리는 테마 변화. */
  subscribe?(listener: (appearance: Appearance) => void): () => void;
}

/** 선택을 저장하는 곳. 다른 창이 바꾸면 `subscribe` 로 알린다. */
export interface PreferenceStorage {
  read(): ThemePreference | undefined;
  write(preference: ThemePreference): void;
  subscribe?(listener: (preference: ThemePreference) => void): () => void;
}

export interface ThemeState {
  preference: ThemePreference;
  /** 칠한 외관. 테마에 고른 외관의 변형이 없으면 테마가 가진 변형이다. */
  appearance: Appearance;
  theme: Theme;
  setPreference(preference: ThemePreference): void;
  setTheme(theme: Theme): void;
}

export interface ThemeStoreOptions {
  system: SystemAppearance;
  storage: PreferenceStorage;
  native?: NativeWindowTheme;
  /** 처음 칠할 테마. */
  theme: Theme;
  /**
   * 테마나 외관이 정해지거나 바뀔 때 부른다. `appearance` 의 변형은 `theme` 에 늘 있다. 처음 한 번은 스토어를 만드는 중에
   * 동기로 부른다(첫 그림 전에 색이 정해진다).
   */
  apply(theme: Theme, appearance: Appearance): void;
}

export interface ThemeStore extends StoreApi<ThemeState> {
  /** 구독을 모두 푼다. */
  dispose(): void;
}

export function parsePreference(value: unknown): ThemePreference | undefined {
  return PREFERENCES.find((p) => p === value);
}

export function createThemeStore({
  system,
  storage,
  native,
  theme,
  apply,
}: ThemeStoreOptions): ThemeStore {
  let osAppearance = system.current();

  const store = createStore<ThemeState>()(() => ({
    preference: storage.read() ?? "system",
    appearance: "light",
    theme,
    setPreference(preference) {
      storage.write(preference);
      select(preference);
    },
    setTheme(next) {
      store.setState({ theme: next });
      sync();
    },
  }));

  let applied: { theme: Theme; appearance: Appearance } | undefined;
  const update = () => {
    const { preference, theme: current } = store.getState();
    const appearance = resolveAppearance(
      current,
      preference === "system" ? osAppearance : preference,
    );
    if (store.getState().appearance !== appearance) store.setState({ appearance });
    if (applied?.theme !== current || applied.appearance !== appearance) {
      applied = { theme: current, appearance };
      apply(current, appearance);
    }
  };

  // 네이티브 창에 고정한 외관. null 이면 OS 를 따른다 — 창은 처음에 OS 를 따른다.
  let pinned: Appearance | null = null;
  const pinTarget = (): Appearance | null => {
    const { preference, theme: current } = store.getState();
    const available = themeAppearances(current);
    if (available.length === 1) return available[0]!;
    return preference === "system" ? null : preference;
  };

  const pinNative = () => {
    const target = pinTarget();
    if (target === pinned) return;
    pinned = target;
    // 고정하는 동안은 OS 변화를 받지 않았다. 푸는 지금의 값을 읽는다(네이티브 고정은 아래에서 풀린 뒤 한 번 더 읽는다).
    if (target === null) osAppearance = system.current();
    if (!native) return;
    native
      .set(target)
      .then(() => {
        // 고정을 푼 뒤의 OS 값. 고정하는 동안 들어온 값은 고정한 외관이었을 수 있다.
        if (pinned !== null) return;
        osAppearance = system.current();
        update();
      })
      .catch((error: unknown) => console.warn("[theme] 창 테마를 바꾸지 못했다", error));
  };

  const sync = () => {
    pinNative();
    update();
  };

  const select = (preference: ThemePreference) => {
    if (store.getState().preference !== preference) store.setState({ preference });
    sync();
  };

  const onOs = (appearance: Appearance) => {
    if (pinned !== null) return;
    osAppearance = appearance;
    update();
  };

  // 처음부터 고정할 외관이 있으면(수동 선택 · 한쪽 변형만 있는 테마) 네이티브 창도 고정한다.
  sync();

  const unsubscribers = [
    system.subscribe(onOs),
    native?.subscribe?.(onOs),
    storage.subscribe?.((preference) => select(preference)),
  ];

  return {
    ...store,
    dispose() {
      for (const unsubscribe of unsubscribers) unsubscribe?.();
    },
  };
}
