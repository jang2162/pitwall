// 에디터 탭 자리의 안내(읽는 중 · 열지 않음 · 뷰어 없음 등, 10-ui §2.8 「빈 상태 · 오류 상태」). 열 수 없는 파일은 「외부 도구로
// 열기」 버튼을 함께 둔다 — 그 액션(`openWith.popup`)이 활성 탭의 파일을 문맥으로 넘긴다.

import { useAppStrings } from "../i18n/app-strings";
import { useKeymap } from "../keymap/keymap-provider";

export function EditorNotice({ text, openWith = true }: { text: string; openWith?: boolean }) {
  const T = useAppStrings().openWith;
  const { registry } = useKeymap();
  return (
    <div
      role="status"
      className="flex h-full flex-col items-center justify-center gap-2 bg-editor-background px-4 text-center text-ui-muted-foreground"
    >
      <p>{text}</p>
      {openWith && (
        <button
          type="button"
          onClick={() => registry.run("openWith.popup")}
          className="rounded-sm border border-ui-border px-2 py-0.5 text-ui-foreground hover:bg-ui-hover"
        >
          {T.action}
        </button>
      )}
    </div>
  );
}
