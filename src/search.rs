//! SIMD-accelerated binary pattern search with wildcard support.
//!
//! Uses [`memchr`] internally — SSE2/AVX2 on x86-64, NEON on aarch64.
//! Exact patterns go through [`memchr::memmem`] (Two-Way + SIMD).
//! Wildcard patterns anchor on the first concrete byte via [`memchr::memchr_iter`],
//! then verify the full pattern.

use memchr::memmem;

/// A single element of a byte pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Match this exact byte.
    Byte(u8),
    /// Match any byte (hex `??`).
    Wildcard,
}

impl Pattern {
    #[inline(always)]
    pub fn matches(&self, byte: &u8) -> bool {
        match self {
            Self::Byte(b) => b == byte,
            Self::Wildcard => true,
        }
    }

    pub fn has_wildcards(patterns: &[Self]) -> bool {
        patterns.iter().any(|p| matches!(p, Self::Wildcard))
    }

    /// Extract raw bytes (panics on wildcards).
    pub fn as_bytes(patterns: &[Self]) -> Vec<u8> {
        patterns
            .iter()
            .map(|p| match p {
                Self::Byte(b) => *b,
                Self::Wildcard => panic!("cannot convert wildcard pattern to bytes"),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find all **non-overlapping** occurrences of `needle` in `haystack`.
pub fn find_all(haystack: &[u8], needle: &[Pattern]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return vec![];
    }
    if Pattern::has_wildcards(needle) {
        find_wildcard(haystack, needle)
    } else {
        find_exact(haystack, needle)
    }
}

/// Replace every non-overlapping match of `find` with `replace`.
///
/// `find` and `replace` must have the same length.
/// At wildcard positions the original byte is preserved.
pub fn replace_all(data: &[u8], find: &[Pattern], replace: &[u8]) -> Vec<u8> {
    debug_assert_eq!(find.len(), replace.len());
    if find.is_empty() || data.len() < find.len() {
        return data.to_vec();
    }
    if !Pattern::has_wildcards(find) {
        return replace_exact_streaming(data, find, replace);
    }
    let positions = find_all(data, find);
    replace_at_positions(data, &positions, find, replace)
}

/// Replace at pre-computed positions (useful when you already called [`find_all`]).
pub fn replace_at_positions(
    data: &[u8],
    positions: &[usize],
    find: &[Pattern],
    replace: &[u8],
) -> Vec<u8> {
    if positions.is_empty() {
        return data.to_vec();
    }
    let needle_len = find.len();
    let has_wc = Pattern::has_wildcards(find);
    let mut out = Vec::with_capacity(data.len());
    let mut last = 0usize;

    for &pos in positions {
        if pos < last {
            continue;
        }
        out.extend_from_slice(&data[last..pos]);
        if has_wc {
            for (i, pat) in find.iter().enumerate() {
                match pat {
                    Pattern::Wildcard => out.push(data[pos + i]),
                    Pattern::Byte(_) => out.push(replace[i]),
                }
            }
        } else {
            out.extend_from_slice(replace);
        }
        last = pos + needle_len;
    }
    out.extend_from_slice(&data[last..]);
    out
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Exact search via `memchr::memmem` (SIMD Two-Way).
fn find_exact(haystack: &[u8], needle: &[Pattern]) -> Vec<usize> {
    let bytes = Pattern::as_bytes(needle);
    memmem::find_iter(haystack, &bytes).collect()
}

/// Wildcard search: anchor on the first concrete byte, verify the rest.
fn find_wildcard(haystack: &[u8], needle: &[Pattern]) -> Vec<usize> {
    let needle_len = needle.len();

    // Anchor on the first non-wildcard byte.
    let (anchor_off, anchor_byte) = needle
        .iter()
        .enumerate()
        .find_map(|(i, p)| match p {
            Pattern::Byte(b) => Some((i, *b)),
            Pattern::Wildcard => None,
        })
        .expect("pattern must contain at least one non-wildcard byte");

    // Restrict search range so every candidate fits.
    let search_end = haystack.len() - needle_len + anchor_off + 1;
    if anchor_off >= search_end {
        return vec![];
    }

    let mut results = Vec::new();
    let mut min_next = 0usize;

    for rel in memchr::memchr_iter(anchor_byte, &haystack[anchor_off..search_end]) {
        let start = rel; // candidate start = relative offset in the slice
        if start < min_next {
            continue;
        }
        if needle
            .iter()
            .zip(&haystack[start..start + needle_len])
            .all(|(n, h)| n.matches(h))
        {
            results.push(start);
            min_next = start + needle_len;
        }
    }
    results
}

/// Single-pass exact replace (no intermediate positions vec).
fn replace_exact_streaming(data: &[u8], find: &[Pattern], replace: &[u8]) -> Vec<u8> {
    let find_bytes = Pattern::as_bytes(find);
    let mut out = Vec::with_capacity(data.len());
    let mut last = 0;
    for pos in memmem::find_iter(data, &find_bytes) {
        out.extend_from_slice(&data[last..pos]);
        out.extend_from_slice(replace);
        last = pos + find_bytes.len();
    }
    out.extend_from_slice(&data[last..]);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(bytes: &[u8]) -> Vec<Pattern> {
        bytes.iter().map(|&b| Pattern::Byte(b)).collect()
    }

    fn pat_wild(bytes: &[u8], wildcard: u8) -> Vec<Pattern> {
        bytes
            .iter()
            .map(|&b| {
                if b == wildcard {
                    Pattern::Wildcard
                } else {
                    Pattern::Byte(b)
                }
            })
            .collect()
    }

    #[test]
    fn exact_single_match() {
        let haystack: Vec<u8> = (0..200).collect();
        assert_eq!(find_all(&haystack, &pat(&[42, 43, 44])), vec![42]);
    }

    #[test]
    fn exact_multiple_matches() {
        let mut h = vec![0u8; 100];
        h[10..14].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        h[50..54].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(find_all(&h, &pat(&[0xDE, 0xAD, 0xBE, 0xEF])), vec![10, 50]);
    }

    #[test]
    fn exact_non_overlapping() {
        let data = vec![0xAA; 4];
        assert_eq!(find_all(&data, &pat(&[0xAA, 0xAA])), vec![0, 2]);
    }

    #[test]
    fn wildcard_match() {
        let haystack: Vec<u8> = (0..200).collect();
        let needle = pat_wild(&[42, 0xFF, 44], 0xFF);
        assert_eq!(find_all(&haystack, &needle), vec![42]);
    }

    #[test]
    fn wildcard_leading() {
        let haystack: Vec<u8> = (0..200).collect();
        let needle = pat_wild(&[0xFF, 0xFF, 44], 0xFF);
        assert_eq!(find_all(&haystack, &needle), vec![42]);
    }

    #[test]
    fn no_match() {
        let data = vec![0u8; 100];
        assert!(find_all(&data, &pat(&[0xDE, 0xAD])).is_empty());
    }

    #[test]
    fn empty_needle() {
        assert!(find_all(&[1, 2, 3], &[]).is_empty());
    }

    #[test]
    fn replace_exact_works() {
        let mut data = vec![0u8; 20];
        data[5..9].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let result = replace_all(
            &data,
            &pat(&[0xDE, 0xAD, 0xBE, 0xEF]),
            &[0x01, 0x02, 0x03, 0x04],
        );
        assert_eq!(&result[5..9], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(result.len(), data.len());
    }

    #[test]
    fn replace_wildcard_preserves_original() {
        let mut data = vec![0u8; 20];
        data[5..9].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let needle = pat_wild(&[0xDE, 0xFF, 0xBE, 0xEF], 0xFF);
        let result = replace_all(&data, &needle, &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&result[5..9], &[0x01, 0xAD, 0x03, 0x04]);
    }

    #[test]
    fn replace_multiple() {
        let mut data = vec![0u8; 100];
        data[10..14].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        data[50..54].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let result = replace_all(
            &data,
            &pat(&[0xDE, 0xAD, 0xBE, 0xEF]),
            &[0x01, 0x02, 0x03, 0x04],
        );
        assert_eq!(&result[10..14], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&result[50..54], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(result.len(), data.len());
    }
}
