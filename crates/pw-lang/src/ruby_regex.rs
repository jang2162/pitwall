//! Linguist 의 Ruby(Onigmo) 정규식을 Rust `regex` 문법으로 옮긴다.
//!
//! 빌드 스크립트(`build.rs`)가 `#[path]` 로 같은 파일을 넣어 heuristics · vendor · documentation 패턴을 옮기고, 옮긴 결과를
//! `regex` 로 컴파일해 본다. 옮기지 못한 패턴은 표에서 빠지고 이유가 [`crate::SKIPPED_PATTERNS`] 와 빌드 로그에 남는다.
//!
//! 옮기는 규칙:
//! - Ruby 의 `^` · `$` 는 언제나 줄 단위다 → 앞에 `(?m)` 를 붙인다.
//! - Ruby 의 `m` 플래그는 「`.` 이 줄바꿈과 맞는다」(Rust 의 `s`) → 플래그 묶음 안의 `m` 을 `s` 로 바꾼다.
//! - `\h` · `\H`(16진 숫자) → 문자 클래스, `\Z` → `\n?\z`, `\e` → `\x1B`, `\<` · `\>` → 글자(Rust 에서는 단어 경계다).
//! - 짝이 없는 `{` 는 글자다(Ruby). Rust 는 반복 문법으로 읽어 실패하므로 `\{` 로 바꾼다.
//! - 옮기지 않는 것(Rust `regex` 에 없다): 앞뒤 보기(`(?=` `(?!` `(?<=` `(?<!`), 원자 묶음 `(?>`, 소유 반복(`*+` 등),
//!   역참조(`\1` `\k<..>`), 부분식 호출(`\g<..>`), `\K` `\G` `\R` `\X`, 조건식 `(?(`.
//!
//! 바뀌는 뜻: `\w` · `\d` · `\s` · `\b` 는 Rust 에서 유니코드 기준이다(Ruby 는 ASCII). 영문 소스에서는 결과가 같다.

