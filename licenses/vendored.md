<!--
저장소에 복사해 넣은 서드파티 코드·데이터의 고지를 여기에 손으로 쓴다(의존성으로 들어오는 것은 생성기가 채운다).
첫 `## ` 절부터 끝까지가 THIRD_PARTY_NOTICES.md 끝에 그대로 실린다. 고친 뒤 `mise run notices` 로 다시 만든다.
항목마다 원본 이름·출처 URL·라이선스·원본 라이선스 전문·변경 사실을 적는다(docs/design/06-roadmap.md 「서드파티 코드 차용」 행).
-->

## Vendored code

### GitHub Linguist v9.7.0 (data files)

- Source: https://github.com/github-linguist/linguist, commit `e0c78d62c42abae6122235d8e68a7aa43eef89da` (tag `v9.7.0`)
- Files: `lib/linguist/languages.yml`, `heuristics.yml`, `vendor.yml`, `documentation.yml`, `generic.yml`, `popular.yml`, copied unmodified into `crates/pw-lang/linguist/`
- Derived code: `crates/pw-lang/src/linguist.rs` (detection strategies ported from `lib/linguist.rb`, `lib/linguist/strategy/*.rb`, `lib/linguist/shebang.rb`, `lib/linguist/heuristics.rb`) and `crates/pw-lang/src/generated.rs` (a subset of the rules in `lib/linguist/generated.rb`), rewritten in Rust
- License: MIT

```text
Copyright (c) 2017 GitHub, Inc.

Permission is hereby granted, free of charge, to any person
obtaining a copy of this software and associated documentation
files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES
OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.
```
