// 창 하나의 UI 언어. `i18n.get`(Host)이 고른 언어 순서와 리소스로 `@fluent/react` 를 채운다. 전역 설정이 다시 읽히면
// (`config.changed`, rpc/host.tsx 의 무효화 규칙) 다시 불러 재시작 없이 바뀐다 — 손으로 settings.toml 을 고쳤든
// `i18n.setLanguage` 로 바꿨든 같은 길이다. 응답 전 · 실패하면 번들 리소스로 그린다(localization.ts).

import type { UiLogger } from "@pitwall/logger";
import type { LanguageInfo } from "@pitwall/rpc-client";
import { LocalizationProvider } from "@fluent/react";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo } from "react";

import { useHost, useRpcQuery } from "../rpc/host";
import {
  BUNDLED_RESOURCES,
  buildLocalization,
  initialLocales,
  type Resource,
} from "./localization";

// 마지막으로 받은 언어 순서. 같은 origin 의 창들이 함께 쓰고, 다음 창의 첫 그림을 그 언어로 그린다. 저장소가 막혀도 동작한다.
const REMEMBERED_KEY = "pitwall.i18n.locales";

function readRemembered(): string[] | undefined {
  try {
    const raw = localStorage.getItem(REMEMBERED_KEY);
    const value: unknown = raw === null ? undefined : JSON.parse(raw);
    return Array.isArray(value) && value.every((v) => typeof v === "string") ? value : undefined;
  } catch {
    return undefined;
  }
}

function remember(locales: readonly string[]) {
  try {
    localStorage.setItem(REMEMBERED_KEY, JSON.stringify(locales));
  } catch {
    // 못 남기면 다음 창이 브라우저 언어로 첫 그림을 그린다.
  }
}

export interface I18nContextValue {
  /** 쓰는 언어 순서. 첫째가 고른 언어다. */
  readonly locales: readonly string[];
  /** 설정 값(`"system"` 또는 언어 태그). 응답 전에는 없다. */
  readonly setting?: string;
  /** 등록된 언어(언어 고르기 목록). 응답 전에는 비어 있다. */
  readonly languages: readonly LanguageInfo[];
  /** 언어 설정을 바꾼다. 반영은 `config.changed` 로 온다. */
  setLanguage(language: string): Promise<void>;
}

const I18nContext = createContext<I18nContextValue | undefined>(undefined);

export function useI18n(): I18nContextValue {
  const value = useContext(I18nContext);
  if (!value) throw new Error("I18nProvider 밖에서 useI18n 을 불렀다");
  return value;
}

export function I18nProvider({
  log,
  children,
}: {
  log?: Pick<UiLogger, "info" | "warn">;
  children: ReactNode;
}) {
  const { client } = useHost();
  // 설정이 바뀌어 무효화될 때만 다시 부른다(창 포커스 · 재연결마다 부르지 않는다). 실패는 번들 리소스가 덮고 있다.
  const query = useRpcQuery("i18n.get", null, { staleTime: Infinity, retry: false });
  const state = query.data;
  const [locales, resources] = useMemo<[readonly string[], readonly Resource[]]>(
    () =>
      state
        ? [state.locales, state.resources]
        : [initialLocales(readRemembered(), navigator.languages ?? []), BUNDLED_RESOURCES],
    [state],
  );
  const l10n = useMemo(
    () =>
      buildLocalization(resources, locales, (message) =>
        log?.warn(`translation: ${message}`, "i18n"),
      ),
    [resources, locales, log],
  );
  const chosen = locales.join(",");
  useEffect(() => {
    if (!state) return;
    remember(state.locales);
    log?.info(`ui language: ${chosen} (setting ${state.setting})`, "i18n");
  }, [state, chosen, log]);
  useEffect(() => {
    if (query.error)
      log?.warn(`i18n.get failed, using the bundled translations: ${String(query.error)}`, "i18n");
  }, [query.error, log]);

  const setLanguage = useCallback(
    async (language: string) => {
      if (!client) throw new Error("not connected to the host");
      await client.call("i18n.setLanguage", { language });
    },
    [client],
  );
  const value = useMemo<I18nContextValue>(
    () => ({
      locales,
      setting: state?.setting,
      languages: state?.languages ?? [],
      setLanguage,
    }),
    [locales, state, setLanguage],
  );
  return (
    <I18nContext value={value}>
      <LocalizationProvider l10n={l10n}>{children}</LocalizationProvider>
    </I18nContext>
  );
}