/// 옮긴 패턴. 단일 패턴이면 `(?m)` 이 붙어 있다.
pub fn translate(ruby: &str) -> Result<String, String> {
    let chars: Vec<char> = ruby.chars().collect();
    let mut out = String::with_capacity(ruby.len() + 8);
    out.push_str("(?m)");
    let mut i = 0;
    // 문자 클래스 깊이(Ruby 는 `[a-z&&[^x]]` 처럼 겹칠 수 있다).
    let mut class = 0usize;
    // 바로 앞이 반복 기호였는가(소유 반복 `*+` 를 알아보려고).
    let mut after_quantifier = false;
    // 바로 앞이 `{n,m}` 이었는가. Ruby 의 `{n}+` 는 소유 반복이 아니라 반복의 반복이고, Rust 는 받지 않는다.
    let mut after_counted = false;
    while i < chars.len() {
        let c = chars[i];
        let was_quantifier = after_quantifier;
        let was_counted = after_counted;
        after_quantifier = false;
        after_counted = false;
        if c == '\\' {
            let Some(&next) = chars.get(i + 1) else {
                return Err("trailing backslash".into());
            };
            i += 2;
            match next {
                'h' if class > 0 => out.push_str("0-9a-fA-F"),
                'h' => out.push_str("[0-9a-fA-F]"),
                'H' if class > 0 => return Err("`\\H` inside a character class".into()),
                'H' => out.push_str("[^0-9a-fA-F]"),
                'Z' if class == 0 => out.push_str("(?:\\n?\\z)"),
                'e' => out.push_str("\\x1B"),
                '<' | '>' => out.push(next),
                '1'..='9' if class == 0 => return Err(format!("backreference `\\{next}`")),
                'k' if class == 0 => return Err("named backreference `\\k`".into()),
                'g' if class == 0 => return Err("subexpression call `\\g`".into()),
                'K' | 'G' | 'R' | 'X' if class == 0 => {
                    return Err(format!("unsupported escape `\\{next}`"));
                }
                _ => {
                    out.push('\\');
                    out.push(next);
                }
            }
            continue;
        }
        if class > 0 {
            match c {
                '[' => {
                    // POSIX 클래스 `[:alpha:]` 는 겹친 클래스가 아니다.
                    if chars.get(i + 1) == Some(&':') {
                        let end = find_posix_end(&chars, i + 2)
                            .ok_or_else(|| "unterminated POSIX class".to_owned())?;
                        out.extend(&chars[i..=end]);
                        i = end + 1;
                        continue;
                    }
                    class += 1;
                    out.push('[');
                    i += 1;
                    if chars.get(i) == Some(&'^') {
                        out.push('^');
                        i += 1;
                    }
                }
                ']' => {
                    class -= 1;
                    out.push(']');
                    i += 1;
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
            continue;
        }
        match c {
            '[' => {
                class = 1;
                out.push('[');
                i += 1;
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                // 맨 앞의 `]` 는 글자다(Ruby · Rust 모두).
                if chars.get(i) == Some(&']') {
                    out.push_str("\\]");
                    i += 1;
                }
            }
            '(' if chars.get(i + 1) == Some(&'?') => {
                let rest: String = chars[i + 2..].iter().take(3).collect();
                if rest.starts_with('=') || rest.starts_with('!') {
                    return Err("lookahead".into());
                }
                if rest.starts_with("<=") || rest.starts_with("<!") {
                    return Err("lookbehind".into());
                }
                if rest.starts_with('>') {
                    return Err("atomic group".into());
                }
                if rest.starts_with('~') {
                    return Err("absence operator".into());
                }
                if rest.starts_with('(') {
                    return Err("conditional".into());
                }
                if rest.starts_with('#') {
                    // 주석 묶음 `(?#...)` — 버린다.
                    let end = chars[i..]
                        .iter()
                        .position(|&c| c == ')')
                        .ok_or_else(|| "unterminated comment group".to_owned())?;
                    i += end + 1;
                    continue;
                }
                if rest.starts_with('\'') {
                    // `(?'name'...)` → `(?P<name>...)`.
                    let end = chars[i + 3..]
                        .iter()
                        .position(|&c| c == '\'')
                        .ok_or_else(|| "unterminated group name".to_owned())?;
                    out.push_str("(?P<");
                    out.extend(&chars[i + 3..i + 3 + end]);
                    out.push('>');
                    i += 3 + end + 1;
                    continue;
                }
                // 플래그 묶음 `(?imx-imx)` · `(?imx-imx:...)`.
                let mut j = i + 2;
                while j < chars.len() && matches!(chars[j], 'i' | 'm' | 'x' | '-') {
                    j += 1;
                }
                if j > i + 2 && matches!(chars.get(j), Some(')' | ':')) {
                    out.push_str("(?");
                    for &f in &chars[i + 2..j] {
                        out.push(if f == 'm' { 's' } else { f });
                    }
                    i = j;
                    continue;
                }
                if j == i + 2 && matches!(chars.get(j), Some(':' | '<' | 'P')) {
                    out.push_str("(?");
                    i += 2;
                    continue;
                }
                return Err(format!(
                    "unsupported group `(?{}`",
                    chars.get(i + 2).copied().unwrap_or(' ')
                ));
            }
            '*' | '+' | '?' => {
                if c == '+' && was_quantifier {
                    return Err("possessive quantifier".into());
                }
                if c != '?' && was_counted {
                    return Err("repetition of a counted repetition".into());
                }
                out.push(c);
                i += 1;
                // `*?` 같은 게으른 반복의 `?` 뒤에는 다시 반복 기호가 올 수 없다.
                after_quantifier = !(c == '?' && was_quantifier);
            }
            '{' => match counted_repetition(&chars, i) {
                Some(end) => {
                    out.push('{');
                    // Rust 는 `{,m}` 을 모른다 — Ruby 에서는 `{0,m}` 이다.
                    if chars[i + 1] == ',' {
                        out.push('0');
                    }
                    out.extend(&chars[i + 1..=end]);
                    i = end + 1;
                    after_counted = true;
                }
                None => {
                    out.push_str("\\{");
                    i += 1;
                }
            },
            '}' => {
                out.push_str("\\}");
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    if class > 0 {
        return Err("unterminated character class".into());
    }
    Ok(out)
}

/// Ruby 의 `Regexp.union(a, b, …)` 처럼 옮긴 패턴들을 하나로 묶는다. 각 패턴의 인라인 플래그가 서로 새지 않게 묶음으로 감싼다.
pub fn union<'a>(translated: impl IntoIterator<Item = &'a str>) -> String {
    let parts: Vec<String> = translated.into_iter().map(|p| format!("(?:{p})")).collect();
    parts.join("|")
}

fn find_posix_end(chars: &[char], from: usize) -> Option<usize> {
    let mut k = from;
    while k + 1 < chars.len() {
        if chars[k] == ':' && chars[k + 1] == ']' {
            return Some(k + 1);
        }
        if !chars[k].is_ascii_alphabetic() && chars[k] != '^' {
            return None;
        }
        k += 1;
    }
    None
}

/// `{n}` · `{n,}` · `{n,m}` · `{,m}` 이면 닫는 `}` 의 위치.
fn counted_repetition(chars: &[char], open: usize) -> Option<usize> {
    let mut k = open + 1;
    let mut digits = 0;
    while chars.get(k).is_some_and(char::is_ascii_digit) {
        k += 1;
        digits += 1;
    }
    let mut after_comma = 0;
    let comma = chars.get(k) == Some(&',');
    if comma {
        k += 1;
        while chars.get(k).is_some_and(char::is_ascii_digit) {
            k += 1;
            after_comma += 1;
        }
    }
    if chars.get(k) != Some(&'}') || (digits == 0 && after_comma == 0) {
        return None;
    }
    Some(k)
}
