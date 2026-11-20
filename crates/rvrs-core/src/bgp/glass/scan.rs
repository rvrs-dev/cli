//! Small hand scanners for the handful of patterns the looking glass pages are read with. They
//! work on bytes and match ASCII without regard to case, so every index they return sits on a
//! character boundary of the text they were given.

use std::ops::Range;

/// Where `needle` first appears in `text` at or after `from`, whatever its case. `needle` is
/// ASCII.
pub(super) fn find_ci(text: &str, needle: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || bytes.len() < needle.len() {
        return None;
    }
    (from..=bytes.len() - needle.len())
        .find(|&at| bytes[at..at + needle.len()].eq_ignore_ascii_case(needle))
}

/// Whether `text` holds `needle` at byte `at`, whatever its case.
pub(super) fn starts_with_ci(text: &str, at: usize, needle: &str) -> bool {
    text.as_bytes()
        .get(at..at + needle.len())
        .is_some_and(|slice| slice.eq_ignore_ascii_case(needle.as_bytes()))
}

/// What a pattern's `\s` takes: ASCII whitespace, the vertical tab included.
pub(super) fn is_space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == 0x0B
}

/// The first index at or after `at` that is not whitespace.
pub(super) fn skip_spaces(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(|&byte| is_space(byte)) {
        at += 1;
    }
    at
}

/// The first index at or after `at` that is not an ASCII digit.
pub(super) fn skip_digits(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    at
}

/// Reads `name = "value"` at byte `at`: the value's range and where the closing quote ends.
pub(super) fn quoted_attribute(text: &str, at: usize, name: &str) -> Option<(Range<usize>, usize)> {
    if !starts_with_ci(text, at, name) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut cursor = skip_spaces(bytes, at + name.len());
    if bytes.get(cursor) != Some(&b'=') {
        return None;
    }
    cursor = skip_spaces(bytes, cursor + 1);
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    let start = cursor + 1;
    let length = bytes[start..].iter().position(|&byte| byte == b'"')?;
    Some((start..start + length, start + length + 1))
}

/// Reads `name = value` or `name = "value"` at byte `at` for one known value, and returns where
/// the value ends, before its closing quote.
pub(super) fn named_at(text: &str, at: usize, value: &str) -> Option<usize> {
    if !starts_with_ci(text, at, "name") {
        return None;
    }
    let bytes = text.as_bytes();
    let mut cursor = skip_spaces(bytes, at + 4);
    if bytes.get(cursor) != Some(&b'=') {
        return None;
    }
    cursor = skip_spaces(bytes, cursor + 1);
    if bytes.get(cursor) == Some(&b'"') {
        cursor += 1;
    }
    starts_with_ci(text, cursor, value).then_some(cursor + value.len())
}

/// Every `<tag` in the text, as the range between the tag's name and its closing `>` (or the
/// end of the text when it never closes).
pub(super) fn tags<'a>(text: &'a str, tag: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut from = 0;
    std::iter::from_fn(move || {
        let start = find_ci(text, tag, from)?;
        let inside = start + tag.len();
        let end = text.as_bytes()[inside..]
            .iter()
            .position(|&byte| byte == b'>')
            .map_or(text.len(), |offset| inside + offset);
        from = start + 1;
        Some(inside..end)
    })
}

/// The quoted value of one attribute on every `<tag`, in order: `<option value="…">`,
/// `<form action="…">`. Where a tag carries the attribute twice, the last one counts.
pub(super) fn tag_attributes<'a>(text: &'a str, tag: &str, attribute: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    // A value may hold a `>`, so a tag that starts inside the last match is skipped.
    let mut consumed = 0;
    for span in tags(text, tag) {
        if span.start < consumed {
            continue;
        }
        let hit = span
            .clone()
            .chain(std::iter::once(span.end))
            .rev()
            .find_map(|at| quoted_attribute(text, at, attribute));
        if let Some((value, end)) = hit {
            found.push(&text[value]);
            consumed = end;
        }
    }
    found
}

/// The text cut at every line break a router or a page may use.
pub(super) fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character| {
        matches!(
            character,
            '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
    })
}

/// The text without the spaces and tabs around it.
pub(super) fn trim_blanks(text: &str) -> &str {
    text.trim_matches(|character| character == ' ' || character == '\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_needle_is_found_whatever_its_case() {
        assert_eq!(find_ci("<HTML><Title>x", "<title>", 0), Some(6));
        assert_eq!(find_ci("<HTML><Title>x", "<title>", 7), None);
        assert_eq!(find_ci("ab", "abc", 0), None);
    }

    #[test]
    fn the_last_attribute_of_a_tag_is_the_one_read() {
        let page = r#"<option value="a"><OPTION class=x VALUE = "b &gt; c"> <option>none"#;
        assert_eq!(tag_attributes(page, "<option", "value"), ["a", "b &gt; c"]);
        let twice = r#"<option value="first" value="last">"#;
        assert_eq!(tag_attributes(twice, "<option", "value"), ["last"]);
    }

    #[test]
    fn a_name_is_read_with_or_without_quotes() {
        assert_eq!(named_at(r#"name="prefix" "#, 0, "prefix"), Some(12));
        assert_eq!(named_at("NAME = prefix>", 0, "prefix"), Some(13));
        assert_eq!(named_at("name=addr", 0, "prefix"), None);
    }
}
