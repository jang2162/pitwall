import { describe, expect, it } from "vitest";

import { dropHits, dropText, quoteForShell, shellQuoting } from "./terminal-drop";

// 셸이 붙여 넣은 글자를 낱말로 읽는 규칙(문서대로)의 작은 해석기. 따옴표 밖에서 메타문자를 만나면 그 낱말이 끝나거나 무언가가
// 펼쳐지므로 던진다 — 돌려준 값이 원래 경로와 같아야 「한 낱말, 글자 그대로」 다.
const UNQUOTED = /[A-Za-z0-9_\-./,:@%+=\u0080-\uffff]/;

function posixWord(text: string): string {
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (c === "'") {
      const end = text.indexOf("'", i + 1);
      if (end < 0) throw new Error(`unterminated quote in ${text}`);
      out += text.slice(i + 1, end); // 작은따옴표 안은 이스케이프가 없다.
      i = end;
    } else if (c === "\\") {
      out += text[++i] ?? "";
    } else if (UNQUOTED.test(c) && !(i === 0 && c === "=")) {
      out += c;
    } else {
      throw new Error(`unquoted ${JSON.stringify(c)} in ${text}`);
    }
  }
  return out;
}

function fishWord(text: string): string {
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (c === "'") {
      // 작은따옴표 안에서 `\\` → `\`, `\'` → `'`, 그 밖의 `\` 는 글자 그대로.
      for (i++; ; i++) {
        if (i >= text.length) throw new Error(`unterminated quote in ${text}`);
        const q = text[i]!;
        if (q === "'") break;
        if (q === "\\" && (text[i + 1] === "\\" || text[i + 1] === "'")) out += text[++i];
        else out += q;
      }
    } else if (c === "\\") {
      out += text[++i] ?? "";
    } else if (UNQUOTED.test(c) && c !== "%") {
      out += c;
    } else {
      throw new Error(`unquoted ${JSON.stringify(c)} in ${text}`);
    }
  }
  return out;
}

const PWSH_QUOTES = "'\u2018\u2019\u201A\u201B";

function pwshWord(text: string): string {
  if (!PWSH_QUOTES.includes(text[0]!)) {
    if (!/^[A-Za-z0-9_\-.\\/:]+$/.test(text)) throw new Error(`unquoted word ${text}`);
    return text;
  }
  let out = "";
  for (let i = 1; ; i++) {
    if (i >= text.length) throw new Error(`unterminated quote in ${text}`);
    const q = text[i]!;
    if (PWSH_QUOTES.includes(q)) {
      // 따옴표 글자 둘이 이어지면 글자 하나, 아니면 문자열의 끝.
      if (i + 1 < text.length && PWSH_QUOTES.includes(text[i + 1]!)) {
        out += q;
        i++;
        continue;
      }
      if (i !== text.length - 1) throw new Error(`text after the quote in ${text}`);
      return out;
    }
    out += q;
  }
}

const TRICKY = [
  "/a/b.txt",
  "/a/my file.txt",
  "/a/it's",
  "/a/'",
  "/a/''",
  `/a/"dq"`,
  "/a/$HOME;rm -rf ~",
  "/a/$(touch pwned)",
  "/a/`touch pwned`",
  String.raw`/a/back\slash`,
  String.raw`/a/end\\`,
  String.raw`/a/x\';echo pwned;#`,
  String.raw`/a/x\\';echo pwned;#`,
  "/a/*?[glob]{a,b}",
  "/a/!hist ^old^new",
  "/a/~tilde #hash",
  "/a/&|<>();",
  "/a/%job",
  "=cmd",
  "/a/=eq",
  "/문서/가.txt",
  "/a/\u2018smart\u2019 \u201Aquotes\u201B",
];

const CONTROL = [
  "/a/new\nline",
  "/a/cr\r",
  "/a/tab\t",
  "/a/esc\u001b[201~; echo pwned",
  "/a/del\u007f",
  "/a/c1\u009b",
  "/a/nul\u0000",
];

