//! Django's `Urlizer.handle_word` for ASCII words, ported from
//! `django/utils/html.py` (5.2, 6.0, 6.1 and 6.2).
//!
//! `handle_word` returns `None` when a word needs behavior that is hard to
//! match exactly, and the caller then passes that word to Django's `urlizer`:
//!
//! - non-ASCII words (`\w`, `re.IGNORECASE` and IDN handling),
//! - links whose remaining text has `[` or `]` (IPv6 hosts, `urlsplit`
//!   errors, and email address literals), or whose `html.unescape`d text is
//!   non-ASCII or has brackets, tabs or newlines,
//! - on 6.0+, `www.` and bare-domain links while `URLIZE_ASSUME_HTTPS` is off,
//!   so Django emits its `RemovedInDjango2028Warning`.
//!
//! Words with `&` stay native: `html.unescape` is the one Python call, made
//! through the `unescape` callback, and only for text containing `&`.

use pyo3::PyResult;

use crate::render::types::append_escaped;

/// Django's `MAX_URL_LENGTH`.
const MAX_URL_LENGTH: usize = 2048;
/// The original seven gTLDs that `simple_url_2_re` recognizes.
const GTLDS: [&str; 7] = ["com", "edu", "gov", "int", "mil", "net", "org"];
/// `RFC3986_SUBDELIMS + RFC3986_GENDELIMS + "~"`, kept by `smart_urlquote`.
const URL_SAFE: &[u8] = b"!$&'()*+,;=:/?#[]@~";

/// Whether `urlize` can build links natively, once a render has checked.
#[derive(Clone, Copy, Debug, Default)]
pub enum NativeLinks {
    #[default]
    Unchecked,
    Rules(UrlizeRules),
    /// Django's `urlizer` is customized, or this series isn't ported.
    Django,
}

/// The parts of `Urlizer` that changed between Django series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UrlizeRules {
    /// 6.0+: `simple_url_2_re` uses `DomainNameValidator`'s patterns, and
    /// links without a scheme follow `URLIZE_ASSUME_HTTPS`.
    pub domain_validator: bool,
    /// 6.1+: `trim_punctuation` keeps earlier trailing text when it strips
    /// unbalanced closing brackets, where 5.2 and 6.0 replace it.
    pub accumulate_trail: bool,
    /// `settings.URLIZE_ASSUME_HTTPS`; only read on 6.0+.
    pub assume_https: bool,
}

impl UrlizeRules {
    /// The rules for a Django series, or `None` for a series not ported here.
    pub fn for_series(series: (u8, u8), assume_https: bool) -> Option<Self> {
        let (domain_validator, accumulate_trail) = match series {
            (5, 2) => (false, false),
            (6, 0) => (true, false),
            (6, 1 | 2) => (true, true),
            _ => return None,
        };
        Some(Self {
            domain_validator,
            accumulate_trail,
            assume_https,
        })
    }
}

