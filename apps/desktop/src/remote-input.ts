// 이 Desktop 의 「마지막 사람 입력」 을 Rust 에 알린다(src-tauri/src/remote_open.rs 의 `note_input`, 18-m7-plan M7-15 ②). 원격 셸의
// `pitwall <파일>` 은 붙은 Desktop 가운데 마지막으로 사람 입력이 있던 것에서 열린다(결정 M7-D3). 키 · 포인터 입력마다 부르지 않고
// `EVERY_MS` 에 한 번만 부른다 — 무엇을 눌렀는지는 보내지 않는다(시각만).

import { invoke } from "@tauri-apps/api/core";

export const EVERY_MS = 10_000;

/** `target` 의 키 · 포인터 입력을 `EVERY_MS` 에 한 번 `note` 로 알린다. 해제 함수를 돌려준다. */
export function watchInput(
  target: Pick<Window, "addEventListener" | "removeEventListener">,
  note: () => void,
  now: () => number = () => Date.now(),
): () => void {
  let last = -Infinity;
  const seen = () => {
    const at = now();
    if (at - last < EVERY_MS) return;
    last = at;
    note();
  };
  const events = ["keydown", "pointerdown"] as const;
  for (const name of events) target.addEventListener(name, seen, { capture: true, passive: true });
  return () => {
    for (const name of events) target.removeEventListener(name, seen, { capture: true });
  };
}

export const tauriNoteInput = () => {
  invoke("note_input").catch((e: unknown) => console.warn("[remote] note_input", e));
};
