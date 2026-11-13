use std::borrow::Cow;

use miette::{Diagnostic, SourceSpan};
use num_bigint::BigInt;
use thiserror::Error;

use crate::types::At;
use crate::{END_TRANSLATE_LEN, QUOTE_LEN, START_TRANSLATE_LEN};

pub trait NextChar {
    fn next_whitespace(&self) -> usize;
    fn next_non_whitespace(&self) -> usize;
}

impl NextChar for str {
    fn next_whitespace(&self) -> usize {
        self.find(char::is_whitespace).unwrap_or(self.len())
    }

    fn next_non_whitespace(&self) -> usize {
        self.find(|c: char| !c.is_whitespace())
            .unwrap_or(self.len())
    }
}

#[derive(Clone, Error, Debug, Diagnostic, PartialEq, Eq)]
pub enum LexerError {
    #[error("Expected a complete string literal")]
    IncompleteString {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Expected a complete translation string")]
    IncompleteTranslatedString {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Expected a valid variable name")]
    InvalidVariableName {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Could not parse the remainder")]
    InvalidRemainder {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Expected a string literal within translation")]
    MissingTranslatedString {
        #[label("here")]
        at: SourceSpan,
    },
}

/// A numeric literal.
#[derive(Debug, PartialEq)]
pub enum Number {
    Int(BigInt),
    Float(f64),
}

/// Whether Python's `\w` matches `character`.
///
/// That is what Django's patterns allow in variable, filter and keyword
/// names: a letter, a number of any kind (`²` and `Ⅷ` as well as digits) or
/// `_`. Combining marks and other connectors such as `‿` don't match, so a
/// name can't hold a decomposed accent.
#[must_use]
pub fn is_word_char(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_alphanumeric() || character == '_';
    }
    if character.is_numeric() {
        return true;
    }
    // `is_alphabetic` adds marks and enclosed letters to the letters.
    character.is_alphabetic()
        && !unicode_normalization::char::is_combining_mark(character)
        && !matches!(
            u32::from(character),
            0x24B6..=0x24E9 | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189
        )
}

/// The zero of each run of ten decimal digits (category Nd) outside ASCII,
/// from Unicode 17. Python's `int()`, `float()` and `\d` take any of them; a
/// Python built on an older Unicode knows a few fewer.
const DIGIT_ZEROS: [u32; 76] = [
    0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6,
    0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50,
    0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x10D40, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0,
    0x11650, 0x116C0, 0x116D0, 0x116DA, 0x11730, 0x118E0, 0x11950, 0x11BF0, 0x11C50, 0x11D50,
    0x11DA0, 0x11DE0, 0x11F50, 0x16130, 0x16A60, 0x16AC0, 0x16B50, 0x16D70, 0x1CCF0, 0x1D7CE,
    0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E5F1, 0x1E950, 0x1FBF0,
];

/// The value of a decimal digit in any script, as Python's `int()` reads it.
#[must_use]
pub fn decimal_digit(character: char) -> Option<u8> {
    if character.is_ascii() {
        return character.is_ascii_digit().then(|| character as u8 - b'0');
    }
    let code = u32::from(character);
    let run = DIGIT_ZEROS
        .partition_point(|zero| *zero <= code)
        .checked_sub(1)?;
    u8::try_from(code - DIGIT_ZEROS[run])
        .ok()
        .filter(|digit| *digit < 10)
}

/// `text` with digits of other scripts written in ASCII.
fn ascii_digits(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|character| match decimal_digit(character) {
                Some(digit) => char::from(b'0' + digit),
                None => character,
            })
            .collect(),
    )
}

/// Digits, with single underscores allowed between them, as Python's
/// `int()` and `float()` accept.
fn is_digits(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('_')
            .all(|group| !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A number as Django's `Variable` reads one.
///
/// That is Python's `float()` when the text has a `.` or an `e`, and `int()`
/// otherwise. So `1_000` is a number, `1_` and `2.` aren't, and `1e309` is
/// infinity. Digits of any script count, so `١٢` is 12.
#[must_use]
pub fn python_number(text: &str) -> Option<Number> {
    let text = &*ascii_digits(text);
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if !text.contains(['.', 'e', 'E']) {
        if !is_digits(unsigned) {
            return None;
        }
        let digits = text.trim_start_matches('+').replace('_', "");
        return digits.parse().ok().map(Number::Int);
    }
    // Django rejects a trailing dot, which `float()` would accept.
    if text.ends_with('.') {
        return None;
    }
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let valid = (whole.is_empty() || is_digits(whole))
        && (fraction.is_empty() || is_digits(fraction))
        && exponent.is_none_or(|exponent| {
            is_digits(exponent.strip_prefix(['-', '+']).unwrap_or(exponent))
        });
    if !valid {
        return None;
    }
    text.replace('_', "").parse().ok().map(Number::Float)
}

pub fn lex_variable(byte: usize, rest: &str) -> (At, usize, &str) {
    let mut in_text = None;
    // Inside a translated filter argument, such as `default:_("text")`.
    let mut in_translation = false;
    let mut end = 0;
    for c in rest.chars() {
        match c {
            '"' => match in_text {
                None => in_text = Some('"'),
                Some('"') => in_text = None,
                _ => {}
            },
            '\'' => match in_text {
                None => in_text = Some('\''),
                Some('\'') => in_text = None,
                _ => {}
            },
            _ if in_text.is_some() => {}
            '(' if !in_translation && rest[..end].ends_with(":_") => in_translation = true,
            ')' if in_translation => in_translation = false,
            c if !is_word_char(c) && c != '.' && c != '|' && c != ':' && c != '-' && c != '+' => {
                break;
            }
            _ => {}
        }
        end += c.len_utf8();
    }
    let at = (byte, end);
    let rest = &rest[end..];
    let byte = byte + end;
    (at, byte, rest)
}

/// Extend a literal's span over an adjacent filter chain in a tag expression.
pub(crate) fn lex_literal_filters(at: At, byte: usize, rest: &str) -> Option<(At, usize, &str)> {
    if !rest.starts_with('|') {
        return None;
    }
    let mut quote = None;
    let mut escaped = false;
    let mut end = 0;
    for character in rest.chars() {
        if escaped {
            escaped = false;
        } else if let Some(delimiter) = quote {
            if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character.is_whitespace() {
            break;
        }
        end += character.len_utf8();
    }
    Some(((at.0, at.1 + end), byte + end, &rest[end..]))
}

pub fn lex_text<'t>(
    byte: usize,
    rest: &'t str,
    chars: &mut std::str::Chars,
    end: char,
) -> Result<(At, usize, &'t str), LexerError> {
    let mut count = 1;
    loop {
        let Some(next) = chars.next() else {
            let at = (byte, count);
            return Err(LexerError::IncompleteString { at: at.into() });
        };
        count += next.len_utf8();
        if next == '\\' {
            let Some(next) = chars.next() else {
                let at = (byte, count);
                return Err(LexerError::IncompleteString { at: at.into() });
            };
            count += next.len_utf8();
        } else if next == end {
            let at = (byte, count);
            let rest = &rest[count..];
            let byte = byte + count;
            return Ok((at, byte, rest));
        }
    }
}

pub fn lex_translated<'t>(
    byte: usize,
    rest: &'t str,
    chars: &mut std::str::Chars,
) -> Result<(At, usize, &'t str), LexerError> {
    let start = byte;
    let byte = byte + START_TRANSLATE_LEN;
    let rest = &rest[START_TRANSLATE_LEN..];
    let (_at, byte, rest) = match chars.next() {
        None => {
            let at = (start, START_TRANSLATE_LEN);
            return Err(LexerError::MissingTranslatedString { at: at.into() });
        }
        Some('\'') => lex_text(byte, rest, chars, '\'')?,
        Some('"') => lex_text(byte, rest, chars, '"')?,
        _ => {
            let at = (start, rest.len() + START_TRANSLATE_LEN);
            return Err(LexerError::MissingTranslatedString { at: at.into() });
        }
    };
    if chars.next() == Some(')') {
        let byte = byte + END_TRANSLATE_LEN;
        let rest = &rest[END_TRANSLATE_LEN..];
        let at = (start, byte - start);
        Ok((at, byte, rest))
    } else {
        let at = (start, byte - start);
        Err(LexerError::IncompleteTranslatedString { at: at.into() })
    }
}

/// Django 6 accepts signed exponents and plus signs in numeric expressions.
pub fn lex_numeric_modern(byte: usize, rest: &str) -> (At, usize, &str) {
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E' | '_')))
        .unwrap_or(rest.len());
    ((byte, end), byte + end, &rest[end..])
}

pub fn trim_variable_modern(variable: &str) -> &str {
    match variable.find(|c: char| !is_word_char(c) && !matches!(c, '.' | '-' | '+')) {
        Some(end) => &variable[..end],
        None => variable,
    }
}

pub fn check_variable_attrs_modern(variable: &str, start: usize) -> Result<(), LexerError> {
    let mut offset = 0;
    for part in variable.split('.') {
        if part.starts_with('_') || part.contains('-') || part.contains('+') {
            return Err(LexerError::InvalidVariableName {
                at: (start + offset, part.len()).into(),
            });
        }
        offset += part.len() + 1;
    }
    Ok(())
}

pub fn lex_numeric(byte: usize, rest: &str) -> (At, usize, &str) {
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | '_')))
        .unwrap_or(rest.len());
    let content = &rest[..end];
    // Match django bug
    let end = match content[1..].find('-') {
        Some(n) => n + 1,
        None => end,
    };
    // End match django bug
    let at = (byte, end);
    (at, byte + end, &rest[end..])
}

