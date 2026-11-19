// 설정 창 「실행」 페이지(17-m6-plan M6-07 ③ — 10-ui.md §2.7 의 왼쪽 트리 「실행」). 실행 구성은 프로젝트의 파일이라 설정 창(프로젝트
// 없음)에서는 편집하지 않는다 — 이 자리는 프로젝트 창의 구성 편집기로 가는 길만 적는다.

import { useRunStrings } from "./strings";

export function RunSettingsSection() {
  const T = useRunStrings().settings;
  return (
    <section data-testid="settings-run">
      <h2 className="mb-2 text-sm font-semibold">{T.title}</h2>
      <p className="text-[12px] text-ui-muted-foreground">{T.body}</p>
    </section>
  );
}
