import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import { detectPlatform, installWebviewGuards } from "./webview-guards";
import { parseWindowKind } from "./window-kind";

const platform = detectPlatform(navigator.userAgent);
installWebviewGuards(window, platform);
document.documentElement.dataset.platform = platform.mac ? "mac" : "other";

const root = document.getElementById("root");
if (!root) throw new Error("#root 가 index.html 에 없다");

createRoot(root).render(
  <StrictMode>
    <App kind={parseWindowKind(window.location.search)} />
  </StrictMode>,
);
