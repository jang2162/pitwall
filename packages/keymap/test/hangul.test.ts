import { describe, expect, it } from "vitest";

import { matchScore, searchKey } from "../src/hangul";

describe("hangul search", () => {
  it("splits syllables, compound vowels and compound finals into jamo", () => {
    expect(searchKey("설정")).toBe("ㅅㅓㄹㅈㅓㅇ");
    expect(searchKey("화")).toBe("ㅎㅗㅏ");
    expect(searchKey("값")).toBe("ㄱㅏㅂㅅ");
    expect(searchKey("Find")).toBe("find");
  });

  // 「설정 열기」를 치는 동안 입력 칸에 차례로 보이는 문자열.
  it.each(["ㅅ", "서", "설", "설ㅈ", "설저", "설정", "설정 ㅇ", "설정 여", "설정 열"])(
    "keeps matching while %j is being composed",
    (typing) => {
      expect(matchScore("설정 열기", typing)).toBeGreaterThan(0);
    },
  );

  it("matches a final consonant that becomes the next initial (가 → 간 → 가나)", () => {
    expect(matchScore("가나다", "간")).toBeGreaterThan(0);
  });

  it("ranks a prefix above a word start above a middle match", () => {
    expect(matchScore("프로젝트 툴 윈도우", "프로")).toBe(1);
    expect(matchScore("툴 윈도우: 프로젝트", "프로")).toBe(0.8);
    expect(matchScore("Find Action", "action")).toBe(0.8);
    expect(matchScore("actionfind", "find")).toBe(0.5);
    expect(matchScore("설정", "터미널")).toBe(0);
  });
});
