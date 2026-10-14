import { HostDemo } from "./host-demo";
import { WINDOW_TITLES, type WindowKind } from "./window-kind";

export function App({ kind }: { kind: WindowKind }) {
  return (
    <div className="window">
      {/* macOS 에서는 웹뷰가 타이틀바 자리까지 덮으므로 이 줄이 창을 옮기는 손잡이가 된다. */}
      <header className="titlebar" data-tauri-drag-region>
        <span data-tauri-drag-region>{WINDOW_TITLES[kind]}</span>
      </header>
      <main className="content">{kind === "welcome" && <HostDemo />}</main>
    </div>
  );
}
