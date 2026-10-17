import { describe, expect, it } from "vitest";

import { describeOpen, parseProjectQuery } from "./project-open";

describe("parseProjectQuery", () => {
  it("reads what Rust puts in a new project window's query", () => {
    // src-tauri/src/windows.rs 의 query_values_are_percent_encoded 와 같은 인코딩.
    expect(
      parseProjectQuery(
        "?window=project&project=ab12&worktree=%2Fwt&file=%2Fa%20b%2Fc%2Bd%26e%3Df%2F%ED%95%9C.rs&line=3&col=4",
      ),
    ).toEqual({
      projectId: "ab12",
      open: { worktree: "/wt", file: "/a b/c+d&e=f/한.rs", line: 3, col: 4 },
    });
  });

  it("leaves out what is missing or not a positive integer", () => {
    expect(parseProjectQuery("?window=project&project=ab12&line=0&col=x")).toEqual({
      projectId: "ab12",
      open: {},
    });
    expect(parseProjectQuery("?window=project")).toEqual({ projectId: undefined, open: {} });
  });
});

describe("describeOpen", () => {
  it("names the worktree and the file location", () => {
    expect(describeOpen({ worktree: "/wt", file: "/wt/a.rs", line: 3, col: 4 })).toBe(
      "worktree /wt, file /wt/a.rs:3:4",
    );
    expect(describeOpen({ file: "/a.rs", line: 7 })).toBe("file /a.rs:7");
    expect(describeOpen({})).toBeUndefined();
  });
});
