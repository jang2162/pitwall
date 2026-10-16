// 액션 찾기의 검색 비교. 한글은 자모로 풀어 비교한다.
//
// 한글을 치는 동안 입력 칸에는 조합 중인 글자가 들어 있다 — 「설정」을 치면 ㅅ → 서 → 설 → 설ㅈ → 설저 → 설정 이다.
// 글자 단위로 비교하면 「서」 · 「설ㅈ」 은 「설정」 에 들어 있지 않아 목록이 조합 도중에 비었다가 돌아온다. 자모로 풀면
// (설 → ㅅㅓㄹ, 설정 → ㅅㅓㄹㅈㅓㅇ) 조합 중인 글자도 앞부분으로 맞는다. 받침이 다음 글자의 초성으로 넘어가는 경우
// (가 → 간 → 가나)도 자모열에서는 같은 앞부분이다. 겹모음 · 겹받침도 낱자로 푼다(화 → ㅎㅗㅏ, 값 → ㄱㅏㅂㅅ).

const CHO = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
const JUNG = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ";
const JONG = ["", ..."ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ"];

const SPLIT: Record<string, string> = {
  ㄳ: "ㄱㅅ",
  ㄵ: "ㄴㅈ",
  ㄶ: "ㄴㅎ",
  ㄺ: "ㄹㄱ",
  ㄻ: "ㄹㅁ",
  ㄼ: "ㄹㅂ",
  ㄽ: "ㄹㅅ",
  ㄾ: "ㄹㅌ",
  ㄿ: "ㄹㅍ",
  ㅀ: "ㄹㅎ",
  ㅄ: "ㅂㅅ",
  ㅘ: "ㅗㅏ",
  ㅙ: "ㅗㅐ",
  ㅚ: "ㅗㅣ",
  ㅝ: "ㅜㅓ",
  ㅞ: "ㅜㅔ",
  ㅟ: "ㅜㅣ",
  ㅢ: "ㅡㅣ",
};

const BASE = 0xac00;
const LAST = 0xd7a3;

/** 비교용 문자열: 소문자, 한글 음절과 겹자모는 낱자모로. */
export function searchKey(text: string): string {
  let out = "";
  for (const ch of text.normalize("NFC").toLowerCase()) {
    const code = ch.codePointAt(0)!;
    if (code >= BASE && code <= LAST) {
      const index = code - BASE;
      const cho = CHO[Math.floor(index / 588)]!;
      const jung = JUNG[Math.floor((index % 588) / 28)]!;
      const jong = JONG[index % 28]!;
      out += cho + (SPLIT[jung] ?? jung) + (SPLIT[jong] ?? jong);
    } else {
      out += SPLIT[ch] ?? ch;
    }
  }
  return out;
}

/**
 * 검색어가 맞는 정도. 0 이면 안 맞는다. 앞부분이 맞으면 1, 낱말의 시작이 맞으면 0.8, 중간이 맞으면 0.5.
 * cmdk 의 `filter` 에 그대로 쓴다.
 */
export function matchScore(text: string, query: string): number {
  const q = searchKey(query.trim());
  if (!q) return 1;
  const t = searchKey(text);
  const at = t.indexOf(q);
  if (at < 0) return 0;
  if (at === 0) return 1;
  return /[\s._\-/]/.test(t[at - 1]!) ? 0.8 : 0.5;
}
