// IME(한글 등) 조합 중인 키를 가린다. 키맵 엔진은 자기 규칙을 따로 갖는다(packages/keymap engine.ts `isComposingEvent`) —
// 여기는 Radix 처럼 엔진을 거치지 않고 키를 받는 위젯용이다.

/** 조합 중이거나(isComposing) 입력기가 처리한 키(keyCode 229 — WebKit 은 조합을 끝낸 키를 이렇게 보낸다). */
export function isComposingKey(e: KeyboardEvent): boolean {
  return e.isComposing || e.keyCode === 229;
}

/** Radix Dialog 의 `onEscapeKeyDown` 에 넘긴다 — 조합을 취소하는 Esc 가 팝업까지 닫아 입력을 잃지 않게. */
export function keepOpenWhileComposing(e: KeyboardEvent) {
  if (isComposingKey(e)) e.preventDefault();
}
