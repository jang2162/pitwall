//! 리터럴 왕복 시험의 무작위 값(드라이버 크레이트의 live 시험이 쓴다 — 기능 `test-support`).
//!
//! 시험 하나: 열 타입마다 무작위 값을 [`super::literal`] 로 만든 `INSERT` 와 바인드 값 `INSERT` 로 한 행씩 넣고, 다시 읽은 두
//! 값이 같고(그리고 [`Sample::exact`] 면 원래 값과도 같은지) 본다. 시드는 `PW_LITERAL_SEED` 로 고정하고, 실패하면 시험이 그
//! 시드를 보인다.

use pw_db_types::{CellLimit, DbValue, SqlDialect};

/// 결정적 의사 난수(xorshift64*). 시험 재현용이다.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// `PW_LITERAL_SEED` 가 있으면 그것, 없으면 시각에서. 고른 시드를 돌려준다.
    pub fn from_env() -> (Self, u64) {
        let seed = std::env::var("PW_LITERAL_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(1)
            });
        (Self::new(seed), seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// 문자열에 섞을 까다로운 글자들.
const TRICKY: &[char] = &[
    '\'', '"', '\\', '`', ']', '[', '{', '}', ',', ';', '-', '/', '*', '%', '_', '$', '\n', '\r',
    '\t', '\u{7}', '\u{1b}', '\u{7f}', '\u{85}', 'é', '한', '글', '😀', '\u{200b}', '\u{feff}',
    ' ',
];

/// 무작위 글. `nul` 이면 NUL 도 섞는다.
pub fn text(rng: &mut Rng, nul: bool) -> String {
    let len = rng.below(24) as usize;
    let mut s = String::new();
    for _ in 0..len {
        let c = match rng.below(10) {
            0..=3 => TRICKY[rng.below(TRICKY.len() as u64) as usize],
            4 if nul => '\0',
            5 => {
                // 아무 유니코드 스칼라(대리 영역 밖).
                loop {
                    if let Some(c) = char::from_u32(rng.below(0x11_0000) as u32)
                        && c != '\0'
                    {
                        break c;
                    }
                }
            }
            _ => (b' ' + rng.below(95) as u8) as char,
        };
        s.push(c);
    }
    s
}

pub fn bytes(rng: &mut Rng) -> Vec<u8> {
    let len = rng.below(40) as usize;
    (0..len)
        .map(|_| match rng.below(4) {
            0 => b'\'',
            1 => b'\\',
            2 => 0,
            _ => rng.next_u64() as u8,
        })
        .collect()
}

/// 정수(경계값 섞음). `unsigned_big` 이면 i64 를 넘는 u64 도.
pub fn int(rng: &mut Rng, unsigned_big: bool) -> DbValue {
    match rng.below(8) {
        0 => DbValue::int(i64::MIN),
        1 => DbValue::int(i64::MAX),
        2 => DbValue::int(9_007_199_254_740_993i64),
        3 if unsigned_big => DbValue::int(u64::MAX - rng.below(1000)),
        4 => DbValue::int(0),
        _ => DbValue::int(rng.next_u64() as i64 >> rng.below(63)),
    }
}

/// 10진 글(`-123.0450`). 정수부 `int_digits`, 소수부 `scale` 까지.
pub fn decimal(rng: &mut Rng, int_digits: u64, scale: u64) -> String {
    let mut s = String::new();
    if rng.chance(40) {
        s.push('-');
    }
    let n = 1 + rng.below(int_digits);
    for i in 0..n {
        let d = rng.below(10) as u8;
        s.push((b'0' + if i == 0 && n > 1 { d.max(1) } else { d }) as char);
    }
    let f = rng.below(scale + 1);
    if f > 0 {
        s.push('.');
        for _ in 0..f {
            s.push((b'0' + rng.below(10) as u8) as char);
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

/// 유한한 실수(아주 크고 작은 것 섞음, -0 은 빼고).
pub fn float(rng: &mut Rng) -> f64 {
    loop {
        let f = match rng.below(4) {
            0 => f64::from_bits(rng.next_u64()),
            1 => (rng.next_u64() as i64) as f64 / 1e6,
            2 => 0.1 * rng.below(1000) as f64,
            _ => (rng.below(2_000_000) as f64 - 1_000_000.0) * 1e-3,
        };
        if f.is_finite() && f.to_bits() != (-0.0f64).to_bits() {
            return f;
        }
    }
}

/// `YYYY-MM-DD HH:MM:SS.ffffff`(마이크로초는 있을 때만).
pub fn datetime(rng: &mut Rng, micros: bool) -> String {
    let s = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        1000 + rng.below(8999),
        1 + rng.below(12),
        1 + rng.below(28),
        rng.below(24),
        rng.below(60),
        rng.below(60)
    );
    if micros && rng.chance(70) {
        format!("{s}.{:06}", 1 + rng.below(999_999))
    } else {
        s
    }
}

/// JSON 글(이스케이프 · 유니코드 섞음). `serde_json` 없이 만든다 — 글자 하나하나를 JSON 문자열 규칙대로.
pub fn json(rng: &mut Rng) -> String {
    let mut s = String::from("{\"k\":\"");
    for c in text(rng, false).chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push_str(&format!("\",\"n\":{}}}", rng.below(1000)));
    s
}

/// 열 하나의 표본.
pub struct Sample {
    /// `CREATE TABLE` 의 열 타입.
    pub column_type: &'static str,
    pub value: DbValue,
    /// 다시 읽은 값이 `value` 와 그대로 같아야 하는가(드라이버가 표현을 바꾸지 않는 타입).
    pub exact: bool,
}

fn sample(column_type: &'static str, value: DbValue, exact: bool) -> Sample {
    Sample {
        column_type,
        value,
        exact,
    }
}

/// 방언의 표본 `n` 벌(타입마다 하나씩 × n).
pub fn samples(dialect: SqlDialect, rng: &mut Rng, n: usize) -> Vec<Sample> {
    let lim = CellLimit::DEFAULT;
    let mut out = Vec::new();
    for _ in 0..n {
        let null = rng.chance(5);
        let maybe_null = |v: DbValue| if null { DbValue::Null } else { v };
        match dialect {
            SqlDialect::Postgres => {
                out.push(sample(
                    "text",
                    maybe_null(DbValue::text(text(rng, false), lim)),
                    true,
                ));
                out.push(sample(
                    "bytea",
                    maybe_null(DbValue::bytes(&bytes(rng), lim)),
                    true,
                ));
                out.push(sample("int8", maybe_null(int(rng, false)), true));
                out.push(sample(
                    "numeric",
                    maybe_null(DbValue::Decimal(decimal(rng, 40, 30))),
                    true,
                ));
                out.push(sample(
                    "float8",
                    maybe_null(DbValue::float(float(rng))),
                    true,
                ));
                out.push(sample(
                    "bool",
                    maybe_null(DbValue::Bool(rng.chance(50))),
                    true,
                ));
                out.push(sample(
                    "timestamp",
                    maybe_null(DbValue::Datetime(datetime(rng, true))),
                    false,
                ));
                out.push(sample("json", maybe_null(DbValue::Json(json(rng))), true));
                let items = (0..rng.below(5))
                    .map(|_| {
                        if rng.chance(15) {
                            DbValue::Null
                        } else {
                            DbValue::text(text(rng, false), lim)
                        }
                    })
                    .collect();
                out.push(sample("text[]", maybe_null(DbValue::Array(items)), false));
            }
            SqlDialect::Mysql => {
                out.push(sample(
                    "TEXT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin",
                    maybe_null(DbValue::text(text(rng, true), lim)),
                    true,
                ));
                out.push(sample(
                    "LONGBLOB",
                    maybe_null(DbValue::bytes(&bytes(rng), lim)),
                    true,
                ));
                out.push(sample("BIGINT", maybe_null(int(rng, false)), true));
                out.push(sample("BIGINT UNSIGNED", maybe_null(unsigned(rng)), true));
                out.push(sample(
                    "DECIMAL(65,30)",
                    maybe_null(DbValue::Decimal(decimal(rng, 35, 30))),
                    false,
                ));
                out.push(sample(
                    "DOUBLE",
                    maybe_null(DbValue::float(float(rng))),
                    true,
                ));
                out.push(sample(
                    "DATETIME(6)",
                    maybe_null(DbValue::Datetime(datetime(rng, true))),
                    false,
                ));
                out.push(sample("JSON", maybe_null(DbValue::Json(json(rng))), false));
            }
            SqlDialect::Sqlite => {
                out.push(sample(
                    "TEXT",
                    maybe_null(DbValue::text(text(rng, true), lim)),
                    true,
                ));
                out.push(sample(
                    "BLOB",
                    maybe_null(DbValue::bytes(&bytes(rng), lim)),
                    true,
                ));
                out.push(sample("INTEGER", maybe_null(int(rng, false)), true));
                out.push(sample("REAL", maybe_null(DbValue::float(float(rng))), true));
            }
        }
    }
    out
}

fn unsigned(rng: &mut Rng) -> DbValue {
    match rng.below(3) {
        0 => DbValue::int(u64::MAX),
        1 => DbValue::int(rng.next_u64()),
        _ => DbValue::int(rng.below(1000)),
    }
}
