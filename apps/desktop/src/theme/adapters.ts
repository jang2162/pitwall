// theme-store 의 입출력을 웹뷰 · Tauri 에 잇는다.

import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  type NativeWindowTheme,
  parsePreference,
  type PreferenceStorage,
  type SystemAppearance,
} from "./theme-store";
import { type Appearance, applyTokens, type Theme } from "./tokens";

// 설정 쓰기 경로(RPC)가 생기면 설정으로 이전한다. 지금은 웹뷰의 localStorage 에 둔다 — 설정 계층은 읽기만 있고 UI 가
// 코어 설정 파일을 직접 쓰지 않으려는 임시 자리다.
// localStorage 는 origin 별이라 `mise run dev` 의 개발 서버(워크트리마다 다른 포트)와 빌드본은 서로의 값을 보지 못한다.
export const THEME_STORAGE_KEY = "pitwall.theme.preference";

export function mediaAppearance(win: Pick<Window, "matchMedia">): SystemAppearance {
  const query = win.matchMedia("(prefers-color-scheme: dark)");
  const read = (dark: boolean): Appearance => (dark ? "dark" : "light");
  return {
    current: () => read(query.matches),
    subscribe(listener) {
      const onChange = (event: MediaQueryListEvent) => listener(read(event.matches));
      query.addEventListener("change", onChange);
      return () => query.removeEventListener("change", onChange);
    },
  };
}

/**
 * 한 창에서 고른 테마를 다른 창(웰컴 · 프로젝트 · 설정)도 따른다. `storage` 이벤트는 값을 쓴 창 말고 같은 origin 의
 * 다른 창에만 온다.
 */
export function localPreferenceStorage(
  win: Pick<Window, "localStorage" | "addEventListener" | "removeEventListener">,
): PreferenceStorage {
  return {
    read() {
      try {
        return parsePreference(win.localStorage.getItem(THEME_STORAGE_KEY));
      } catch {
        return undefined;
      }
    },
    write(preference) {
      try {
        win.localStorage.setItem(THEME_STORAGE_KEY, preference);
      } catch (error) {
        console.warn("[theme] 테마 선택을 저장하지 못했다", error);
      }
    },
    subscribe(listener) {
      const onStorage = (event: StorageEvent) => {
        if (event.key !== THEME_STORAGE_KEY) return;
        listener(parsePreference(event.newValue) ?? "system");
      };
      win.addEventListener("storage", onStorage);
      return () => win.removeEventListener("storage", onStorage);
    },
  };
}

/** Tauri 창 밖(브라우저로 연 개발 서버 등)이면 undefined. */
export function tauriWindowTheme(): NativeWindowTheme | undefined {
  let win: ReturnType<typeof getCurrentWindow>;
  try {
    win = getCurrentWindow();
  } catch {
    return undefined;
  }
  return {
    set: (theme) => win.setTheme(theme),
    subscribe(listener) {
      const unlisten = win.onThemeChanged(({ payload }) => listener(payload));
      unlisten.catch((error: unknown) => console.warn("[theme] 창 테마 변화를 듣지 못했다", error));
      return () => void unlisten.then((stop) => stop()).catch(() => {});
    },
  };
}

/** 문서 루트에 토큰 · `data-theme` · `color-scheme` 을 쓴다. `color-scheme` 이 웹뷰의 스크롤바 · 폼 컨트롤 색을 정한다. */
export function documentThemeApplier(root: HTMLElement) {
  return (theme: Theme, appearance: Appearance) => {
    const values = theme.variants[appearance];
    if (!values) throw new Error(`테마 ${theme.id}: ${appearance} 변형이 없다`);
    applyTokens(root.style, values);
    root.dataset.theme = appearance;
    root.style.colorScheme = appearance;
  };
}
