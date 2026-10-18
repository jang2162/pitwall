//! 여러 회의 값 → 중앙값과 범위.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Summary {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

/// 값이 없으면 `None`. 짝수 개면 가운데 둘의 평균이 중앙값이다.
pub fn summarize(values: &[f64]) -> Option<Summary> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let median = match n {
        0 => return None,
        _ if n % 2 == 1 => sorted[n / 2],
        _ => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    };
    Some(Summary {
        median,
        min: sorted[0],
        max: sorted[n - 1],
        n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_range() {
        let odd = summarize(&[900.0, 700.0, 800.0]).unwrap();
        assert_eq!(
            (odd.median, odd.min, odd.max, odd.n),
            (800.0, 700.0, 900.0, 3)
        );
        let even = summarize(&[4.0, 1.0, 3.0, 2.0]).unwrap();
        assert_eq!((even.median, even.min, even.max), (2.5, 1.0, 4.0));
        assert_eq!(summarize(&[]), None);
    }
}
