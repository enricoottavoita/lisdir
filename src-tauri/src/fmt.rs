//! Plain text helpers for the windows: sizes, counts, highlights and word edges. Nothing here touches Windows.

/// 1536 -> "1.5 KB". Binary units, like Explorer.
pub fn size(n: u64) -> String {
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    let unit = ["B", "KB", "MB", "GB", "TB"][i];
    if i > 0 && v < 10.0 { format!("{v:.1} {unit}") } else { format!("{} {unit}", v.round()) }
}

/// 1234567 -> "1,234,567" or "1.234.567".
pub fn count(n: usize, sep: char) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Char positions (as in `Hit::hl`) to runs of UTF-16 units.
pub fn marks(name: &str, hl: &[usize]) -> Vec<(u32, u32)> {
    let (mut out, mut at): (Vec<(u32, u32)>, u32) = (vec![], 0);
    for (i, c) in name.chars().enumerate() {
        let n = c.len_utf16() as u32;
        if hl.contains(&i) {
            match out.last_mut() {
                Some((start, len)) if *start + *len == at => *len += n,
                _ => out.push((at, n)),
            }
        }
        at += n;
    }
    out
}

/// Where the word before `at` starts, for Ctrl+Backspace (the text box has no such key of its own).
pub fn word_start(text: &[u16], at: usize) -> usize {
    let space = |i: usize| char::from_u32(text[i] as u32).is_some_and(char::is_whitespace);
    let mut i = at;
    while i > 0 && space(i - 1) {
        i -= 1;
    }
    while i > 0 && !space(i - 1) {
        i -= 1;
    }
    i
}

/// A count with the thousands separator of the language in use.
pub fn grouped(n: usize) -> String {
    count(n, if crate::italian() { '.' } else { ',' })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_like_explorer() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(20 * 1024), "20 KB");
        assert_eq!(size(3 << 30), "3.0 GB");
    }

    #[test]
    fn counts_have_separators() {
        assert_eq!(count(7, ','), "7");
        assert_eq!(count(1234, '.'), "1.234");
        assert_eq!(count(1234567, ','), "1,234,567");
    }

    #[test]
    fn highlights_become_utf16_runs() {
        assert_eq!(marks("abcd", &[1, 2]), vec![(1, 2)]);
        assert_eq!(marks("😀ab", &[1, 2]), vec![(2, 2)]); // the emoji is two units, one position
        assert_eq!(marks("a😀b", &[0, 1, 2]), vec![(0, 4)]);
        assert_eq!(marks("abc", &[0, 2]), vec![(0, 1), (2, 1)]);
    }

    #[test]
    fn ctrl_backspace_takes_a_word() {
        let t: Vec<u16> = "foo bar  ".encode_utf16().collect();
        assert_eq!(word_start(&t, 9), 4);
        assert_eq!(word_start(&t, 7), 4);
        assert_eq!(word_start(&t, 3), 0);
        assert_eq!(word_start(&t, 0), 0);
    }
}