pub fn trim_variable(variable: &str) -> &str {
    match variable.find(|c: char| !is_word_char(c) && c != '.' && c != '-') {
        Some(end) => &variable[..end],
        None => variable,
    }
}

pub fn check_variable_attrs(variable: &str, start: usize) -> Result<(), LexerError> {
    let mut offset = 0;
    for (i, var) in variable.split('.').enumerate() {
        if i == 0 {
            let mut chars = var.chars();
            chars.next();
            if chars.any(|c| c == '-') {
                let at = (start + offset, var.len());
                return Err(LexerError::InvalidVariableName { at: at.into() });
            }
        } else if var.find('-').is_some() {
            let at = (start + offset, var.len());
            return Err(LexerError::InvalidVariableName { at: at.into() });
        }

        // An empty part, as in `a..b` or `a.`, is a lookup that finds
        // nothing, as it is in Django.
        if var.starts_with('_') {
            let at = (start + offset, var.len());
            return Err(LexerError::InvalidVariableName { at: at.into() });
        }
        offset += var.len() + 1;
    }
    Ok(())
}

pub fn lex_variable_argument(byte: usize, rest: &str) -> Result<(At, usize, &str), LexerError> {
    let content = trim_variable(rest);
    check_variable_attrs(content, byte)?;
    let end = content.len();
    let at = (byte, end);
    Ok((at, byte + end, &rest[end..]))
}

