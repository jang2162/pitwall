// 문서 모델 밖의 읽기 전용 텍스트(12-m1-plan M1-23): 열지 않은 큰 파일의 [앞부분 보기](앞 1 MiB)와 바이너리 · 이미지를 「텍스트로
// 열기」. 이 탭이 직접 `fs.read` 범위 읽기로 앞에서 `limit` 바이트까지 읽고, 그 텍스트로 이 탭만의 `LiveText` 를 만들어 텍스트
// 뷰어(text-viewer.tsx)에 넘긴다 — 줄 번호 · 찾기 · 줄 이동 · 커서 표시는 텍스트 뷰어 그대로다. 문서 모델(documents.ts)에 넣지
// 않으므로 저장 · 미저장 확인 · 디스크 맞추기 대상이 아니고, 편집 · 강조는 늘 끈다(`readOnly`, `highlight: false`).
//
// 텍스트로 오지 않은 내용(바이너리)은 UTF-8 로 풀고 풀 수 없는 바이트는 U+FFFD 로 보인다.

import type { FsRead } from "@pitwall/rpc-client";
import { useEffect, useState } from "react";

import { useProjectStrings } from "../layout/strings";
import type { TextDocumentState } from "./documents";
import { LiveText } from "./live-text";
import { EditorNotice } from "./notice";
import { decodeBase64, usePreviewHost } from "./preview-host";
import { TextViewer } from "./text-viewer";
import type { ViewerProps } from "./viewers";

type Loaded =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "ready"; doc: TextDocumentState };

/** 범위 읽기 하나로 만든 읽기 전용 텍스트 문서. */
export function rawTextDocument(read: FsRead): TextDocumentState {
  const text =
    read.content.kind === "text"
      ? read.content.text
      : new TextDecoder("utf-8").decode(decodeBase64(read.content.data));
  return {
    status: "text",
    kind: "text",
    live: new LiveText({ text, lineEnding: read.judgment.lineEnding, bom: false }),
    judgment: { ...read.judgment, editable: false, highlight: false },
    version: null,
    size: read.size,
    mtimeMs: read.mtimeMs,
    language: null,
    readOnly: true,
    dirty: false,
    disk: { kind: "same" },
    saving: false,
  };
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function RawTextViewer({
  panel,
  params,
  limit,
  notice,
}: Pick<ViewerProps, "panel" | "params"> & {
  /** 앞에서 읽을 바이트 수. */
  limit: number;
  /** 위 막대의 안내(무엇을 보이는지 · 읽기 전용). */
  notice: string;
}) {
  const T = useProjectStrings().editor;
  const host = usePreviewHost();
  const { path, worktree } = params;
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });

  useEffect(() => {
    let current = true;
    setLoaded({ kind: "loading" });
    host.readHead(path, worktree, limit).then(
      (read) => current && setLoaded({ kind: "ready", doc: rawTextDocument(read) }),
      (e: unknown) => current && setLoaded({ kind: "failed", message: errorText(e) }),
    );
    return () => {
      current = false;
    };
  }, [host, path, worktree, limit]);

  if (loaded.kind === "loading") return <EditorNotice text={T.loading} openWith={false} />;
  if (loaded.kind === "failed") return <EditorNotice text={T.failed(loaded.message)} />;
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="raw-text-viewer">
      <div
        role="status"
        className="flex-none border-b border-ui-border bg-ui-panel px-2 py-0.5 text-[12px] text-ui-muted-foreground"
      >
        {notice}
      </div>
      <div className="min-h-0 flex-1">
        <TextViewer panel={panel} params={params} doc={loaded.doc} />
      </div>
    </div>
  );
}
