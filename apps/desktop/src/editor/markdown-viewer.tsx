// Markdown 뷰어(12-m1-plan M1-22, 결정 M1-D11): [에디터] [미리보기] [나란히]. 에디터 쪽은 텍스트 뷰어(text-viewer.tsx) 그대로라
// 편집 · 저장 · 디스크 배너가 같다. 미리보기는 문서(`LiveText`)의 텍스트를 markdown.ts 로 바꿔 그리고, 문서가 바뀌면
// (`DocumentStore.onChange` — 편집 · undo · 디스크에서 받은 내용) `PREVIEW_DEBOUNCE_MS` 뒤에 다시 그린다.
//
// 모드는 탭마다 기억한다(패널 id). 그룹 사이로 옮겨 내용이 다시 마운트돼도 남고, 처음 열면 미리보기다(Viewer-first).
//
// 미리보기 안:
// - 이미지: 상대 경로만, 루트 안일 때만 이미지 뷰어와 같은 길(`fs.read` → Blob URL)로 그린다(규칙 A, markdown.ts). 그리지 않는
//   이미지는 대체 글자로 바꾼다. 원격 이미지도 그리지 않는다 — 웹뷰가 주소를 직접 읽지 않는다.
// - 링크: 누름을 모두 가로챈다(앱 창은 이동하지 않는다). http(s) · mailto 는 외부 브라우저, 상대 경로는 앱의 탭, `#제목` 은 그
//   제목으로 스크롤, 나머지는 아무것도 하지 않는다(markdown.ts `classifyLink`).

import { type MouseEvent, useEffect, useRef, useState } from "react";

import { useProjectStrings } from "../layout/strings";
import { useEditorWorkspace } from "./editor-workspace";
import { ToolbarToggle } from "./image-viewer";
import { classifyLink, IMAGE_SOURCE_ATTR, renderMarkdown, resolveLocal } from "./markdown";
import { imageMime, usePreviewHost } from "./preview-host";
import { TextViewer } from "./text-viewer";
import "./preview.css";
import type { ViewerProps } from "./viewers";

/** 편집이 멈춘 뒤 미리보기를 다시 그리기까지. */
export const PREVIEW_DEBOUNCE_MS = 150;

export type MarkdownMode = "editor" | "preview" | "split";

/** 탭(패널 id) → 고른 모드. 탭이 닫히면 남은 항목은 쓰이지 않는다(작다). */
const modes = new Map<string, MarkdownMode>();

/** GitHub 식 제목 앵커: 소문자, 글자 · 숫자 · 공백 · `-` 밖은 지우고 공백은 `-`. */
function slug(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/\s/g, "-");
}