describe("dropped paths", () => {
  it("pick the quoting from the shell the terminal runs", () => {
    expect(shellQuoting("/bin/zsh")).toBe("posix");
    expect(shellQuoting("-bash")).toBe("posix");
    expect(shellQuoting("/bin/sh")).toBe("posix");
    expect(shellQuoting("/opt/homebrew/bin/fish")).toBe("fish");
    expect(shellQuoting(String.raw`C:\Program Files\PowerShell\7\pwsh.exe`)).toBe("powershell");
    expect(shellQuoting("powershell.exe")).toBe("powershell");
    expect(shellQuoting("/usr/bin/nu")).toBe("unknown");
    expect(shellQuoting(String.raw`C:\Windows\System32\cmd.exe`)).toBe("unknown");
    expect(shellQuoting(undefined)).toBe("unknown");
  });

  it("quote every tricky name so a POSIX shell reads back one word, as is", () => {
    for (const path of TRICKY) expect(posixWord(quoteForShell(path, "posix")!), path).toBe(path);
    expect(quoteForShell("/a/b.txt", "posix")).toBe("/a/b.txt");
    expect(quoteForShell("/a/it's", "posix")).toBe(`'/a/it'\\''s'`);
    expect(quoteForShell(String.raw`/a/b\c`, "posix")).toBe(String.raw`'/a/b'\\'c'`);
  });

  it("read the POSIX quoting the same in fish, which can run inside bash or zsh", () => {
    for (const path of TRICKY) expect(fishWord(quoteForShell(path, "posix")!), path).toBe(path);
  });

  it("quote every tricky name so fish reads back one word, as is", () => {
    for (const path of TRICKY) expect(fishWord(quoteForShell(path, "fish")!), path).toBe(path);
    expect(quoteForShell(String.raw`/a/x\';echo pwned;#`, "fish")).toBe(
      String.raw`'/a/x\\\';echo pwned;#'`,
    );
  });

  it("quote every tricky name so PowerShell reads back one word, as is", () => {
    for (const path of TRICKY)
      expect(pwshWord(quoteForShell(path, "powershell")!), path).toBe(path);
    expect(quoteForShell(String.raw`C:\a\b.txt`, "powershell")).toBe(String.raw`C:\a\b.txt`);
    expect(quoteForShell(String.raw`C:\my dir\it's.txt`, "powershell")).toBe(
      String.raw`'C:\my dir\it''s.txt'`,
    );
  });

  it("paste for an unknown shell only names that need no quoting", () => {
    expect(quoteForShell("/a/b-c_d.txt", "unknown")).toBe("/a/b-c_d.txt");
    for (const path of TRICKY.filter((p) => !/^[A-Za-z0-9_\-./]+$/.test(p))) {
      expect(quoteForShell(path, "unknown"), path).toBeUndefined();
    }
  });

  it("paste no name with a control character into any shell", () => {
    for (const quoting of ["posix", "fish", "powershell", "unknown"] as const) {
      for (const path of CONTROL) expect(quoteForShell(path, quoting), path).toBeUndefined();
      expect(dropText(["/a", CONTROL[0]!], quoting)).toBeUndefined();
    }
  });

  it("join with spaces and end with one", () => {
    expect(dropText(["/a", "/b c"], "posix")).toBe("/a '/b c' ");
    expect(dropText([], "posix")).toBe("");
  });

  it("hit-test physical drop positions against CSS rectangles", () => {
    const rect = { left: 10, top: 100, right: 210, bottom: 300 };
    expect(dropHits(rect, { x: 40, y: 400 }, 2)).toBe(true);
    expect(dropHits(rect, { x: 10, y: 400 }, 2)).toBe(false);
    expect(dropHits(rect, { x: 40, y: 99 }, 1)).toBe(false);
  });
});
