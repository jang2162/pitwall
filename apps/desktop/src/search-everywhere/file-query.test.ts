import { describe, expect, it } from "vitest";

import { MAX_QUERY_CHARS, matchRecent, parseFileQuery } from "./file-query";

describe("parseFileQuery", () => {
  it("takes the line and column off 「name:line[:col]」", () => {
    expect(parseFileQuery("main.rs:12")).toEqual({ text: "main.rs", line: 12 });
    expect(parseFileQuery("src/main.rs:12:5")).toEqual({
      text: "src/main.rs",
      line: 12,
      column: 5,
    });
    expect(parseFileQuery("main.rs:12 ")).toEqual({ text: "main.rs", line: 12 });
  });

  it("leaves anything else as the text", () => {
    expect(parseFileQuery("main.rs")).toEqual({ text: "main.rs" });
    expect(parseFileQuery("a:b")).toEqual({ text: "a:b" });
    expect(parseFileQuery("main.rs:")).toEqual({ text: "main.rs:" });
    // 줄은 1부터다.
    expect(parseFileQuery("main.rs:0")).toEqual({ text: "main.rs:0" });
    expect(parseFileQuery("main.rs:3:0")).toEqual({ text: "main.rs", line: 3 });
  });

  it("cuts the text to what the Host accepts", () => {
    expect(parseFileQuery("a".repeat(300)).text).toHaveLength(MAX_QUERY_CHARS);
    expect(parseFileQuery(`${"a".repeat(300)}:4`)).toEqual({
      text: "a".repeat(MAX_QUERY_CHARS),
      line: 4,
    });
  });
});

describe("matchRecent", () => {
  it("matches the file name first, then the whole path, word by word", () => {
    expect(matchRecent("src/main.rs", "mars")).toEqual([4, 5, 9, 10]);
    expect(matchRecent("src/main.rs", "sr ma")).toEqual([0, 1, 4, 5]);
    expect(matchRecent("src/main.rs", "lib")).toBeUndefined();
    expect(matchRecent("src/main.rs", "")).toEqual([]);
  });

  it("ignores case unless the query has a capital letter", () => {
    expect(matchRecent("src/Main.rs", "main")).toEqual([4, 5, 6, 7]);
    expect(matchRecent("src/main.rs", "Main")).toBeUndefined();
  });
});
