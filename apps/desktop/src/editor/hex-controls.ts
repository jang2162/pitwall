// 활성 탭의 hex 뷰어(hex-viewer.tsx)를 부르는 길: 패널 id → 조작. 액션 `hex.gotoOffset`(editor-actions.ts)이 쓴다. 뷰어 컴포넌트와
// 따로 두어 액션 모듈이 뷰어를 import 하지 않는다.

export const hexControls = new Map<string, { gotoOffset(): void }>();
