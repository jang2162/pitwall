import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import {
  documentThemeApplier,
  localPreferenceStorage,
  mediaAppearance,
  tauriWindowTheme,
} from "./theme/adapters";
import defaultTheme from "./theme/default-theme.json";
import { createThemeStore } from "./theme/theme-store";
import { ThemeStoreProvider } from "./theme/theme-switcher";
import { parseTheme } from "./theme/tokens";
import { installUiLog } from "./ui-log";
import { detectPlatform, installWebviewGuards } from "./webview-guards";
import { parseWindowKind } from "./window-kind";

const log = installUiLog(window);
log.info(`window opened: ${parseWindowKind(window.location.search)}`, "main");
const platform = detectPlatform(navigator.userAgent);
installWebviewGuards(window, platform);
document.documentElement.dataset.platform = platform.mac ? "mac" : "other";

// 렌더 전에 만든다 — 첫 그림부터 저장된 테마의 색으로 그린다.
const themeStore = createThemeStore({
  system: mediaAppearance(window),
  storage: localPreferenceStorage(window),
  native: tauriWindowTheme(),
  apply: documentThemeApplier(document.documentElement, parseTheme(defaultTheme)),
});

const root = document.getElementById("root");
if (!root) throw new Error("#root 가 index.html 에 없다");

createRoot(root).render(
  <StrictMode>
    <ThemeStoreProvider store={themeStore}>
      <App kind={parseWindowKind(window.location.search)} log={log} />
    </ThemeStoreProvider>
  </StrictMode>,
);
