// 미리보기 뷰어(이미지 · Markdown, 12-m1-plan M1-22)가 창에서 받는 것: 파일 바이트, 탭의 루트, 외부 브라우저. 창(project-window.tsx)이
// Host 연결과 opener 로 채우고, 시험은 가짜를 넘긴다.
//
// 바이트는 `fs.read` 범위 읽기를 이어 붙인 Blob 이다(결정 M1-D11). Tauri 의 asset 프로토콜은 로컬 디스크만 읽어 원격 Host 에서
// 깨지므로 쓰지 않는다. 범위 읽기는 Host 의 접근 범위(crates/pw-fs/src/scope.rs)를 지나므로 루트 밖을 가리키는 심링크는 거절된다.

import type { FsRead, FsReadParams } from "@pitwall/rpc-client";
import { createContext, type ReactNode, useContext } from "react";

/** 범위 읽기 한 번의 크기. */
export const BYTES_CHUNK = 8 * 1024 * 1024;

/** 미리보기가 한 파일에서 읽는 바이트의 상한(결정 M1-D9 — 텍스트 · 이미지 탭과 같은 50 MiB). */
export const MAX_PREVIEW_BYTES = 50 * 1024 * 1024;

/** 파일이 읽기 상한을 넘는다. `size` 는 Host 가 알려 준 파일 전체 크기다. */
export class PreviewTooLarge extends Error {
  constructor(
    readonly size: number,
    readonly limit: number,
  ) {
    super(`larger than ${limit} bytes (${size})`);
  }
}

export interface PreviewHost {
  /**
   * 파일 전체를 바이트로 읽는다(`fs.read` 범위 읽기). `worktree` 는 접근 범위다. `limit`(기본 `MAX_PREVIEW_BYTES`)을 넘으면
   * 첫 조각의 크기를 보고 더 읽지 않고 `PreviewTooLarge` 로 거절한다.
   */
  readBytes(path: string, worktree: string | undefined, limit?: number): Promise<Blob>;
  /** 탭의 루트(워크트리 안의 프로젝트 루트, 워크트리가 없으면 등록 루트). 모르면 `undefined` — 상대 경로를 풀지 않는다. */
  rootOf(worktree: string | undefined): Promise<string | undefined>;
  /** 외부 브라우저(메일 앱)로 연다. 앱 창은 그대로다. */
  openExternal(url: string): Promise<void>;
}

function decodeBase64(data: string): Uint8Array<ArrayBuffer> {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/**
 * `read`(접근 범위가 정해진 `fs.read`)로 파일 전체를 `BYTES_CHUNK` 씩 읽어 Blob 으로. 조각이 알려 주는 파일 전체 크기(`size`)가
 * `limit` 을 넘으면 첫 조각만 받고 거절한다. 누적이 넘어도 멈춘다.
 */
export async function readAllBytes(
  read: (params: Pick<FsReadParams, "path" | "offset" | "length" | "bytes">) => Promise<FsRead>,
  path: string,
  limit = MAX_PREVIEW_BYTES,
): Promise<Blob> {
  const parts: Uint8Array<ArrayBuffer>[] = [];
  let offset = 0;
  for (;;) {
    const chunk = await read({ path, offset, length: BYTES_CHUNK, bytes: true });
    if (chunk.size > limit) throw new PreviewTooLarge(chunk.size, limit);
    if (chunk.content.kind !== "base64") throw new Error(`expected bytes from fs.read: ${path}`);
    const bytes = decodeBase64(chunk.content.data);
    parts.push(bytes);
    offset += bytes.length;
    if (offset > limit) throw new PreviewTooLarge(Math.max(chunk.size, offset), limit);
    if (chunk.eof || bytes.length === 0) return new Blob(parts);
  }
}

const PreviewHostContext = createContext<PreviewHost | undefined>(undefined);

export function PreviewHostProvider({
  host,
  children,
}: {
  host: PreviewHost;
  children: ReactNode;
}) {
  return <PreviewHostContext.Provider value={host}>{children}</PreviewHostContext.Provider>;
}

export function usePreviewHost(): PreviewHost {
  const host = useContext(PreviewHostContext);
  if (!host) throw new Error("PreviewHostProvider 밖에서 미리보기를 썼다");
  return host;
}

const IMAGE_MIME: Readonly<Record<string, string>> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
};

/**
 * 이미지 파일의 MIME(확장자로). 이미지가 아니면 `undefined`. Blob 에 형식이 없으면 웹뷰가 svg 를 그리지 않는다 — svg 도 `<img>` 로만
 * 그리므로(결정 M1-D11) 그 안의 스크립트는 돌지 않는다.
 */
export function imageMime(path: string): string | undefined {
  const name = path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? IMAGE_MIME[name.slice(dot + 1).toLowerCase()] : undefined;
}
