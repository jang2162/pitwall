// 시험용 디스크(15-m4-plan M4-12): 적용기(workspace-edit.ts)의 `EditFs` 와 문서 모델의 `DocumentSource` 가 같은 파일을 본다. `fs.*` 의
// 규칙을 흉내 낸다 — 쓰기는 기대 `version` 이 다르면 `CONFLICT`(기대가 없으면 「없을 때만」), 만들기 · 이름 바꾸기는 덮지 않고, 루트
// 밖은 `PERMISSION_DENIED`, 없는 것은 `NOT_FOUND`. 바이트는 BOM 표지 + 텍스트(줄바꿈 그대로)다.

import {
  ERROR_CODE,
  type FsContent,
  type FsRead,
  type FsStat,
  type Judgment,
  type LineEnding,
  RpcCallError,
} from "@pitwall/rpc-client";

import type { DocumentSource } from "../editor/documents";
import type { EditFs } from "./workspace-edit";

interface File {
  text: string;
  bom: boolean;
  version: number;
}

function lineEndingOf(text: string): LineEnding {
  const crlf = (text.match(/\r\n/g) ?? []).length;
  const lf = (text.match(/\n/g) ?? []).length - crlf;
  const cr = (text.match(/\r(?!\n)/g) ?? []).length;
  const kinds = [crlf > 0, lf > 0, cr > 0].filter(Boolean).length;
  if (kinds > 1) return "mixed";
  if (crlf > 0) return "crlf";
  if (cr > 0) return "cr";
  return "lf";
}

const fail = (code: number, message: string) => new RpcCallError({ code, message });

export function fakeDisk(
  root: string,
  files: Record<string, string | { text: string; bom: boolean }>,
) {
  const disk = new Map<string, File>();
  const dirs = new Set<string>([root]);
  let seq = 0;
  const put = (path: string, text: string, bom: boolean) => {
    disk.set(path, { text, bom, version: ++seq });
    for (
      let d = path.replace(/\/[^/]*$/, "");
      d.length >= root.length;
      d = d.replace(/\/[^/]*$/, "")
    )
      dirs.add(d);
  };
  for (const [path, f] of Object.entries(files)) {
    if (typeof f === "string") put(path, f, false);
    else put(path, f.text, f.bom);
  }
  const inside = (path: string) => {
    if (path !== root && !path.startsWith(`${root}/`))
      throw fail(ERROR_CODE.PERMISSION_DENIED, `${path} is outside the project`);
  };
  /** 쓰기 직전에 부른다(경합 시험 — 디스크를 바꿔 둔다). */
  let beforeWrite: ((path: string) => void) | undefined;
  const calls: { method: string; path: string }[] = [];

  const judgment = (f: File): Judgment => ({
    binary: false,
    encoding: "UTF-8",
    lossy: false,
    bom: f.bom ? "utf8" : null,
    lineEnding: lineEndingOf(f.text),
    longLines: false,
    sizeClass: "editable",
    editable: true,
    highlight: true,
  });
  const stat = async (path: string): Promise<FsStat> => {
    inside(path);
    const f = disk.get(path);
    if (!f && !dirs.has(path)) throw fail(ERROR_CODE.NOT_FOUND, `${path} not found`);
    return {
      path,
      kind: f ? "file" : "dir",
      target: null,
      size: f?.text.length ?? 0,
      mtimeMs: f?.version ?? 0,
      readonly: false,
      sizeClass: f ? "editable" : null,
    };
  };
  const read = async (path: string): Promise<FsRead> => {
    inside(path);
    const f = disk.get(path);
    if (!f) throw fail(ERROR_CODE.NOT_FOUND, `${path} not found`);
    return {
      path,
      size: f.text.length,
      mtimeMs: f.version,
      offset: 0,
      eof: true,
      version: `v${f.version}`,
      content: { kind: "text", text: f.text },
      judgment: judgment(f),
    };
  };
  const write = async (
    path: string,
    content: FsContent,
    bom: boolean,
    expected: string | null | undefined,
  ) => {
    inside(path);
    calls.push({ method: "write", path });
    beforeWrite?.(path);
    const f = disk.get(path);
    if (expected == null ? f !== undefined : !f || `v${f.version}` !== expected)
      throw fail(ERROR_CODE.CONFLICT, `${path} changed`);
    if (content.kind !== "text") throw new Error("text only");
    put(path, content.text, bom);
    const now = disk.get(path)!;
    return { path, size: now.text.length, mtimeMs: now.version, version: `v${now.version}` };
  };
  const fs: EditFs = {
    stat,
    read,
    write,
    async create(path) {
      inside(path);
      calls.push({ method: "create", path });
      if (disk.has(path) || dirs.has(path)) throw fail(ERROR_CODE.CONFLICT, `${path} exists`);
      put(path, "", false);
      return { path };
    },
    async rename(from, to) {
      inside(from);
      inside(to);
      calls.push({ method: "rename", path: to });
      const f = disk.get(from);
      if (!f) throw fail(ERROR_CODE.NOT_FOUND, `${from} not found`);
      if (disk.has(to)) throw fail(ERROR_CODE.CONFLICT, `${to} exists`);
      disk.delete(from);
      disk.set(to, f);
      return { from, to };
    },
    async trash(path) {
      inside(path);
      calls.push({ method: "trash", path });
      disk.delete(path);
      return { removed: [path], failed: [] };
    },
  };
  const source: DocumentSource = {
    stat: (path) => stat(path),
    read: ({ path }) => read(path),
    write: ({ path, content, bom, expectedVersion }) =>
      write(path, content, bom ?? false, expectedVersion),
  };
  return {
    fs,
    source,
    calls,
    /** 파일의 바이트(UTF-8, BOM 포함). 없으면 `undefined`. */
    bytes(path: string): Uint8Array | undefined {
      const f = disk.get(path);
      return f && new TextEncoder().encode((f.bom ? "﻿" : "") + f.text);
    },
    text: (path: string) => disk.get(path)?.text,
    has: (path: string) => disk.has(path),
    /** 바깥(다른 프로그램)에서 고친다. */
    edit(path: string, text: string) {
      const f = disk.get(path)!;
      put(path, text, f.bom);
    },
    set beforeWrite(hook: ((path: string) => void) | undefined) {
      beforeWrite = hook;
    },
  };
}