/// `Urlizer.handle_word` for a word that may become a link.
///
/// Called as `handle_word(word, safe_input=safe, trim_url_limit=limit,
/// nofollow=True, autoescape=autoescape)`. Returns `None` to leave the word
/// to Django. `unescape` is `html.unescape`.
pub fn handle_word(
    word: &str,
    safe: bool,
    limit: Option<i64>,
    autoescape: bool,
    rules: UrlizeRules,
    unescape: &mut impl FnMut(&str) -> PyResult<String>,
) -> PyResult<Option<String>> {
    if !word.is_ascii() {
        return Ok(None);
    }
    if !word.contains(['.', '@', ':']) {
        return Ok(Some(plain_word(word, safe, autoescape)));
    }
    let (lead, middle, trail) = trim_punctuation(word, rules.accumulate_trail, unescape)?;
    if middle.contains(['[', ']']) {
        return Ok(None);
    }
    let mut nofollow = true;
    let url = if middle.len() <= MAX_URL_LENGTH && is_simple_url(middle) {
        let Some(url) = unescaped_url(middle, unescape)? else {
            return Ok(None);
        };
        smart_urlquote(&url)
    } else if middle.len() <= MAX_URL_LENGTH && is_simple_url_2(middle, rules) {
        if rules.domain_validator && !rules.assume_https {
            // Django warns here; let it.
            return Ok(None);
        }
        let Some(url) = unescaped_url(middle, unescape)? else {
            return Ok(None);
        };
        let scheme = if rules.domain_validator {
            "https"
        } else {
            "http"
        };
        smart_urlquote(&format!("{scheme}://{url}"))
    } else if !middle.contains(':') && is_email_simple(middle) {
        nofollow = false;
        let Some((local, domain)) = middle.rsplit_once('@') else {
            return Ok(None);
        };
        format!("mailto:{}@{}", quote(local, b""), quote(domain, b""))
    } else {
        return Ok(Some(plain_word(word, safe, autoescape)));
    };
    let trimmed = trim_url(middle, limit);
    let escape = autoescape && !safe;
    let mut output = String::with_capacity(word.len() * 2 + url.len() + 32);
    push(&mut output, lead, escape);
    output.push_str("<a href=\"");
    append_escaped(&url, &mut output);
    output.push('"');
    if nofollow {
        output.push_str(" rel=\"nofollow\"");
    }
    output.push('>');
    push(&mut output, &trimmed, escape);
    output.push_str("</a>");
    push(&mut output, &trail, escape);
    Ok(Some(output))
}

/// `html.unescape(middle)`, or `None` when the result needs Django:
/// non-ASCII text, brackets, or characters `urlsplit` removes.
fn unescaped_url(
    middle: &str,
    unescape: &mut impl FnMut(&str) -> PyResult<String>,
) -> PyResult<Option<String>> {
    if !middle.contains('&') {
        return Ok(Some(middle.to_string()));
    }
    let url = unescape(middle)?;
    let native = url.is_ascii() && !url.contains(['[', ']', '\t', '\r', '\n']);
    Ok(native.then_some(url))
}

fn push(output: &mut String, text: &str, escape: bool) {
    if escape {
        append_escaped(text, output);
    } else {
        output.push_str(text);
    }
}

/// A word that doesn't become a link: escaped unless it's safe.
fn plain_word(word: &str, safe: bool, autoescape: bool) -> String {
    let mut output = String::with_capacity(word.len());
    push(&mut output, word, autoescape && !safe);
    output
}

