import type { FsRead, FsStat, Judgment } from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import { createDocumentStore, type DocumentSource } from "./documents";
import { createDocLanguages, PLAIN_TEXT } from "./language";

const judgment: Judgment = {
  binary: false,
  encoding: "UTF-8",
  lossy: false,
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};

/** 파일마다 내용과 판별 언어. */
function store(files: Record<string, { text: string; language: string | null }>) {
  const source: DocumentSource = {
    stat: async (path): Promise<FsStat> => ({
      path,
      kind: "file",
      target: null,
      size: files[path]!.text.length,
      mtimeMs: 1,
      readonly: false,
      sizeClass: "editable",
    }),
    read: async ({ path }): Promise<FsRead> => ({
      path,
      size: files[path]!.text.length,
      mtimeMs: 1,
      offset: 0,
      eof: true,
      version: "v1",
      content: { kind: "text", text: files[path]!.text },
      judgment,
    }),
    detect: async (path) => files[path]!.language,
  };
  return createDocumentStore(source);
}

const loaded = (documents: ReturnType<typeof store>, path: string) =>
  vi.waitFor(() => expect(documents.get(path)?.status).toBe("text"));

it("uses the detected language until a person picks one, and goes back to it on null", async () => {
  const documents = store({ "/p/a.txt": { text: "fn main() {}", language: "Rust" } });
  const languages = createDocLanguages();
  const unbind = languages.bind(documents);
  const seen: string[] = [];
  languages.onChange("/p/a.txt", (language) => seen.push(language));

  const release = documents.acquire("/p/a.txt");
  await loaded(documents, "/p/a.txt");
  expect(languages.get("/p/a.txt")).toBe("Rust");

  // 사람이 Plain Text 를 고른 것과 고른 것을 지운 것(null)은 다르다.
  languages.setOverride("/p/a.txt", PLAIN_TEXT);
  expect(languages.get("/p/a.txt")).toBe(PLAIN_TEXT);
  // 같은 값은 알리지 않는다.
  languages.setOverride("/p/a.txt", PLAIN_TEXT);
  languages.setOverride("/p/a.txt", null);
  expect(languages.get("/p/a.txt")).toBe("Rust");
  expect(seen).toEqual(["Rust", PLAIN_TEXT, "Rust"]);
  release();
  unbind();
});

it("forgets a picked language when the last tab lets go of the document", async () => {
  const documents = store({ "/p/a.rs": { text: "x", language: "Rust" } });
  const languages = createDocLanguages();
  languages.bind(documents);

  const first = documents.acquire("/p/a.rs");
  const second = documents.acquire("/p/a.rs");
  await loaded(documents, "/p/a.rs");
  languages.setOverride("/p/a.rs", "Go");
  // 다른 탭이 아직 그 문서를 본다 — 고른 값이 남는다.
  first();
  expect(languages.get("/p/a.rs")).toBe("Go");
  second();
  expect(languages.get("/p/a.rs")).toBe(PLAIN_TEXT);

  // 다시 열면 판별값이다.
  const again = documents.acquire("/p/a.rs");
  await loaded(documents, "/p/a.rs");
  expect(languages.get("/p/a.rs")).toBe("Rust");
  again();
});

it("answers plain text for a document the detection could not name, an unknown one, and after unbinding", async () => {
  const documents = store({
    "/p/a.py": { text: "x", language: "Python" },
    "/p/README": { text: "x", language: null },
  });
  const languages = createDocLanguages();
  const unbind = languages.bind(documents);
  const readme = documents.acquire("/p/README");
  await loaded(documents, "/p/README");
  expect(languages.get("/p/README")).toBe(PLAIN_TEXT);
  expect(languages.get("/p/nothing")).toBe(PLAIN_TEXT);
  unbind();
  const release = documents.acquire("/p/a.py");
  await loaded(documents, "/p/a.py");
  expect(languages.get("/p/a.py")).toBe(PLAIN_TEXT);
  release();
  readme();
});
