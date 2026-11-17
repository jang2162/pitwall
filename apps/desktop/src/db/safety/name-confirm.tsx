// 연결 이름 치기(14-m3-plan.md M3-19 ②, 결정 M3-D20 의 등급별 확인): 파괴 등급 · 프로덕션 연결의 확인에서 사람이 연결 이름을 그대로
// 쳐야 실행 단추가 켜진다. 위험 쿼리 확인(risk-confirm.tsx)과 MCP 승인 다이얼로그(mcp/approval-dialog.tsx, 16-m5-plan M5-14)가 같은
// 칸을 쓴다. 한글 조합 중의 Enter 는 조합을 끝낼 뿐이다(친 이름을 잃지 않게). 맞는지 판정은 부르는 쪽이 `typed === name` 으로 한다.

import { isComposingKey } from "../../ui/ime";
import { useDbSafetyStrings } from "./strings";

export function ConnectionNameField({
  name,
  typed,
  onTyped,
  onSubmit,
  testId = "db-confirm-name",
}: {
  /** 쳐야 하는 연결 이름. */
  name: string;
  typed: string;
  onTyped(value: string): void;
  /** 조합 중이 아닌 Enter. */
  onSubmit(): void;
  testId?: string;
}) {
  const T = useDbSafetyStrings().confirm;
  const ok = typed === name;
  return (
    <label className="flex flex-col gap-1">
      <span>{T.namePrompt(name)}</span>
      <input
        data-testid={testId}
        aria-label={T.name}
        aria-invalid={typed !== "" && !ok}
        value={typed}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => onTyped(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || isComposingKey(e.nativeEvent)) return;
          e.preventDefault();
          onSubmit();
        }}
        className="rounded-sm border border-ui-border bg-transparent px-2 py-1 font-mono outline-none select-text focus:border-ui-accent"
      />
      {typed !== "" && !ok && <span className="text-[12px] text-ui-accent">{T.nameMismatch}</span>}
    </label>
  );
}