/// `Urlizer.trim_punctuation` for ASCII text. `unescape` is `html.unescape`,
/// used to tell a trailing `;` from the end of an HTML entity.
fn trim_punctuation<'w>(
    word: &'w str,
    accumulate_trail: bool,
    unescape: &mut impl FnMut(&str) -> PyResult<String>,
) -> PyResult<(&'w str, &'w str, String)> {
    const PUNCTUATION: [char; 5] = ['.', ',', ':', ';', '!'];
    const NO_SEMICOLON: [char; 4] = ['.', ',', ':', '!'];
    let mut middle = word.trim_start_matches(['(', '[']);
    let lead = &word[..word.len() - middle.len()];
    let mut trail = String::new();
    // `CountsDict(word=middle)`: counts in the middle before any trimming.
    let count = |character| middle.matches(character).count();
    let mut counts = [(count('('), count(')'), ')'), (count('['), count(']'), ']')];
    let mut trimmed_something = true;
    while trimmed_something && !middle.is_empty() {
        trimmed_something = false;
        for (opening, closing, character) in &mut counts {
            if *opening < *closing && middle.ends_with(*character) {
                // Python slices `middle[-strip:]`, which may take more than
                // the closing brackets themselves.
                let strip = *closing - *opening;
                let cut = middle.len().saturating_sub(strip);
                if accumulate_trail {
                    trail.insert_str(0, &middle[cut..]);
                } else {
                    trail = middle[cut..].to_string();
                }
                middle = &middle[..cut];
                trimmed_something = true;
                *closing -= strip;
            }
        }
        let amp = middle.rfind('&');
        let stripped = if amp.is_some() {
            middle.trim_end_matches(NO_SEMICOLON)
        } else {
            middle.trim_end_matches(PUNCTUATION)
        };
        if stripped.len() != middle.len() {
            trail.insert_str(0, &middle[stripped.len()..]);
            middle = stripped;
            trimmed_something = true;
        }
        if middle.ends_with(';') {
            // Only strip if not part of an HTML entity. Python's
            // `middle[amp:]` with `amp == -1` is the last character.
            let potential_entity = &middle[amp.unwrap_or(middle.len() - 1)..];
            let escaped = unescape(potential_entity)?;
            if escaped == potential_entity || escaped.ends_with(';') {
                let trail_start = middle.trim_end_matches(PUNCTUATION).len();
                let semicolons = middle.len() - middle.trim_end_matches(';').len();
                let cut = if amp.is_some() && semicolons > 1 {
                    // Leave up to the most recent semicolon: it might end an
                    // entity.
                    middle[trail_start..]
                        .find(';')
                        .map_or(trail_start, |index| trail_start + index + 1)
                } else {
                    trail_start
                };
                trail.insert_str(0, &middle[cut..]);
                middle = &middle[..cut];
                trimmed_something = true;
            }
        }
    }
    Ok((lead, middle, trail))
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `simple_url_re`: `^https?://\[?\w`, ignoring case. `[` is handled by Django.
fn is_simple_url(middle: &str) -> bool {
    ["http://", "https://"].iter().any(|scheme| {
        starts_with_ignore_case(middle, scheme)
            && middle
                .as_bytes()
                .get(scheme.len())
                .is_some_and(|&byte| is_word_byte(byte))
    })
}

fn is_gtld(label: &str) -> bool {
    GTLDS.iter().any(|tld| label.eq_ignore_ascii_case(tld))
}

/// `simple_url_2_re`, ignoring case:
/// - 5.2: `^www\.|^(?!http)\w[^@]+\.(com|…|org)($|/.*)$`
/// - 6.0+: `^www\.|^(?!http)(?:hostname)(?:domain)\.(com|…|org)($|/.*)$`
fn is_simple_url_2(middle: &str, rules: UrlizeRules) -> bool {
    if starts_with_ignore_case(middle, "www.") {
        return true;
    }
    if starts_with_ignore_case(middle, "http") {
        return false;
    }
    let bytes = middle.as_bytes();
    if rules.domain_validator {
        // Labels can't contain `/`, so the host ends at the first one.
        let host = middle.split('/').next().unwrap_or_default();
        let labels: Vec<&str> = host.split('.').collect();
        let Some((tld, rest)) = labels.split_last() else {
            return false;
        };
        return !rest.is_empty() && is_gtld(tld) && rest.iter().all(|label| is_label(label));
    }
    if !bytes.first().is_some_and(|&byte| is_word_byte(byte)) {
        return false;
    }
    // `[^@]+` covers everything between the first character and the dot.
    let at = middle[1..]
        .find('@')
        .map_or(middle.len(), |index| index + 1);
    (2..middle.len()).any(|dot| {
        bytes[dot] == b'.'
            && dot <= at
            && middle.get(dot + 1..dot + 4).is_some_and(is_gtld)
            && (dot + 4 == middle.len() || bytes[dot + 4] == b'/')
    })
}

/// `DomainNameValidator.hostname_re` and the labels in `domain_re`, for
/// ASCII: 1–63 letters, digits or `-`, not starting or ending with `-`.
fn is_label(label: &str) -> bool {
    (1..=63).contains(&label.len())
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

/// `DomainNameValidator.tld_no_fqdn_re` after its dot, for ASCII.
fn is_tld(label: &str) -> bool {
    let letters = (2..=63).contains(&label.len())
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-');
    let punycode = starts_with_ignore_case(label, "xn--")
        && (1..=59).contains(&(label.len() - 4))
        && label[4..].bytes().all(|byte| byte.is_ascii_alphanumeric());
    letters || punycode
}

/// `Urlizer.is_email_simple`: `EmailValidator(allowlist=[])` for ASCII
/// without `[`, so no quoted local part (`"` splits words) or IP literal.
fn is_email_simple(value: &str) -> bool {
    if value.is_empty() || value.len() > 320 {
        return false;
    }
    let Some((user, domain)) = value.rsplit_once('@') else {
        return false;
    };
    // The dot-atom form of `user_regex`.
    let atom_byte =
        |byte: u8| byte.is_ascii_alphanumeric() || b"-!#$%&'*+/=?^_`{}|~".contains(&byte);
    let user_ok = user
        .split('.')
        .all(|atom| !atom.is_empty() && atom.bytes().all(atom_byte));
    // `domain_regex`: hostname, more labels, then a TLD, without a final dot.
    let labels: Vec<&str> = domain.split('.').collect();
    let domain_ok = match labels.split_last() {
        Some((tld, rest)) => !rest.is_empty() && rest.iter().all(|l| is_label(l)) && is_tld(tld),
        None => false,
    };
    user_ok && domain_ok
}

/// `Urlizer.trim_url`.
fn trim_url(text: &str, limit: Option<i64>) -> String {
    match limit {
        Some(limit) if i64::try_from(text.len()).unwrap_or(i64::MAX) > limit => {
            let keep = usize::try_from(limit.saturating_sub(1)).unwrap_or(0);
            format!("{}…", &text[..keep.min(text.len())])
        }
        _ => text.to_string(),
    }
}

/// Python's `urllib.parse.quote(text, safe)` for UTF-8.
fn quote(text: &str, safe: &[u8]) -> String {
    let mut output = String::with_capacity(text.len());
    quote_into(text, safe, false, &mut output);
    output
}

fn quote_into(text: &str, safe: &[u8], plus: bool, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte) || safe.contains(&byte) {
            output.push(char::from(byte));
        } else if plus && byte == b' ' {
            output.push('+');
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0xF)]));
        }
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Python's `urllib.parse.unquote(text)`: each run of ASCII characters is
/// percent-decoded and decoded as UTF-8 with `errors="replace"`.
fn unquote(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let ascii = rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len());
        let (run, remaining) = rest.split_at(ascii);
        let bytes = run.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%'
                && let (Some(&high), Some(&low)) = (bytes.get(index + 1), bytes.get(index + 2))
                && let (Some(high), Some(low)) = (hex_value(high), hex_value(low))
            {
                decoded.push(high << 4 | low);
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        output.push_str(&String::from_utf8_lossy(&decoded));
        let other = remaining
            .find(|c: char| c.is_ascii())
            .unwrap_or(remaining.len());
        output.push_str(&remaining[..other]);
        rest = &remaining[other..];
    }
    output
}

