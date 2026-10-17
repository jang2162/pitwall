// UI 언어(docs/design/07-extensibility.md §4.1). 리소스는 Rust 와 같은 `.ftl` 파일이다 — 확장 레지스트리(crates/pw-i18n 의
// `i18n.get`)가 고른 언어 순서와 그 리소스를 주고, 그 응답 전의 첫 그림은 번들에 묶은 내장 확장(extensions/i18n-default)의 같은
// 파일로 그린다(테마 · 키맵과 같은 방식, 07 「3. 테마」 「기본 테마」 행). 리소스를 따로 베껴 두지 않는다.

import { FluentBundle, FluentResource, type FluentVariable } from "@fluent/bundle";
import { ReactLocalization } from "@fluent/react";

/** 폴백 언어(crates/pw-i18n 의 `FALLBACK`). 모든 메시지가 있다. */
export const FALLBACK = "en";

export interface Resource {
  locale: string;
  /** 사람이 읽는 위치(오류 보고용). */
  file: string;
  source: string;
}

// 내장 확장의 번역 파일. Rust 는 같은 폴더를 바이너리에 묶는다(crates/pw-ext/build.rs).
const files = import.meta.glob<string>("../../../../extensions/i18n-default/i18n/*/*.ftl", {
  query: "?raw",
  import: "default",
  eager: true,
});

/** 번들에 묶은 내장 번역. 파일 이름순이다(Rust 레지스트리와 같다). */
export const BUNDLED_RESOURCES: readonly Resource[] = Object.entries(files)
  .map(([path, source]) => {
    const [locale = "", name = ""] = path.split("/").slice(-2);
    return { locale, file: `builtin:i18n-default/i18n/${locale}/${name}`, source };
  })
  .sort((a, b) => a.file.localeCompare(b.file));

export const BUNDLED_LOCALES: readonly string[] = [
  ...new Set(BUNDLED_RESOURCES.map((r) => r.locale)),
];

/** 이 언어 순서의 번들. 양방향 격리 문자를 넣지 않는다 — 이유는 crates/pw-i18n 의 `bundle`. */
export function buildBundles(
  resources: readonly Resource[],
  locales: readonly string[],
  report: (message: string) => void,
): FluentBundle[] {
  return locales.map((locale) => {
    const bundle = new FluentBundle(locale, { useIsolating: false });
    for (const r of resources) {
      if (r.locale !== locale) continue;
      const resource = new FluentResource(r.source);
      for (const error of bundle.addResource(resource)) report(`${r.file}: ${error.message}`);
    }
    return bundle;
  });
}

export function buildLocalization(
  resources: readonly Resource[],
  locales: readonly string[],
  report: (message: string) => void,
): ReactLocalization {
  return new ReactLocalization(buildBundles(resources, locales, report), null, (error) =>
    report(error.message),
  );
}

/**
 * 레지스트리 응답 전에 쓸 언어 순서. 마지막으로 받은 순서(같은 origin 의 창들이 함께 쓴다)가 번들에 있으면 그것, 없으면
 * 브라우저 언어 중 번들에 있는 첫 언어(주 언어 마디로 맞춘다), 끝에 폴백. 레지스트리 응답이 오면 그것으로 바꾼다.
 */
export function initialLocales(
  remembered: readonly string[] | undefined,
  browser: readonly string[],
): string[] {
  const known = (l: string) => BUNDLED_LOCALES.includes(l);
  const chosen =
    remembered?.find(known) ??
    browser
      .map((tag) => BUNDLED_LOCALES.find((l) => l === tag.split("-")[0]?.toLowerCase()))
      .find((l) => l !== undefined) ??
    FALLBACK;
  return chosen === FALLBACK ? [FALLBACK] : [chosen, FALLBACK];
}

/**
 * 언어 고르기(설정 창)에 보일 값. `"system"` 이면 그대로, 아니면 **실제로 쓰는 언어**(`locales[0]`)다 — 설정 값을 사람이
 * `ko-KR` · `KO` 처럼 등록 태그와 다르게 적었거나 설치되지 않은 언어를 적었어도, 목록에 있는 값 중 화면에 쓰이는 언어를
 * 보인다(목록에 없는 값이면 controlled select 가 첫 옵션 「시스템」을 보여 오해를 낳는다). 설정 값은 고쳐 쓰지 않는다.
 */
export function selectedLanguage(setting: string, locales: readonly string[]): string {
  return setting === "system" ? "system" : (locales[0] ?? FALLBACK);
}

/** 메시지 하나를 만든다. 없는 메시지는 id 를 그대로 낸다(`ReactLocalization.getString`). */
export type Translate = (id: string, vars?: Record<string, FluentVariable>) => string;

export function translator(l10n: ReactLocalization): Translate {
  return (id, vars) => l10n.getString(id, vars ?? null);
}
