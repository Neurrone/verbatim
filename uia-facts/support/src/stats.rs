//! Medians, ranges, and the Wilson interval the rates are judged by.

/// A count as a float; the facts' counts are far below 2 to the 52nd.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "counts of trials and calls are small"
)]
pub fn as_f64(n: usize) -> f64 {
    n as f64
}

/// Microseconds as milliseconds.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "the facts' durations are far below 2 to the 52nd microseconds"
)]
pub fn us_to_ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

/// The median of `values`.
///
/// # Panics
///
/// On no values.
#[must_use]
pub fn median(values: &[f64]) -> f64 {
    assert!(!values.is_empty(), "a median of no values");
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    }
}

/// The smallest and largest of `values`.
#[must_use]
pub fn range(values: &[f64]) -> (f64, f64) {
    values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        })
}

/// A number with thousands separators and `decimals` places.
#[must_use]
pub fn number(value: f64, decimals: usize) -> String {
    let text = format!("{:.*}", decimals, value.abs());
    let (whole, fraction) = text
        .split_once('.')
        .map_or((text.as_str(), None), |(w, f)| (w, Some(f)));
    let mut grouped = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Durations in milliseconds as "median ms (n trials, low to high)".
#[must_use]
pub fn summary_ms(values: &[f64]) -> String {
    let (low, high) = range(values);
    let decimals = usize::from(high < 10_000.0);
    if values.len() == 1 {
        return format!("{} ms (1 trial)", number(values[0], decimals));
    }
    format!(
        "{} ms ({} trials, {} to {})",
        number(median(values), decimals),
        values.len(),
        number(low, decimals),
        number(high, decimals)
    )
}

/// The 95 percent Wilson score interval of `k` occurrences in `n` trials.
///
/// # Panics
///
/// When `n` is 0.
#[must_use]
pub fn wilson(k: usize, n: usize) -> (f64, f64) {
    assert!(n > 0, "a rate of no trials");
    let z = 1.959_964;
    let all = k == n;
    let n = as_f64(n);
    let p = as_f64(k) / n;
    let denominator = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denominator;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / denominator;
    // At k = 0 and k = n the interval reaches 0 and 1 exactly, which the
    // floating-point sum can miss by a rounding error.
    let low = if k == 0 {
        0.0
    } else {
        (centre - half).max(0.0)
    };
    let high = if all { 1.0 } else { (centre + half).min(1.0) };
    (low, high)
}
