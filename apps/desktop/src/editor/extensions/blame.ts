// 현재 줄 인라인 blame · `⌃⌥A` blame 주석(13-m2-plan M2-17). 구현은 src/git/gutter/blame-extension.ts 다. 켜는 조건은 거터(`gitGutter`)
// 와 같다 — 줄을 지금 텍스트로 옮기는 데 거터와 같은 diff 를 쓰므로 거터를 끄는 문서는 blame 도 끈다.

import { blameExtension } from "../../git/gutter/blame-extension";
import type { EditorFeature } from ".";

export const blame: EditorFeature = {
  id: "blame",
  extension: (c) =>
    c.git && c.judgment.sizeClass === "editable" ? blameExtension(c.git, c.path) : [],
};
