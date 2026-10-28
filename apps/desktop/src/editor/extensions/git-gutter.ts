// 에디터 거터의 git 변경 표시 · 인라인 diff 팝업(13-m2-plan M2-17). 구현은 src/git/gutter/gutter-extension.ts 다 — 이 모듈은 켜는
// 조건만 정한다: 창이 git 호스트를 넘겼고(`context.git`, 프로젝트 창) 편집 등급(결정 M1-D9, ≤ 5 MiB) 문서일 때. 그 밖의 문서는
// 끈다(빈 확장). 저장소 밖 · untracked · 필터 드라이버가 걸린 파일은 모델이 끈다(gutter-model.ts).

import { gitGutterExtension } from "../../git/gutter/gutter-extension";
import type { EditorFeature } from ".";

export const gitGutter: EditorFeature = {
  id: "gitGutter",
  extension: (c) =>
    c.git && c.judgment.sizeClass === "editable"
      ? gitGutterExtension(c.git, c.path, c.readOnly)
      : [],
};
