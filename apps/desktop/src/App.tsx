import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";

import { HostDemo } from "./host-demo";
import { ProjectWindow } from "./layout/project-window";
import { HostProvider } from "./rpc/host";
import { ThemeSwitcher } from "./theme/theme-switcher";
import { WINDOW_TITLES, type WindowKind } from "./window-kind";

// src-tauri/src/main.rs 의 LOCAL_HOST_ID. 프로젝트 모델이 생기기 전이라(M0-09 · M0-12) 모든 창이 Host 를 직접 고른다.
const LOCAL_HOST = { hostId: "local" };

function WelcomeWindow() {
  return (
    <>
      <main className="min-h-0 flex-1 overflow-auto">
        <HostDemo />
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

export function App({ kind }: { kind: WindowKind }) {
  const [queryClient] = useState(() => new QueryClient());
  return (
    <QueryClientProvider client={queryClient}>
      <HostProvider target={LOCAL_HOST}>
        <div className="flex h-full flex-col bg-ui-background text-ui-foreground">
          {/* macOS 에서는 웹뷰가 타이틀바 자리까지 덮으므로 이 줄이 창을 옮기는 손잡이가 된다. */}
          <header className="titlebar bg-ui-toolbar" data-tauri-drag-region>
            <span data-tauri-drag-region>{WINDOW_TITLES[kind]}</span>
          </header>
          {kind === "welcome" && <WelcomeWindow />}
          {kind === "project" && <ProjectWindow />}
          {kind === "settings" && <SettingsWindow />}
        </div>
      </HostProvider>
    </QueryClientProvider>
  );
}
