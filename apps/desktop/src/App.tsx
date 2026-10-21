import type { UiLogger } from "@pitwall/logger";
import type { DockviewApi } from "dockview-react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useMemo, useState } from "react";

import { useAppStrings } from "./i18n/app-strings";
import { I18nProvider, useI18n } from "./i18n/i18n-provider";

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
import { type SettingsNative, tauriSettingsNative } from "./settings/native";
import { SettingsWindow } from "./settings/settings-window";
import { ThemeSwitcher, useTheme } from "./theme/theme-switcher";
import { tauriWelcomeNative, type WelcomeNative } from "./welcome/native";
import { type WelcomeLog, WelcomeWindow } from "./welcome/welcome-window";
import { WELCOME_TITLE, type WindowKind } from "./window-kind";

// src-tauri/src/main.rs 의 LOCAL_HOST_ID. 프로젝트 모델이 생기기 전이라(M0-09 · M0-12) 모든 창이 Host 를 직접 고른다.
const LOCAL_HOST = { hostId: "local" };

function Welcome({ native, log }: { native: WelcomeNative; log?: WelcomeLog }) {
  return (
    <>
      <main className="flex min-h-0 flex-1 flex-col">
        <WelcomeWindow native={native} log={log} />
      </main>
      <footer className="flex h-[22px] flex-none items-center justify-end border-t border-ui-border bg-ui-statusbar px-2 text-ui-statusbar-foreground">
        <ThemeSwitcher />
      </footer>
    </>
  );
}

// 모든 창에 있는 액션. 프로젝트 창의 툴 윈도우 액션은 ProjectWindow 가 등록한다. `window.close` 는 프로젝트 창이면
// ProjectWindow 가(탭을 먼저 닫는다), 아니면 여기서 등록한다.
function AppActions({ native, kind }: { native: KeymapNative; kind: WindowKind }) {
  const { openFindAction } = useKeymap();
  const setTheme = useTheme((s) => s.setPreference);
  const strings = useAppStrings();
  const { languages, setLanguage } = useI18n();
  const actions = useMemo(
    () =>
      appActions({
        strings,
        languages,
        setLanguage,
        openFindAction,
        openSettings: native.openSettings,
        setTheme,
        installShellCommand: native.installShellCommand,
        closeWindow: kind === "project" ? undefined : native.closeWindow,
      }),
    [strings, languages, setLanguage, openFindAction, native, setTheme, kind],
  );
  useRegisterActions(actions);
  return null;
}

function WindowTitle({ kind }: { kind: WindowKind }) {
  const titles = useAppStrings().windowTitles;
  return <span data-tauri-drag-region>{kind === "welcome" ? WELCOME_TITLE : titles[kind]}</span>;
}

export function App({
  kind,
  native = tauriKeymapNative,
  welcomeNative = tauriWelcomeNative,
  settingsNative = tauriSettingsNative,
  log,
  onDockApi,
}: {
  kind: WindowKind;
  native?: KeymapNative;
  welcomeNative?: WelcomeNative;
  settingsNative?: SettingsNative;
  log?: Pick<UiLogger, "info" | "warn" | "debug">;
  /** 프로젝트 창의 도킹 영역 api(시험). */
  onDockApi?(api: DockviewApi): void;
}) {
  const [queryClient] = useState(() => new QueryClient());
  return (
    <QueryClientProvider client={queryClient}>
      <HostProvider target={LOCAL_HOST}>
        <I18nProvider log={log}>
          <KeymapProvider native={native} log={log}>
            <AppActions native={native} kind={kind} />
            <div className="flex h-full flex-col bg-ui-background text-ui-foreground">
              {/* macOS 에서는 웹뷰가 타이틀바 자리까지 덮으므로 이 줄이 창을 옮기는 손잡이가 된다. */}
              <header className="titlebar bg-ui-toolbar" data-tauri-drag-region>
                <WindowTitle kind={kind} />
              </header>
              {kind === "welcome" && <Welcome native={welcomeNative} log={log} />}
              {kind === "project" && (
                <ProjectWindow
                  log={log}
                  closeWindow={native.closeWindow}
                  guardClose={native.guardClose}
                  onDockApi={onDockApi}
                />
              )}
              {kind === "settings" && <SettingsWindow native={settingsNative} />}
              <KeymapErrorBar />
            </div>
          </KeymapProvider>
        </I18nProvider>
      </HostProvider>
    </QueryClientProvider>
  );
}
