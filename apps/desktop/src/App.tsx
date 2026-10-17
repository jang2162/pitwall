import type { UiLogger } from "@pitwall/logger";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useMemo, useState } from "react";

import { appActions } from "./keymap/actions";
import {
  KeymapErrorBar,
  KeymapProvider,
  useKeymap,
  useRegisterActions,
} from "./keymap/keymap-provider";
import { type KeymapNative, tauriKeymapNative } from "./keymap/native";
import { ProjectWindow } from "./layout/project-window";
import { HostProvider } from "./rpc/host";
import { ThemeSwitcher, useTheme } from "./theme/theme-switcher";
import { tauriWelcomeNative, type WelcomeNative } from "./welcome/native";
import { WelcomeWindow } from "./welcome/welcome-window";
import { WINDOW_TITLES, type WindowKind } from "./window-kind";

// src-tauri/src/main.rs 의 LOCAL_HOST_ID. 프로젝트 모델이 생기기 전이라(M0-09 · M0-12) 모든 창이 Host 를 직접 고른다.
const LOCAL_HOST = { hostId: "local" };

function Welcome({ native }: { native: WelcomeNative }) {
  return (
    <>
      <main className="flex min-h-0 flex-1 flex-col">
        <WelcomeWindow native={native} />
      </main>
      <footer className="flex h-[22px] flex-none items-center justify-end border-t border-ui-border bg-ui-statusbar px-2 text-ui-statusbar-foreground">
        <ThemeSwitcher />
      </footer>
    </>
  );
}

// 설정 화면(10-ui §2.7)이 들어오기 전까지 외관 > 테마만 둔다.
function SettingsWindow() {
  return (
    <main className="flex-1 px-6 py-4">
      <h2 className="mb-2 text-sm font-semibold">외관</h2>
      <div className="flex items-center gap-3">
        <span>테마</span>
        <ThemeSwitcher />
      </div>
    </main>
  );
}

// 모든 창에 있는 액션. 프로젝트 창의 툴 윈도우 액션은 ProjectWindow 가 등록한다.
function AppActions({ native }: { native: KeymapNative }) {
  const { openFindAction } = useKeymap();
  const setTheme = useTheme((s) => s.setPreference);
  const actions = useMemo(
    () =>
      appActions({
        openFindAction,
        openSettings: native.openSettings,
        setTheme,
        installShellCommand: native.installShellCommand,
      }),
    [openFindAction, native, setTheme],
  );
  useRegisterActions(actions);
  return null;
}

export function App({
  kind,
  native = tauriKeymapNative,
  welcomeNative = tauriWelcomeNative,
  log,
}: {
  kind: WindowKind;
  native?: KeymapNative;
  welcomeNative?: WelcomeNative;
  log?: Pick<UiLogger, "info" | "warn" | "debug">;
}) {
  const [queryClient] = useState(() => new QueryClient());
  return (
    <QueryClientProvider client={queryClient}>
      <HostProvider target={LOCAL_HOST}>
        <KeymapProvider native={native} log={log}>
          <AppActions native={native} />
          <div className="flex h-full flex-col bg-ui-background text-ui-foreground">
            {/* macOS 에서는 웹뷰가 타이틀바 자리까지 덮으므로 이 줄이 창을 옮기는 손잡이가 된다. */}
            <header className="titlebar bg-ui-toolbar" data-tauri-drag-region>
              <span data-tauri-drag-region>{WINDOW_TITLES[kind]}</span>
            </header>
            {kind === "welcome" && <Welcome native={welcomeNative} />}
            {kind === "project" && <ProjectWindow />}
            {kind === "settings" && <SettingsWindow />}
            <KeymapErrorBar />
          </div>
        </KeymapProvider>
      </HostProvider>
    </QueryClientProvider>
  );
}
