// 이미지 뷰어(12-m1-plan M1-22, 결정 M1-D11): png · jpg · gif · webp · bmp · ico · svg. 바이트를 `fs.read` 범위 읽기로 받아 Blob URL 로
// `<img>` 에 넘긴다(preview-host.tsx). svg 도 `<img>` 로만 그린다 — 웹뷰가 `<img>` 안의 svg 는 스크립트를 돌리지 않는다. 문서에
// 끼워 넣지(인라인 svg · `<object>` · `<iframe>`) 않는다.
//
// 위의 막대: [맞춤] [실제 크기], 해상도(그린 뒤 `naturalWidth` × `naturalHeight`) · 파일 크기. 맞춤은 창보다 큰 이미지만 줄이고
// 작은 이미지는 키우지 않는다. 50 MiB 를 넘는 파일은 열지 않는다(결정 M1-D9 — 텍스트와 같은 기준).

import { useEffect, useState } from "react";

import { useProjectStrings } from "../layout/strings";
import { EditorNotice } from "./notice";
import { imageMime, usePreviewHost } from "./preview-host";
import "./preview.css";
import type { ViewerProps } from "./viewers";

const KIB = 1024;

/** 파일 크기(B · KiB · MiB, 소수 한 자리 — viewers.tsx 의 `formatMiB` 와 같은 꼴). */
export function formatBytes(bytes: number): string {
  if (bytes < KIB) return `${bytes} B`;
  if (bytes < KIB * KIB) return `${(bytes / KIB).toFixed(1)} KiB`;
  return `${(bytes / KIB / KIB).toFixed(1)} MiB`;
}

type Fit = "fit" | "actual";

type Loaded =
  | { kind: "loading" }
  | { kind: "ready"; url: string }
  | { kind: "failed"; message: string };

export function ToolbarToggle({
  pressed,
  label,
  onPress,
  name,
}: {
  pressed: boolean;
  label: string;
  onPress(): void;
  name: string;
}) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      data-mode={name}
      onClick={onPress}
      className={`rounded-sm px-2 py-px ${pressed ? "bg-ui-active text-ui-foreground" : "text-ui-muted-foreground hover:bg-ui-hover"}`}
    >
      {label}
    </button>
  );
}

export function ImageViewer({ params, doc }: ViewerProps) {
  const T = useProjectStrings().editor;
  const host = usePreviewHost();
  const path = doc.status === "external" ? doc.stat.path : params.path;
  const size = doc.status === "external" ? doc.stat.size : doc.size;
  const tooLarge = doc.status === "external" && doc.stat.sizeClass === "tooLarge";
  const { worktree } = params;
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  const [natural, setNatural] = useState<{ width: number; height: number }>();
  const [fit, setFit] = useState<Fit>("fit");

  useEffect(() => {
    if (tooLarge) return;
    let url: string | undefined;
    let current = true;
    setLoaded({ kind: "loading" });
    host.readBytes(path, worktree).then(
      (blob) => {
        if (!current) return;
        url = URL.createObjectURL(new Blob([blob], { type: imageMime(path) ?? "" }));
        setLoaded({ kind: "ready", url });
      },
      (e: unknown) => current && setLoaded({ kind: "failed", message: errorText(e) }),
    );
    return () => {
      current = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [host, path, worktree, tooLarge]);

  if (tooLarge) return <EditorNotice text={T.tooLarge(formatBytes(size))} />;
  if (loaded.kind === "failed") return <EditorNotice text={T.image.failed(loaded.message)} />;
  return (
    <div className="flex h-full min-h-0 flex-col bg-editor-background" data-testid="image-viewer">
      <div className="flex h-[26px] flex-none items-center gap-1 border-b border-ui-border bg-ui-panel px-2 text-[12px]">
        <div role="group" aria-label={T.image.zoom} className="flex gap-0.5">
          <ToolbarToggle
            name="fit"
            pressed={fit === "fit"}
            label={T.image.fit}
            onPress={() => setFit("fit")}
          />
          <ToolbarToggle
            name="actual"
            pressed={fit === "actual"}
            label={T.image.actual}
            onPress={() => setFit("actual")}
          />
        </div>
        <span data-testid="image-info" className="ml-auto text-ui-muted-foreground">
          {natural ? `${T.image.dimensions(natural.width, natural.height)} · ` : ""}
          {formatBytes(size)}
        </span>
      </div>
      <div
        className={`pw-checkerboard min-h-0 flex-1 overflow-auto ${fit === "fit" ? "flex items-center justify-center p-2" : "p-2"}`}
      >
        {loaded.kind === "ready" && (
          <img
            src={loaded.url}
            alt={path}
            draggable={false}
            onLoad={(e) =>
              setNatural({
                width: e.currentTarget.naturalWidth,
                height: e.currentTarget.naturalHeight,
              })
            }
            className={fit === "fit" ? "max-h-full max-w-full object-contain" : "max-w-none"}
          />
        )}
      </div>
    </div>
  );
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
