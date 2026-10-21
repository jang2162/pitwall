import { describe, expect, it } from "vitest";

import { dropHits, dropText, quoteForShell } from "./terminal-drop";

describe("dropped paths", () => {
  it("quote for a POSIX shell only when needed", () => {
    expect(quoteForShell("/a/b.txt", "mac")).toBe("/a/b.txt");
    expect(quoteForShell("/a/my file.txt", "linux")).toBe("'/a/my file.txt'");
    expect(quoteForShell("/a/it's", "mac")).toBe(`'/a/it'\\''s'`);
    expect(quoteForShell("/a/$HOME;rm", "mac")).toBe("'/a/$HOME;rm'");
    expect(quoteForShell("/문서/가.txt", "mac")).toBe("'/문서/가.txt'");
  });

  it("quote for PowerShell on Windows", () => {
    expect(quoteForShell(String.raw`C:\a\b.txt`, "win")).toBe(String.raw`C:\a\b.txt`);
    expect(quoteForShell(String.raw`C:\my dir\it's.txt`, "win")).toBe(
      String.raw`'C:\my dir\it''s.txt'`,
    );
  });

  it("join with spaces and end with one", () => {
    expect(dropText(["/a", "/b c"], "mac")).toBe("/a '/b c' ");
    expect(dropText([], "mac")).toBe("");
  });

  it("hit-test physical drop positions against CSS rectangles", () => {
    const rect = { left: 10, top: 100, right: 210, bottom: 300 };
    expect(dropHits(rect, { x: 40, y: 400 }, 2)).toBe(true);
    expect(dropHits(rect, { x: 10, y: 400 }, 2)).toBe(false);
    expect(dropHits(rect, { x: 40, y: 99 }, 1)).toBe(false);
  });
});
