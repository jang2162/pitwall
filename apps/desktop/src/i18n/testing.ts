// 시험용: 번들 리소스(내장 확장의 .ftl)로 만든 문자열과, 가짜 Host 가 `i18n.get` 에 돌려줄 응답. 시험은 언어를 이것으로
// 고정한다 — 브라우저 언어(happy-dom 은 en-US)에 따라 화면 문자열이 바뀌지 않게.

import type { I18nState } from "@pitwall/rpc-client";

import { welcomeStrings } from "../welcome/strings";
import { appStrings } from "./app-strings";
import {
  BUNDLED_RESOURCES,
  buildLocalization,
  FALLBACK,
  type Translate,
  translator,
} from "./localization";

const LANGUAGE_NAMES: Record<string, string> = { en: "English", ko: "한국어" };

/** `locale` 다음에 폴백. 번역 오류는 시험을 실패시킨다. */
export function chainFor(locale: string): string[] {
  return locale === FALLBACK ? [FALLBACK] : [locale, FALLBACK];
}

export function bundledTranslator(locale: string): Translate {
  return translator(
    buildLocalization(BUNDLED_RESOURCES, chainFor(locale), (message) => {
      throw new Error(message);
    }),
  );
}

export const KO_APP = appStrings(bundledTranslator("ko"));
export const KO_WELCOME = welcomeStrings(bundledTranslator("ko"));

/** 가짜 Host 의 `i18n.get` 응답. 실제 Host 처럼 고른 언어 순서의 리소스만 싣는다. */
export function i18nState(locale: string, setting = locale): I18nState {
  const locales = chainFor(locale);
  return {
    setting,
    locales,
    languages: ["en", "ko"].map((l) => ({
      locale: l,
      name: LANGUAGE_NAMES[l]!,
      extension: "dev.pitwall.i18n-default",
    })),
    resources: locales.flatMap((l) =>
      BUNDLED_RESOURCES.filter((r) => r.locale === l).map((r) => ({
        ...r,
        extension: "dev.pitwall.i18n-default",
      })),
    ),
  };
}