function MarkdownPreview({
  path,
  worktree,
  initial,
}: {
  path: string;
  worktree: string | undefined;
  initial: string;
}) {
  const workspace = useEditorWorkspace();
  const host = usePreviewHost();
  const container = useRef<HTMLDivElement>(null);
  const [html, setHtml] = useState(() => renderMarkdown(initial));
  const [root, setRoot] = useState<string>();
  /** 풀어 쓴 경로 → Blob URL(못 읽으면 `undefined`). 뷰어가 사는 동안 같은 이미지를 다시 읽지 않는다. */
  const images = useRef(new Map<string, Promise<string | undefined>>());

  useEffect(() => {
    let current = true;
    host.rootOf(worktree).then(
      (r) => current && setRoot(r),
      (e: unknown) => console.warn("[markdown] the root is unknown", worktree, e),
    );
    return () => {
      current = false;
    };
  }, [host, worktree]);

  useEffect(() => {
    const { documents } = workspace;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = documents.onChange(path, () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        const doc = documents.get(path);
        if (doc?.status === "text") setHtml(renderMarkdown(doc.live.text.toString()));
      }, PREVIEW_DEBOUNCE_MS);
    });
    return () => {
      clearTimeout(timer);
      off();
    };
  }, [workspace, path]);

  useEffect(() => {
    const cache = images.current;
    return () => {
      for (const url of cache.values()) void url.then((u) => u && URL.revokeObjectURL(u));
      cache.clear();
    };
  }, []);

  // 새로 그린 이미지 자리에 Blob URL 을 넣는다. 루트를 아직 모르면 기다린다(그 사이엔 빈 자리).
  useEffect(() => {
    const el = container.current;
    if (!el || root === undefined) return;
    let current = true;
    for (const img of el.querySelectorAll<HTMLImageElement>(`img[${IMAGE_SOURCE_ATTR}]`)) {
      const src = img.getAttribute(IMAGE_SOURCE_ATTR) ?? "";
      const target = resolveLocal(src, path, root);
      const type = target === undefined ? undefined : imageMime(target);
      const blocked = () => {
        const alt = document.createElement("span");
        alt.className = "pw-image-blocked";
        alt.title = src;
        alt.textContent = img.alt || src;
        img.replaceWith(alt);
      };
      if (target === undefined || type === undefined) {
        blocked();
        continue;
      }
      let url = images.current.get(target);
      if (!url) {
        url = host.readBytes(target, worktree).then(
          (blob) => URL.createObjectURL(new Blob([blob], { type })),
          () => undefined,
        );
        images.current.set(target, url);
      }
      void url.then((u) => {
        if (!current || !img.isConnected) return;
        if (u) img.src = u;
        else blocked();
      });
    }
    return () => {
      current = false;
    };
  }, [html, root, path, worktree, host]);

  const follow = (event: MouseEvent) => {
    const anchor = (event.target as Element).closest?.("a[href]");
    if (!anchor) return;
    event.preventDefault();
    if (event.type !== "click") return;
    const target = classifyLink(anchor.getAttribute("href") ?? "", path, root);
    switch (target.kind) {
      case "external":
        host
          .openExternal(target.url)
          .catch((e: unknown) => console.warn("[markdown] could not open the link", e));
        return;
      case "file":
        void workspace.open({ path: target.path, worktree });
        return;
      case "anchor": {
        const id = slug(target.id);
        for (const heading of container.current?.querySelectorAll("h1, h2, h3, h4, h5, h6") ?? [])
          if (slug(heading.textContent ?? "") === id) {
            heading.scrollIntoView({ block: "start" });
            return;
          }
        return;
      }
      case "blocked":
        return;
    }
  };

  return (
    <div
      ref={container}
      data-testid="markdown-preview"
      onClick={follow}
      onAuxClick={follow}
      className="pw-markdown min-h-0 min-w-0 flex-1 overflow-auto bg-editor-background px-6 py-4 text-editor-foreground select-text"
      // markdown.ts 가 raw HTML 을 끈 markdown-it 의 출력이다 — 문서의 HTML 은 글자로 들어 있다.
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

export function MarkdownViewer(props: ViewerProps) {
  const T = useProjectStrings().editor.markdown;
  const { panel, params, doc } = props;
  const [mode, setMode] = useState<MarkdownMode>(() => modes.get(panel.id) ?? "preview");
  const choose = (next: MarkdownMode) => {
    modes.set(panel.id, next);
    setMode(next);
  };
  if (doc.status !== "text") return <TextViewer {...props} />;
  return (
    <div
      className="flex h-full min-h-0 flex-col bg-editor-background"
      data-testid="markdown-viewer"
    >
      <div className="flex h-[26px] flex-none items-center border-b border-ui-border bg-ui-panel px-2 text-[12px]">
        <div role="group" aria-label={T.mode} className="ml-auto flex gap-0.5">
          {(["editor", "split", "preview"] as const).map((m) => (
            <ToolbarToggle
              key={m}
              name={m}
              pressed={mode === m}
              label={T[m]}
              onPress={() => choose(m)}
            />
          ))}
        </div>
      </div>
      <div className="flex min-h-0 flex-1">
        {mode !== "preview" && (
          <div className="h-full min-w-0 flex-1">
            <TextViewer {...props} />
          </div>
        )}
        {mode === "split" && <div className="w-px flex-none bg-ui-border" />}
        {mode !== "editor" && (
          <MarkdownPreview
            path={params.path}
            worktree={params.worktree}
            initial={doc.live.text.toString()}
          />
        )}
      </div>
    </div>
  );
}
