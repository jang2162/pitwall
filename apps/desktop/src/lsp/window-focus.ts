// 이 창의 label 과 앱 포커스(Tauri) — 서버의 `window/showDocument` 를 보일 창을 가르고(Host 가 이벤트에 그 창의 label 을 싣는다),
// `takeFocus` 일 때 창을 앞으로 가져온다(15-m4-plan M4-08 리뷰 M2). Tauri 밖(시험)에서는 label 이 없고(`null`) 창을 건드리지 않는다.
// 권한은 src-tauri/capabilities/project.json(창 목록 · 포커스 묻기 · 포커스 주기).

import { getAllWebviewWindows, getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

/** 이 창의 label(Host 의 `Caller::window` 와 같은 값). Tauri 밖이면 `null`. */
export function windowLabel(): string | null {
  try {
    return getCurrentWebviewWindow().label;
  } catch {
    return null;
  }
}

/**
 * 이 창을 앞으로 가져온다 — 앱이 포커스를 가지고 있을 때만(앱의 다른 창이 포커스를 가졌을 때). 이 창이 이미 포커스를 가졌으면
 * 할 일이 없고, 사람이 다른 앱을 쓰는 중이면 그 앱에서 포커스를 빼앗지 않는다.
 */
export async function bringForward(): Promise<void> {
  if (document.hasFocus()) return;
  try {
    const windows = await getAllWebviewWindows();
    const focused = await Promise.all(windows.map((w) => w.isFocused()));
    if (!focused.some(Boolean)) return;
    await getCurrentWebviewWindow().setFocus();
  } catch {
    // Tauri 밖이거나 창이 닫히는 중 — 앞으로 가져오지 않는다.
  }
}