pub fn text_content_at(at: At) -> At {
    let (start, len) = at;
    let start = start + QUOTE_LEN;
    let len = len - 2 * QUOTE_LEN;
    (start, len)
}

pub fn translated_text_content_at(at: At) -> At {
    let (start, len) = at;
    let start = start + START_TRANSLATE_LEN + QUOTE_LEN;
    let len = len - START_TRANSLATE_LEN - END_TRANSLATE_LEN - 2 * QUOTE_LEN;
    (start, len)
}

/// Returns a new `At` representing the span from the beginning of `start`
/// to the end of `end`.
///
/// # Panics
///
/// Panics if `end` starts before `start`.
pub fn get_all_at(start: At, end: At) -> At {
    assert!(
        end.0 >= start.0,
        "End position must be greater than or equal to start position"
    );
    (start.0, end.0 - start.0 + end.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex_text_non_ascii() {
        let template = "'N\u{ec655}'";
        let mut chars = template.chars();
        chars.next();
        let (at, byte, rest) = lex_text(1, template, &mut chars, '\'').unwrap();
        assert_eq!(at, (1, 7));
        assert_eq!(byte, 8);
        assert_eq!(rest, "");
    }

    #[test]
    fn test_lex_argument_non_ascii() {
        let template = "ZJ5G4YXZJUH6|default:\"#`´କ¯\"";
        let (at, byte, rest) = lex_variable(0, template);
        assert_eq!(at, (0, 32));
        assert_eq!(byte, 32);
        assert_eq!(rest, "");
    }

    #[test]
    fn test_lex_translated_argument() {
        let template = "x|default:_(\"Not started\") as y";
        let (at, byte, rest) = lex_variable(0, template);
        assert_eq!(at, (0, 26));
        assert_eq!(byte, 26);
        assert_eq!(rest, " as y");
    }

    #[test]
    fn test_lex_translated_argument_parenthesis_in_text() {
        let template = "x|default:_(')') y";
        let (at, _, rest) = lex_variable(0, template);
        assert_eq!(at, (0, 16));
        assert_eq!(rest, " y");
    }

    #[test]
    fn test_lex_parenthesis_outside_translation() {
        let template = "x(1)";
        let (at, _, rest) = lex_variable(0, template);
        assert_eq!(at, (0, 1));
        assert_eq!(rest, "(1)");
    }

    #[test]
    fn test_get_all_at_valid() {
        let start = (10, 5);
        let end = (20, 3);
        assert_eq!(get_all_at(start, end), (10, 13));
    }

    #[test]
    #[should_panic(expected = "End position must be greater than or equal to start position")]
    fn test_get_all_at_invalid_range() {
        let start = (20, 5);
        let end = (10, 3);
        get_all_at(start, end);
    }
}
