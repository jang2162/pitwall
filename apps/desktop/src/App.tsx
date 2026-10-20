import type { UiLogger } from "@pitwall/logger";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useMemo, useState } from "react";

import { useAppStrings } from "./i18n/app-strings";
import { I18nProvider, useI18n } from "./i18n/i18n-provider";
import { selectedLanguage } from "./i18n/localization";

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

// 설정 화면(10-ui §2.7)이 들어오기 전까지 외관 > 테마 · 언어만 둔다.
function SettingsWindow() {
  const T = useAppStrings().settings;
  return (
    <main className="flex-1 px-6 py-4">
      <h2 className="mb-2 text-sm font-semibold">{T.appearance}</h2>
      <div className="flex items-center gap-3">
        <span>{T.theme}</span>
        <ThemeSwitcher />
      </div>
      <LanguageSelect />
    </main>
  );
}

/** UI 언어(settings.toml 의 `ui.language`). 고르면 `i18n.setLanguage` 가 그 키만 고쳐 쓰고, 바뀐 언어는 `config.changed` 로 온다. */
function LanguageSelect() {
  const T = useAppStrings().settings;
  const { setting, locales, languages, setLanguage } = useI18n();
  const [error, setError] = useState<string>();
  return (
    <div className="mt-2 flex items-center gap-3">
      <label className="flex items-center gap-3">
        <span>{T.language}</span>
        <select
          value={selectedLanguage(setting ?? "system", locales)}
          disabled={setting === undefined}
          onChange={(e) => {
            setError(undefined);
            setLanguage(e.target.value).catch((err: unknown) =>
              setError(T.languageFailed(err instanceof Error ? err.message : String(err))),
            );
          }}
          className="rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
        >
          <option value="system">{T.languageSystem}</option>
          {languages.map((l) => (
            <option key={l.locale} value={l.locale}>
              {l.name}
            </option>
          ))}
        </select>
      </label>
      {error && (
        <span role="alert" className="text-[12px] text-ui-accent">
          {error}
        </span>
      )}
    </div>
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
        <I18nProvider log={log}>
          <KeymapProvider native={native} log={log}>
            <AppActions native={native} kind={kind} />
            <div className="flex h-full flex-col bg-ui-background text-ui-foreground">
              {/* macOS 에서는 웹뷰가 타이틀바 자리까지 덮으므로 이 줄이 창을 옮기는 손잡이가 된다. */}
              <header className="titlebar bg-ui-toolbar" data-tauri-drag-region>
                <WindowTitle kind={kind} />
              </header>
              {kind === "welcome" && <Welcome native={welcomeNative} log={log} />}
              {kind === "project" && <ProjectWindow log={log} closeWindow={native.closeWindow} />}
              {kind === "settings" && <SettingsWindow />}
              <KeymapErrorBar />
            </div>
          </KeymapProvider>
        </I18nProvider>
      </HostProvider>
    </QueryClientProvider>
  );
}