/// `smart_urlquote`'s `unquote_quote`.
fn unquote_quote(segment: &str) -> String {
    quote(&unquote(segment), URL_SAFE)
}

/// Python's `urlsplit` for ASCII URLs without brackets in the host.
fn urlsplit(url: &str) -> (String, &str, &str, &str, &str) {
    let (scheme, rest) = match url.find(':') {
        Some(colon)
            if colon > 0
                && url.as_bytes()[0].is_ascii_alphabetic()
                && url[..colon]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte)) =>
        {
            (url[..colon].to_ascii_lowercase(), &url[colon + 1..])
        }
        _ => (String::new(), url),
    };
    let (netloc, rest) = match rest.strip_prefix("//") {
        Some(after) => after.split_at(after.find(['/', '?', '#']).unwrap_or(after.len())),
        None => ("", rest),
    };
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    (scheme, netloc, path, query, fragment)
}

/// Django's `smart_urlquote` for the URLs `handle_word` builds: ASCII, with a
/// scheme and a host, and without brackets.
fn smart_urlquote(url: &str) -> String {
    let (scheme, netloc, path, query, fragment) = urlsplit(url);
    let netloc = unquote_quote(netloc);
    // `urlencode` of `parse_qsl(query, keep_blank_values=True)`, with each
    // name and value unquoted once more, as Django does.
    let mut encoded = String::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if !encoded.is_empty() {
            encoded.push('&');
        }
        let name = unquote(&unquote(&name.replace('+', " ")));
        let value = unquote(&unquote(&value.replace('+', " ")));
        quote_into(&name, b"", true, &mut encoded);
        encoded.push('=');
        quote_into(&value, b"", true, &mut encoded);
    }
    let path = unquote_quote(path);
    let fragment = unquote_quote(fragment);
    // `urlunsplit`: the host is never empty here and the path starts with `/`.
    let mut output = String::with_capacity(url.len() + 16);
    if !scheme.is_empty() {
        output.push_str(&scheme);
        output.push(':');
    }
    output.push_str("//");
    output.push_str(&netloc);
    if !path.is_empty() && !path.starts_with('/') {
        output.push('/');
    }
    output.push_str(&path);
    if !encoded.is_empty() {
        output.push('?');
        output.push_str(&encoded);
    }
    if !fragment.is_empty() {
        output.push('#');
        output.push_str(&fragment);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `html.unescape` for text without entities.
    #[allow(clippy::unnecessary_wraps)]
    fn no_entities(text: &str) -> PyResult<String> {
        Ok(text.to_string())
    }

    const RULES_52: UrlizeRules = UrlizeRules {
        domain_validator: false,
        accumulate_trail: false,
        assume_https: false,
    };
    const RULES_61: UrlizeRules = UrlizeRules {
        domain_validator: true,
        accumulate_trail: true,
        assume_https: true,
    };

    #[test]
    fn test_trim_punctuation() {
        // 5.2 and 6.0 replace the trail when they strip a closing bracket, so
        // the `.` is lost; 6.1 keeps it. Both match Django's output.
        assert_eq!(
            trim_punctuation("(http://a.com/).", false, &mut no_entities).unwrap(),
            ("(", "http://a.com/", ")".to_string())
        );
        assert_eq!(
            trim_punctuation("(http://a.com/).", true, &mut no_entities).unwrap(),
            ("(", "http://a.com/", ").".to_string())
        );
        // Python slices `middle[-strip:]`, which can take more than brackets.
        assert_eq!(
            trim_punctuation("a.com).)", true, &mut no_entities).unwrap(),
            ("", "a.com)", ".)".to_string())
        );
    }

    #[test]
    fn test_simple_url_2() {
        assert!(is_simple_url_2("www.x", RULES_52));
        assert!(is_simple_url_2("a.b.com/x", RULES_52));
        assert!(!is_simple_url_2("a.comx", RULES_52));
        assert!(!is_simple_url_2("http.com", RULES_52));
        assert!(!is_simple_url_2("a@b.com", RULES_52));
        assert!(is_simple_url_2("a_b.com", RULES_52));
        assert!(!is_simple_url_2("a_b.com", RULES_61));
        assert!(!is_simple_url_2("-a.com", RULES_61));
        assert!(is_simple_url_2("a-b.example.COM/path", RULES_61));
    }

    #[test]
    fn test_email() {
        assert!(is_email_simple("a.b+c@example.co"));
        assert!(!is_email_simple("a..b@example.com"));
        assert!(!is_email_simple("a@example"));
        assert!(!is_email_simple("a@example.c"));
        assert!(is_email_simple("a@example.xn--p1ai"));
        assert!(!is_email_simple("a@-example.com"));
    }

    #[test]
    fn test_smart_urlquote() {
        assert_eq!(smart_urlquote("http://a.com"), "http://a.com");
        assert_eq!(
            smart_urlquote("http://a.com/a b%20c?x=1+2&y&&z=%2525#f g"),
            "http://a.com/a%20b%20c?x=1+2&y=&z=%25#f%20g"
        );
        assert_eq!(smart_urlquote("HTTP://A.com/%ff"), "http://A.com/%EF%BF%BD");
        assert_eq!(smart_urlquote("http://a.com?"), "http://a.com");
    }

    #[test]
    fn test_handle_word() {
        let link = handle_word("x@y.com", false, None, true, RULES_52, &mut no_entities)
            .unwrap()
            .unwrap();
        assert_eq!(link, "<a href=\"mailto:x@y.com\">x@y.com</a>");
        let link = handle_word(
            "www.a.com",
            false,
            Some(5),
            true,
            RULES_52,
            &mut no_entities,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            link,
            "<a href=\"http://www.a.com\" rel=\"nofollow\">www.…</a>"
        );
        let ascii_only = handle_word("été.com", false, None, true, RULES_52, &mut no_entities);
        assert_eq!(ascii_only.unwrap(), None);
    }
}
