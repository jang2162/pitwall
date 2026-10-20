// 설정 창. 설정 화면(10-ui §2.7)이 들어오기 전까지 외관 > 테마 · 언어와 외부 도구 > Orca 에서 Pitwall 열기(F8-6)만 둔다.

import { useState } from "react";

import { useI18n } from "../i18n/i18n-provider";
import { selectedLanguage } from "../i18n/localization";
import { ThemeSwitcher } from "../theme/theme-switcher";
import type { SettingsNative } from "./native";
import { OrcaSection } from "./orca-section";
import { useSettingsStrings } from "./strings";

export function SettingsWindow({ native }: { native: SettingsNative }) {
  const S = useSettingsStrings();
  const T = S.appearance;
  return (
    <main className="flex-1 overflow-y-auto px-6 py-4">
      <h2 className="mb-2 text-sm font-semibold">{T.title}</h2>
      <div className="flex items-center gap-3">
        <span>{T.theme}</span>
        <ThemeSwitcher />
      </div>
      <LanguageSelect />
      <h2 className="mt-6 mb-2 text-sm font-semibold">{S.externalTools}</h2>
      <OrcaSection native={native} />
    </main>
  );
}

/** UI 언어(settings.toml 의 `ui.language`). 고르면 `i18n.setLanguage` 가 그 키만 고쳐 쓰고, 바뀐 언어는 `config.changed` 로 온다. */
function LanguageSelect() {
  const T = useSettingsStrings().appearance;
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
