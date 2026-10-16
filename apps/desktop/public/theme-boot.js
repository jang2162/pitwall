// JS 번들보다 먼저, 저장된 테마 선택으로 문서 루트의 color-scheme 을 정한다. 번들이 토큰을 주입하기 전 웹뷰가 칠하는
// 기본 바탕(라이트면 흰색)이 다크 선택에서 한 번 비치지 않게 한다. 토큰 주입은 번들(src/theme/adapters.ts)이 한다.
// 인라인 스크립트가 아닌 파일인 것은 CSP(default-src 'self', tauri.conf.json)가 인라인 스크립트를 막기 때문이다.
// 저장 키와 값은 src/theme/adapters.ts 의 THEME_STORAGE_KEY · theme-store.ts 의 ThemePreference 를 따른다
// (src/theme/theme-boot.test.ts 가 맞는지 본다).
// 테마의 변형은 보지 않는다 — 지금 칠할 수 있는 테마는 두 변형이 있는 기본 테마뿐이다. 테마를 고르는 설정이 생길 때 할 일은
// docs/design/07-extensibility.md 「3. 테마」 「기본 테마」 행.
(function () {
  var preference = null;
  try {
    preference = window.localStorage.getItem("pitwall.theme.preference");
  } catch {
    // 저장소를 못 읽으면 OS 를 따른다.
  }
  var appearance =
    preference === "light" || preference === "dark"
      ? preference
      : window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
  document.documentElement.dataset.theme = appearance;
  document.documentElement.style.colorScheme = appearance;
})();
