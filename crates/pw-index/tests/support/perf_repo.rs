//! 심볼 인덱스 성능의 합성 저장소(파일 2만 · 심볼 50만, M4-15 완료 기준 ④ · 결정 M4-D19 ⑤). release 시험 `tests/perf.rs` 와 앱으로 재는
//! 예제 `examples/perf_repo.rs`(M4-19 — `scripts/perf-m4.mjs` 가 부른다)가 같은 저장소를 만들도록 둘이 이 파일을 `#[path]` 로 함께 쓴다.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const FILES: usize = 20_000;
pub const SYMBOLS_PER_FILE: usize = 25;
/// 다 만든 뒤 쓰는 표식(내용은 소스 바이트 수). 있으면 다시 만들지 않는다.
pub const MARKER: &str = ".pitwall-perf-ready";

/// 메서드 하나의 본문(문장 여러 줄 — 심볼당 바이트를 이 저장소의 실제 코드 비율 400 ~ 500 B 에 맞춘다, 리뷰 L3). `{in}` 은 들여쓰기.
fn body(lang: usize) -> &'static str {
    match lang {
        0 => {
            "        // Doubles the input, then folds a few steps of noise into it.\n        let doubled = input * 2;\n        let mut total = doubled;\n        for step in 0..input.min(16) {\n            total = total.wrapping_add(step * 3 + 1);\n            if total % 7 == 0 {\n                total /= 2;\n            }\n        }\n        let label = format!(\"{total}:{doubled}\");\n        total + label.len() as u64\n"
        }
        1 => {
            "    // Doubles the input, then folds a few steps of noise into it.\n    const doubled = input * 2;\n    let total = doubled;\n    for (let step = 0; step < Math.min(input, 16); step++) {\n      total += step * 3 + 1;\n      if (total % 7 === 0) {\n        total = Math.floor(total / 2);\n      }\n    }\n    const label = `${total}:${doubled}`;\n    return total + label.length;\n"
        }
        2 => {
            "        # Doubles the value, then folds a few steps of noise into it.\n        doubled = value * 2\n        total = doubled\n        for step in range(min(value, 16)):\n            total += step * 3 + 1\n            if total % 7 == 0:\n                total //= 2\n        label = f\"{total}:{doubled}\"\n        return total + len(label)\n"
        }
        3 => {
            "        // Doubles the input, then folds a few steps of noise into it.\n        int doubled = input * 2;\n        int total = doubled;\n        for (int step = 0; step < Math.min(input, 16); step++) {\n            total += step * 3 + 1;\n            if (total % 7 == 0) {\n                total /= 2;\n            }\n        }\n        String label = total + \":\" + doubled;\n        return total + label.length();\n"
        }
        4 => {
            "        // Doubles the input, then folds a few steps of noise into it.\n        val doubled = input * 2\n        var total = doubled\n        for (step in 0 until minOf(input, 16)) {\n            total += step * 3 + 1\n            if (total % 7 == 0) {\n                total /= 2\n            }\n        }\n        val label = \"$total:$doubled\"\n        return total + label.length\n"
        }
        _ => {
            "    // Doubles the input, then folds a few steps of noise into it.\n    const doubled = input * 2;\n    let total = doubled;\n    for (let step = 0; step < Math.min(input, 16); step++) {\n      total += step * 3 + 1;\n      if (total % 7 === 0) {\n        total = Math.floor(total / 2);\n      }\n    }\n    const label = `${total}:${doubled}`;\n    return total + label.length;\n"
        }
    }
}

/// 파일 `i` 의 (상대 경로, 내용). 심볼은 정확히 [`SYMBOLS_PER_FILE`] 개.
pub fn file(i: usize) -> (String, String) {
    let dir = format!("pkg{:03}/mod{:02}", i / 200, (i / 10) % 20);
    let methods = SYMBOLS_PER_FILE - 1;
    let mut s = String::new();
    let verbs = ["handle", "render", "parse", "load", "update", "compute"];
    let name = |k: usize| format!("{}Item{i}x{k}", verbs[k % verbs.len()]);
    let lang = i % 6;
    let b = body(lang);
    match lang {
        0 => {
            let _ = writeln!(s, "pub struct Widget{i};\n\nimpl Widget{i} {{");
            for k in 0..methods - 1 {
                let _ = writeln!(
                    s,
                    "    /// Computes one item of the widget.\n    pub fn {}(&self, input: u64) -> u64 {{\n{b}    }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/widget{i}.rs"), s)
        }
        1 => {
            let _ = writeln!(s, "export class Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "  /** Computes one item of the widget. */\n  {}(input: number): number {{\n{b}  }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/widget{i}.ts"), s)
        }
        2 => {
            let _ = writeln!(s, "class Widget{i}:");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "    def {}(self, value):\n        \"\"\"Computes one item of the widget.\"\"\"\n{b}",
                    name(k)
                );
            }
            (format!("{dir}/widget{i}.py"), s)
        }
        3 => {
            let _ = writeln!(s, "package demo;\n\npublic class Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "    /** Computes one item of the widget. */\n    public int {}(int input) {{\n{b}    }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/Widget{i}.java"), s)
        }
        4 => {
            let _ = writeln!(s, "package demo\n\nclass Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "    /** Computes one item of the widget. */\n    fun {}(input: Int): Int {{\n{b}    }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/Widget{i}.kt"), s)
        }
        _ => {
            let _ = writeln!(s, "export class Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "  /** Computes one item of the widget. */\n  {}(input) {{\n{b}  }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/widget{i}.js"), s)
        }
    }
}

/// `dir` 에 합성 저장소를 만든다(표식이 있으면 그대로 둔다). 여섯 언어를 돌려 가며 파일마다 심볼 [`SYMBOLS_PER_FILE`] 개.
pub fn ensure(dir: &Path) -> PathBuf {
    let marker = dir.join(MARKER);
    if marker.exists() {
        return dir.to_path_buf();
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let mut bytes = 0usize;
    for i in 0..FILES {
        let (rel, text) = file(i);
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        bytes += text.len();
        std::fs::write(path, text).unwrap();
    }
    std::fs::write(&marker, bytes.to_string()).unwrap();
    dir.to_path_buf()
}
