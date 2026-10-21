// Markdown 미리보기의 변환 규칙(12-m1-plan M1-22, 결정 M1-D11). 화면은 markdown-viewer.tsx 다.
//
// - `markdown-it` 에 raw HTML 을 끄고(`html: false` — `<script>` · `<img onerror>` 는 글자로 보인다) GFM 표 · 취소선 · 체크박스를
//   켠다. 체크박스는 `[ ]` · `[x]` 로 시작하는 목록 항목을 읽기 전용 체크박스로 바꾼다(아래 `taskLists`).
// - 이미지는 `src` 없이 `data-pw-src` 에 원래 주소를 담아 낸다 — 화면이 그 주소를 `resolveLocal` 로 풀어 `fs.read` 로 읽은 Blob
//   URL 을 넣는다. 웹뷰가 주소를 직접 읽지 않으므로 원격 이미지(추적 픽셀)도 · 루트 밖 파일도 열리지 않는다.
// - 링크는 화면이 누름을 가로채 `classifyLink` 로 고른다: http(s) · mailto 는 외부 브라우저, 상대 경로는 앱의 탭, 나머지 스킴
//   (`javascript:` · `file:` · `data:` …)은 열지 않는다. `markdown-it` 도 위험한 스킴의 링크를 만들지 않지만(`validateLink`)
//   그것에 기대지 않는다.
//
// 규칙 A(루트 밖은 그리지도 열지도 않는다): 상대 경로만 받고 파일의 폴더를 기준으로 풀어 루트 안일 때만 경로를 준다. 절대 경로 ·
// `../` 로 루트를 벗어나는 경로는 여기서 막고, 루트 안 경로가 루트 밖을 가리키는 심링크면 Host 의 `fs.read` 가 거절한다
// (crates/pw-fs/src/scope.rs).

import MarkdownIt from "markdown-it";
import type StateCore from "markdown-it/lib/rules_core/state_core.mjs";

/** 렌더한 HTML 에서 이미지의 원래 주소를 담는 속성. */
export const IMAGE_SOURCE_ATTR = "data-pw-src";

/** `[ ]` · `[x]` 로 시작하는 목록 항목을 체크박스로. 항목의 첫 인라인 글자에서 표식을 떼고 그 앞에 체크박스를 둔다. */
function taskLists(md: MarkdownIt) {
  md.core.ruler.after("inline", "pw-task-lists", (state: StateCore) => {
    const tokens = state.tokens;
    for (let i = 2; i < tokens.length; i++) {
      const inline = tokens[i]!;
      if (
        inline.type !== "inline" ||
        tokens[i - 1]!.type !== "paragraph_open" ||
        tokens[i - 2]!.type !== "list_item_open"
      )
        continue;
      const first = inline.children?.[0];
      const mark = first?.type === "text" ? /^\[([ xX])\]\s/.exec(first.content) : null;
      if (!first || !mark) continue;
      first.content = first.content.slice(mark[0].length);
      const box = new state.Token("html_inline", "", 0);
      const checked = mark[1] !== " ";
      box.content = `<input type="checkbox" class="pw-task" disabled${checked ? " checked" : ""}> `;
      inline.children!.unshift(box);
      tokens[i - 2]!.attrJoin("class", "pw-task-item");
    }
  });
}

function createRenderer(): MarkdownIt {
  const md = new MarkdownIt({ html: false, linkify: true, typographer: false });
  md.use(taskLists);
  const image = md.renderer.rules.image!;
  md.renderer.rules.image = (tokens, idx, options, env, self) => {
    const token = tokens[idx]!;
    const src = token.attrGet("src");
    if (src !== null) {
      token.attrs = token.attrs!.filter(([name]) => name !== "src");
      token.attrSet(IMAGE_SOURCE_ATTR, src);
    }
    return image(tokens, idx, options, env, self);
  };
  return md;
}

let renderer: MarkdownIt | undefined;

/** Markdown 텍스트 → 미리보기 HTML. */
export function renderMarkdown(text: string): string {
  renderer ??= createRenderer();
  return renderer.render(text);
}

const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
const EXTERNAL_SCHEMES = new Set(["http:", "https:", "mailto:"]);

/** 경로를 구분자로 나눈다. Windows 경로(`C:\`)도 같은 규칙이다. */
function segments(path: string): string[] {
  return path.split(/[\\/]/);
}

/** 뒤에 오는 `?query` · `#fragment` 를 떼고 퍼센트 인코딩을 푼다. 풀 수 없으면 그대로. */
function pathPart(href: string): string {
  const bare = href.replace(/[?#].*$/s, "");
  try {
    return decodeURIComponent(bare);
  } catch {
    return bare;
  }
}

/**
 * 문서 `from`(절대 경로)에 적힌 주소 `href` 를 루트 `root` 안의 절대 경로로 푼다. 스킴이 있는 주소 · 절대 경로 · 루트를 벗어나는
 * 경로면 `undefined`(규칙 A).
 */
export function resolveLocal(href: string, from: string, root: string): string | undefined {
  const trimmed = href.trim();
  // 퍼센트 인코딩을 푼 뒤에 본다 — `%2Fetc` 도 절대 경로다.
  const relative = pathPart(trimmed);
  if (SCHEME.test(trimmed) || relative === "" || /^[\\/]/.test(relative) || SCHEME.test(relative))
    return undefined;
  const separator = from.includes("/") ? "/" : "\\";
  const base = segments(from).slice(0, -1);
  const out = [...base];
  for (const part of segments(relative)) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (out.length <= 1) return undefined;
      out.pop();
    } else out.push(part);
  }
  const resolved = out.join(separator);
  const top = segments(root).filter((p, i) => p !== "" || i === 0);
  const within = top.every((part, i) => out[i] === part);
  return within && out.length > top.length ? resolved : undefined;
}

export type LinkTarget =
  /** 외부 브라우저(또는 메일 앱)로 연다. */
  | { kind: "external"; url: string }
  /** 같은 문서 안의 자리. 미리보기 안에서 그 제목으로 간다. */
  | { kind: "anchor"; id: string }
  /** 루트 안의 파일. 앱의 탭으로 연다. */
  | { kind: "file"; path: string }
  /** 열지 않는다(위험한 스킴 · 루트 밖 · 알 수 없는 주소). */
  | { kind: "blocked" };

/** 미리보기의 링크 `href` 를 누르면 무엇을 할지. 경로는 `resolveLocal` 과 같은 규칙이다. */
export function classifyLink(href: string, from: string, root: string | undefined): LinkTarget {
  const trimmed = href.trim();
  if (trimmed.startsWith("#")) return { kind: "anchor", id: pathPart(trimmed.slice(1)) };
  const scheme = SCHEME.exec(trimmed)?.[0].toLowerCase();
  if (scheme !== undefined) {
    if (!EXTERNAL_SCHEMES.has(scheme)) return { kind: "blocked" };
    try {
      return { kind: "external", url: new URL(trimmed).href };
    } catch {
      return { kind: "blocked" };
    }
  }
  const path = root === undefined ? undefined : resolveLocal(trimmed, from, root);
  return path === undefined ? { kind: "blocked" } : { kind: "file", path };
}
