// 다른 머신 문서의 조각 읽기 · 쓰기(18-m7-plan M7-13b). 다른 머신의 RPC 는 링크 틀 하나(64 MiB)에 실리고 틀을 넘는 메시지는 그
// 머신의 채널을 닫는다(editor/link-limits.ts). 통째 `fs.read` · `fs.write` 대신 Host 의 `fs.readChunk` · `fs.writeChunk` →
// `fs.writeCommit` 을 부른다(crates/pw-fs/src/chunks.rs) — 조각은 파일 바이트가 아니라 **와이어 바이트**(직렬화한 JSON 문자열)로
// 잘라 제어 문자(`\u00XX` 6배) · 따옴표(2배)가 많은 글도 한 조각이 `CHUNK_WIRE_MAX`(16 MiB) 안이다.

import { ERROR_CODE } from "@pitwall/rpc-client";
import type { FsContent, FsRead, FsReadChunk, FsWriteChunk, FsWritten } from "@pitwall/rpc-client";

import { errorCode } from "./doc-sync";
import { CHUNK_WIRE_MAX } from "./limits";

/** 조각 사이에 파일이 바뀌면(`CONFLICT`) 처음부터 다시 읽는 횟수 — 통째 읽기의 다시 읽기(crates/pw-fs/src/read.rs)와 같다. */
const READ_ATTEMPTS = 5;

/** 조각 읽기 한 번(`fs.readChunk`). 경로 · 접근 범위는 부르는 쪽이 채운다. */
export type ReadChunk = (params: {
  offset: number;
  maxBytes: number;
  expectedVersion: string | null;
}) => Promise<FsReadChunk>;

/**
 * 파일 전체를 조각으로 읽어 통째 `fs.read` 의 결과 꼴로 모은다. 모든 조각이 같은 `version` 이어야 하고(Host 가 둘째 조각부터
 * 첫 조각의 것과 견준다), 사이에 파일이 바뀌면 처음부터 다시 읽는다 — 서로 다른 순간의 조각을 이은 내용에 `version` 을 붙이지 않는다.
 */
export async function readInChunks(readChunk: ReadChunk): Promise<FsRead> {
  for (let attempt = 1; ; attempt++) {
    try {
      return await readOnce(readChunk);
    } catch (e) {
      if (errorCode(e) !== ERROR_CODE.CONFLICT || attempt >= READ_ATTEMPTS) throw e;
    }
  }
}

async function readOnce(readChunk: ReadChunk): Promise<FsRead> {
  const pieces: string[] = [];
  let offset = 0;
  let expectedVersion: string | null = null;
  for (;;) {
    const chunk = await readChunk({ offset, maxBytes: CHUNK_WIRE_MAX, expectedVersion });
    if (expectedVersion !== null && chunk.version !== expectedVersion)
      throw Object.assign(new Error("the file changed between chunks"), {
        code: ERROR_CODE.CONFLICT,
      });
    expectedVersion = chunk.version;
    pieces.push(chunk.content.kind === "text" ? chunk.content.text : chunk.content.data);
    if (chunk.eof) {
      const joined = pieces.join("");
      return {
        path: chunk.path,
        size: chunk.size,
        mtimeMs: chunk.mtimeMs,
        offset: 0,
        eof: true,
        version: chunk.version,
        content:
          chunk.content.kind === "text"
            ? { kind: "text", text: joined }
            : { kind: "base64", data: joined },
        judgment: chunk.judgment,
      };
    }
    if (chunk.next <= offset) throw new Error("the host did not advance the chunked read");
    offset = chunk.next;
  }
}

/**
 * UTF-16 한 단위(대리 쌍은 둘째 단위와 함께)를 `JSON.stringify` 한 UTF-8 바이트. `JSON.stringify` 는 `"` · `\` · 제어 문자를
 * 이스케이프하고(U+0000 ~ U+001F 는 `\b` 류 2바이트 아니면 `\u00XX` 6바이트) 짝 없는 대리 단위도 `\uDXXX` 로 쓴다.
 */
function unitCost(text: string, i: number): [cost: number, units: number] {
  const c = text.charCodeAt(i);
  if (c === 0x22 || c === 0x5c) return [2, 1];
  if (c < 0x20) return [c === 8 || c === 9 || c === 10 || c === 12 || c === 13 ? 2 : 6, 1];
  if (c < 0x80) return [1, 1];
  if (c < 0x800) return [2, 1];
  if (c >= 0xd800 && c <= 0xdbff) {
    const d = text.charCodeAt(i + 1);
    return d >= 0xdc00 && d <= 0xdfff ? [4, 2] : [6, 1];
  }
  if (c >= 0xdc00 && c <= 0xdfff) return [6, 1];
  return [3, 1];
}

/** 텍스트를 조각마다 직렬화한 바이트(따옴표 포함)가 `budget` 안이게 자른다. 대리 쌍을 가르지 않는다. */
export function splitTextByWire(text: string, budget: number = CHUNK_WIRE_MAX): string[] {
  const pieces: string[] = [];
  let start = 0;
  let used = 2;
  let i = 0;
  while (i < text.length) {
    const [cost, units] = unitCost(text, i);
    if (used + cost > budget) {
      if (i === start) throw new Error("the wire budget is too small for one character");
      pieces.push(text.slice(start, i));
      start = i;
      used = 2;
      continue;
    }
    used += cost;
    i += units;
  }
  if (start < text.length || pieces.length === 0) pieces.push(text.slice(start));
  return pieces;
}

/** base64 를 4글자 묶음(3바이트) 단위로 잘라 조각마다 따로 풀린다. */
function splitBase64(data: string, budget: number = CHUNK_WIRE_MAX): string[] {
  const size = Math.max(4, Math.floor((budget - 2) / 4) * 4);
  const pieces: string[] = [];
  for (let i = 0; i < data.length; i += size) pieces.push(data.slice(i, i + size));
  return pieces.length === 0 ? [""] : pieces;
}

/** 조각 쓰기의 세 RPC. 경로 · 접근 범위는 부르는 쪽이 채운다. */
export interface WriteChunks {
  chunk(params: { upload: string | null; content: FsContent }): Promise<FsWriteChunk>;
  commit(params: {
    upload: string;
    bom: boolean;
    expectedVersion: string | null;
  }): Promise<FsWritten>;
  abort(upload: string): Promise<void>;
}

/**
 * 내용을 조각으로 보내 Host 에 모으고 커밋에서 한 번에 쓴다(`expectedVersion` 충돌이면 아무것도 쓰지 않고 `CONFLICT` — 통째
 * `fs.write` 와 같다). 조각을 보내다 실패하면 모음을 버린다(파일은 그대로다). 커밋은 성공 · 실패와 상관없이 Host 가 모음을 닫는다.
 */
export async function writeInChunks(
  rpc: WriteChunks,
  content: FsContent,
  bom: boolean,
  expectedVersion: string | null,
  budget: number = CHUNK_WIRE_MAX,
): Promise<FsWritten> {
  const pieces =
    content.kind === "text"
      ? splitTextByWire(content.text, budget).map((text): FsContent => ({ kind: "text", text }))
      : splitBase64(content.data, budget).map((data): FsContent => ({ kind: "base64", data }));
  let upload: string | null = null;
  try {
    for (const piece of pieces) upload = (await rpc.chunk({ upload, content: piece })).upload;
  } catch (e) {
    if (upload !== null) await rpc.abort(upload).catch(() => {});
    throw e;
  }
  return rpc.commit({ upload: upload!, bom, expectedVersion });
}
