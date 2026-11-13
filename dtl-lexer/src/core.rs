use crate::types::{At, TemplateString};
use crate::{DelimitedToken, END_TAG_LEN, START_TAG_LEN};

enum EndTag {
    Variable,
    Tag,
    Comment,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TokenType {
    Text,
    Variable,
    Tag,
    Comment,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Token {
    pub token_type: TokenType,
    pub at: At,
}

impl Token {
    fn text(at: At) -> Self {
        Self {
            at,
            token_type: TokenType::Text,
        }
    }

    fn variable(at: At) -> Self {
        Self {
            at,
            token_type: TokenType::Variable,
        }
    }

    fn tag(at: At) -> Self {
        Self {
            at,
            token_type: TokenType::Tag,
        }
    }

    fn comment(at: At) -> Self {
        Self {
            at,
            token_type: TokenType::Comment,
        }
    }
}

impl DelimitedToken for Token {
    fn trimmed_at(&self) -> At {
        if matches!(self.token_type, TokenType::Text) {
            self.at
        } else {
            let (start, len) = self.at;
            let start = start + START_TAG_LEN;
            let len = len - START_TAG_LEN - END_TAG_LEN;
            (start, len)
        }
    }
}

impl<'t> Token {
    pub fn content(&self, template: TemplateString<'t>) -> &'t str {
        template.content(self.trimmed_at())
    }
}

pub struct Lexer<'t> {
    template: TemplateString<'t>,
    rest: &'t str,
    byte: usize,
    /// The opening tag of the `verbatim` (or 6.2 `comment`) block being lexed.
    verbatim: Option<&'t str>,
    modern_raw_blocks: bool,
    multiline_tags: bool,
}

impl<'t> Lexer<'t> {
    pub fn new(template: TemplateString<'t>) -> Self {
        Self {
            template,
            rest: template.0,
            byte: 0,
            verbatim: None,
            modern_raw_blocks: false,
            multiline_tags: false,
        }
    }

    /// Django 6.2 lexes `comment` and `verbatim` contents as raw text up to
    /// the closing tag, which may span lines.
    #[must_use]
    pub fn with_modern_raw_blocks(mut self, enabled: bool) -> Self {
        self.modern_raw_blocks = enabled;
        self
    }

    /// Django 6.2 lets `{% ... %}` tags span lines unless the engine sets
    /// the transitional `allow_multiline_tags` option to `False`.
    /// Variables and comments still end at a newline.
    #[must_use]
    pub fn with_multiline_tags(mut self, enabled: bool) -> Self {
        self.multiline_tags = enabled;
        self
    }

    fn lex_text(&mut self) -> Token {
        // Search once for the next opening delimiter. Three independent
        // searches repeatedly scanned the entire remaining source when one
        // delimiter kind (often comments) was absent, making lexing quadratic.
        let mut endings: [Option<Option<usize>>; 3] = [None; 3];
        let mut next_newline = None;
        let next = self.rest.match_indices('{').find_map(|(index, _)| {
            let (kind, end) = match self.rest.as_bytes().get(index + 1) {
                Some(b'{') => (0, "}}"),
                Some(b'%') => (1, "%}"),
                Some(b'#') => (2, "#}"),
                _ => return None,
            };
            // Reuse unsuccessful lookups across literal opening delimiters.
            let closing = match endings[kind] {
                Some(None) => return None,
                Some(Some(closing)) if closing >= index + 2 => closing,
                _ => {
                    let closing = self.rest[index + 2..]
                        .find(end)
                        .map(|offset| index + 2 + offset);
                    endings[kind] = Some(closing);
                    closing?
                }
            };
            // Callers already rejected a token at the start; text is never
            // empty, so lexing always moves forward.
            if index == 0 {
                return None;
            }
            if kind == 1 && self.multiline_tags {
                return Some(index);
            }
            if next_newline.is_none_or(|newline| newline < index) {
                next_newline = self.rest[index..closing]
                    .find('\n')
                    .map(|offset| index + offset);
            }
            next_newline
                .is_none_or(|newline| newline >= closing)
                .then_some(index)
        });
        let len = match next {
            None => {
                let len = self.rest.len();
                self.rest = "";
                len
            }
            Some(n) => {
                self.rest = &self.rest[n..];
                n
            }
        };
        let at = (self.byte, len);
        self.byte += len;
        Token::text(at)
    }

    fn lex_text_to_end(&mut self) -> Token {
        let len = self.rest.len();
        let at = (self.byte, len);
        self.byte += len;
        self.rest = "";
        Token::text(at)
    }

    fn lex_tag(&mut self, end_tag: EndTag) -> Token {
        let end_str = match end_tag {
            EndTag::Variable => "}}",
            EndTag::Tag => "%}",
            EndTag::Comment => "#}",
        };
        // The end marker can't share a character with the start marker:
        // Django's `{%.*?%}` doesn't match `{%}`.
        let Some(n) = self.rest[START_TAG_LEN..]
            .find(end_str)
            .map(|n| n + START_TAG_LEN)
        else {
            return self.lex_text();
        };
        let multiline = matches!(end_tag, EndTag::Tag) && self.multiline_tags;
        if !multiline && self.rest[..n].contains('\n') {
            return self.lex_text();
        }
        let len = n + end_str.len();
        self.rest = &self.rest[len..];

        let at = (self.byte, len);
        self.byte += len;
        match end_tag {
            EndTag::Variable => Token::variable(at),
            EndTag::Tag => Token::tag(at),
            EndTag::Comment => Token::comment(at),
        }
    }

    fn closes_verbatim(&self, inner: &str, verbatim: &str) -> bool {
        if verbatim.split_whitespace().next() == Some("comment") {
            return inner == "endcomment";
        }
        if !self.multiline_tags {
            return inner.strip_prefix("end") == Some(verbatim);
        }
        // Django matches each word of a named `verbatim` with any whitespace
        // between them, so `{% endverbatim a b %}` closes `{% verbatim a  b %}`.
        let Some(mut rest) = inner.strip_prefix("end") else {
            return false;
        };
        for bit in verbatim.split_whitespace() {
            rest = match rest.strip_prefix(bit) {
                Some(rest) => rest.trim_start(),
                None => return false,
            };
        }
        rest.is_empty()
    }

    fn lex_verbatim(&mut self, verbatim: &'t str) -> Token {
        let verbatim = verbatim.trim();

        let mut rest = self.rest;
        let mut index = 0;
        loop {
            let Some(start_tag) = rest.find("{%") else {
                return self.lex_text_to_end();
            };
            rest = &rest[start_tag..];
            let Some(end_tag) = rest.find("%}") else {
                return self.lex_text_to_end();
            };
            let inner = rest[2..end_tag].trim();
            // Django 6.2 allows any whitespace, including newlines, around
            // the closing tag's name. Earlier versions reject newlines.
            let multiline = !self.modern_raw_blocks && rest[..end_tag].contains('\n');
            // Check we have the right endverbatim tag
            if multiline || !self.closes_verbatim(inner, verbatim) {
                let consumed = if self.modern_raw_blocks || multiline {
                    // A literal opener must not hide a later valid closing tag.
                    2
                } else {
                    end_tag + 2
                };
                rest = &rest[consumed..];
                index += start_tag + consumed;
                continue;
            }

            index += start_tag;
            if index == 0 {
                // Return the endverbatim tag since we have no text
                self.verbatim = None;
                let tag_len = end_tag + "%}".len();
                let at = (self.byte, tag_len);
                self.byte += tag_len;
                self.rest = &self.rest[tag_len..];
                return Token::tag(at);
            }
            // Stay in the raw block so the next call returns the closing
            // tag, even one the normal lexer would treat as text.
            self.rest = &self.rest[index..];
            let at = (self.byte, index);
            self.byte += index;
            return Token::text(at);
        }
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        Some(match self.verbatim {
            None => match self.rest.get(..START_TAG_LEN) {
                Some("{{") => self.lex_tag(EndTag::Variable),
                Some("{%") => {
                    let tag = self.lex_tag(EndTag::Tag);
                    if let Token {
                        token_type: TokenType::Tag,
                        ..
                    } = tag
                    {
                        let verbatim = tag.content(self.template).trim();
                        // Django 6.2 also keeps `comment` contents as raw text.
                        let opens_verbatim = if self.modern_raw_blocks {
                            matches!(
                                verbatim.split_whitespace().next(),
                                Some("verbatim" | "comment")
                            )
                        } else {
                            verbatim == "verbatim" || verbatim.starts_with("verbatim ")
                        };
                        if opens_verbatim {
                            self.verbatim = Some(verbatim);
                        }
                    }
                    tag
                }
                Some("{#") => self.lex_tag(EndTag::Comment),
                _ => self.lex_text(),
            },
            Some(verbatim) => self.lex_verbatim(verbatim),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn contents<'t>(template: impl Into<TemplateString<'t>>, tokens: Vec<Token>) -> Vec<&'t str> {
        let template = template.into();
        tokens.iter().map(|t| t.content(template)).collect()
    }

    #[test]
    fn test_lex_empty() {
        let template = "";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![]);
    }

    #[test]
    fn test_opening_delimiters_after_literal_braces_and_unicode() {
        let template = "é {x { {{ value }} { {% if value %} {# comment #} {";
        let tokens: Vec<_> = Lexer::new(template.into()).collect();
        assert_eq!(
            contents(template, tokens),
            vec![
                "é {x { ",
                " value ",
                " { ",
                " if value ",
                " ",
                " comment ",
                " {",
            ]
        );
    }

    #[test]
    fn test_newline_after_delimiter_does_not_make_tag_text() {
        let template = "{{ value }}\n{% if value %}\n{# comment #}\n";
        let tokens: Vec<_> = Lexer::new(template.into()).collect();
        assert_eq!(
            tokens
                .iter()
                .map(|token| &token.token_type)
                .collect::<Vec<_>>(),
            vec![
                &TokenType::Variable,
                &TokenType::Text,
                &TokenType::Tag,
                &TokenType::Text,
                &TokenType::Comment,
                &TokenType::Text,
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![" value ", "\n", " if value ", "\n", " comment ", "\n",]
        );
    }

    #[test]
    fn test_lex_text() {
        let template = "Just some text";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::text((0, 14))]);
        assert_eq!(contents(template, tokens), vec![template]);
    }

    #[test]
    fn test_lex_text_whitespace() {
        let template = "    ";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::text((0, 4))]);
        assert_eq!(contents(template, tokens), vec![template]);
    }

    #[test]
    fn test_lex_comment() {
        let template = "{# comment #}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::comment((0, 13))]);
        assert_eq!(contents(template, tokens), vec![" comment "]);
    }

    #[test]
    fn test_lex_variable() {
        let template = "{{ foo.bar|title }}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::variable((0, 19))]);
        assert_eq!(contents(template, tokens), vec![" foo.bar|title "]);
    }

    #[test]
    fn test_lex_tag() {
        let template = "{% for foo in bar %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::tag((0, 20))]);
        assert_eq!(contents(template, tokens), vec![" for foo in bar "]);
    }

    #[test]
    fn test_lex_incomplete_comment() {
        let template = "{# comment #";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::text((0, 12))]);
        assert_eq!(contents(template, tokens), vec![template]);
    }

    #[test]
    fn test_lex_incomplete_variable() {
        let template = "{{ foo.bar|title }";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::text((0, 18))]);
        assert_eq!(contents(template, tokens), vec![template]);
    }

    #[test]
    fn test_lex_incomplete_tag() {
        let template = "{% for foo in bar %";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::text((0, 19))]);
        assert_eq!(contents(template, tokens), vec![template]);
    }

    #[test]
    fn test_django_example() {
        let template = "text\n{% if test %}{{ varvalue }}{% endif %}{#comment {{not a var}} {%not a block%} #}end text";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::text((0, 5)),
                Token::tag((5, 13)),
                Token::variable((18, 14)),
                Token::tag((32, 11)),
                Token::comment((43, 42)),
                Token::text((85, 8)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![
                "text\n",
                " if test ",
                " varvalue ",
                " endif ",
                "comment {{not a var}} {%not a block%} ",
                "end text",
            ]
        );
    }

    #[test]
    fn test_verbatim_with_variable() {
        let template = "{% verbatim %}{{bare   }}{% endverbatim %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::tag((0, 14)),
                Token::text((14, 11)),
                Token::tag((25, 17)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![" verbatim ", "{{bare   }}", " endverbatim "]
        );
    }

    #[test]
    fn test_verbatim_with_tag() {
        let template = "{% verbatim %}{% endif %}{% endverbatim %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::tag((0, 14)),
                Token::text((14, 11)),
                Token::tag((25, 17)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![" verbatim ", "{% endif %}", " endverbatim "]
        );
    }

    #[test]
    fn test_verbatim_with_verbatim_tag() {
        let template = "{% verbatim %}It's the {% verbatim %} tag{% endverbatim %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::tag((0, 14)),
                Token::text((14, 27)),
                Token::tag((41, 17)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![" verbatim ", "It's the {% verbatim %} tag", " endverbatim "]
        );
    }

    #[test]
    fn test_verbatim_nested() {
        let template = "{% verbatim %}{% verbatim %}{% endverbatim %}{% endverbatim %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::tag((0, 14)),
                Token::text((14, 14)),
                Token::tag((28, 17)),
                Token::tag((45, 17)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![
                " verbatim ",
                "{% verbatim %}",
                " endverbatim ",
                " endverbatim ",
            ]
        );
    }

    #[test]
    fn test_verbatim_adjacent() {
        let template = "{% verbatim %}{% endverbatim %}{% verbatim %}{% endverbatim %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::tag((0, 14)),
                Token::tag((14, 17)),
                Token::tag((31, 14)),
                Token::tag((45, 17)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![" verbatim ", " endverbatim ", " verbatim ", " endverbatim "]
        );
    }

    #[test]
    fn test_verbatim_special() {
        let template =
            "{% verbatim special %}Don't {% endverbatim %} just yet{% endverbatim special %}";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(
            tokens,
            vec![
                Token::tag((0, 22)),
                Token::text((22, 32)),
                Token::tag((54, 25)),
            ]
        );
        assert_eq!(
            contents(template, tokens),
            vec![
                " verbatim special ",
                "Don't {% endverbatim %} just yet",
                " endverbatim special ",
            ]
        );
    }

    #[test]
    fn test_verbatim_open_tag() {
        let template = "{% verbatim %}Don't {% ";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::tag((0, 14)), Token::text((14, 9))]);
        assert_eq!(contents(template, tokens), vec![" verbatim ", "Don't {% "]);
    }

    #[test]
    fn test_verbatim_no_tag() {
        let template = "{% verbatim %}Don't end verbatim";
        let lexer = Lexer::new(template.into());
        let tokens: Vec<_> = lexer.collect();
        assert_eq!(tokens, vec![Token::tag((0, 14)), Token::text((14, 18))]);
        assert_eq!(
            contents(template, tokens),
            vec![" verbatim ", "Don't end verbatim"]
        );
    }

    fn lex_multiline(template: &str, multiline_tags: bool) -> Vec<(TokenType, &str)> {
        Lexer::new(template.into())
            .with_modern_raw_blocks(true)
            .with_multiline_tags(multiline_tags)
            .map(|token| {
                let content = token.content(template.into());
                (token.token_type, content)
            })
            .collect()
    }

    #[test]
    fn test_multiline_block_tag() {
        assert_eq!(
            lex_multiline("a{% if\n x\n%}b{{ y\n }}{# z\n #}", true),
            vec![
                (TokenType::Text, "a"),
                (TokenType::Tag, " if\n x\n"),
                (TokenType::Text, "b{{ y\n }}{# z\n #}"),
            ]
        );
    }

    #[test]
    fn test_multiline_block_tag_disabled() {
        assert_eq!(
            lex_multiline("a{% if\n x\n%}b", false),
            vec![(TokenType::Text, "a{% if\n x\n%}b")]
        );
    }

    #[test]
    fn test_multiline_tag_after_literal_brace() {
        assert_eq!(
            lex_multiline("{{% for\n x in y %}}", true),
            vec![
                (TokenType::Text, "{"),
                (TokenType::Tag, " for\n x in y "),
                (TokenType::Text, "}"),
            ]
        );
    }

    #[test]
    fn test_multiline_verbatim_named_closing_tag() {
        assert_eq!(
            lex_multiline("{% verbatim a  b %}x{%\n endverbatim\n a b\n%}", true),
            vec![
                (TokenType::Tag, " verbatim a  b "),
                (TokenType::Text, "x"),
                (TokenType::Tag, "\n endverbatim\n a b\n"),
            ]
        );
        // Without multiline tags the name must keep its exact spacing.
        assert_eq!(
            lex_multiline(
                "{% verbatim a  b %}{% endverbatim a b %}{% endverbatim a  b %}",
                false
            ),
            vec![
                (TokenType::Tag, " verbatim a  b "),
                (TokenType::Text, "{% endverbatim a b %}"),
                (TokenType::Tag, " endverbatim a  b "),
            ]
        );
    }

    #[test]
    fn test_multiline_raw_closing_tag_without_multiline_tags() {
        assert_eq!(
            lex_multiline("{% verbatim %}x{%\nendverbatim\n%}", false),
            vec![
                (TokenType::Tag, " verbatim "),
                (TokenType::Text, "x"),
                (TokenType::Tag, "\nendverbatim\n"),
            ]
        );
    }

    #[test]
    fn test_raw_comment_block() {
        assert_eq!(
            lex_multiline("{%comment%}{% hidden{%endcomment%}after", true),
            vec![
                (TokenType::Tag, "comment"),
                (TokenType::Text, "{% hidden"),
                (TokenType::Tag, "endcomment"),
                (TokenType::Text, "after"),
            ]
        );
        assert_eq!(
            lex_multiline("{% comment %}{% endcomment x %}{% endcomment %}", true),
            vec![
                (TokenType::Tag, " comment "),
                (TokenType::Text, "{% endcomment x %}"),
                (TokenType::Tag, " endcomment "),
            ]
        );
    }

    #[test]
    fn test_comment_is_not_raw_before_django_6_2() {
        let template = "{% comment %}{% x %}{% endcomment %}";
        let tokens: Vec<_> = Lexer::new(template.into()).collect();
        assert_eq!(tokens.len(), 3);
        assert!(
            tokens
                .iter()
                .all(|token| token.token_type == TokenType::Tag)
        );
    }

    #[test]
    fn test_trimmed_at_doesnt_panic() {
        assert_eq!(Token::text((34, 1)).trimmed_at(), (34, 1));
        assert_eq!(Token::text((34, 20)).trimmed_at(), (34, 20));
        assert_eq!(Token::tag((34, 5)).trimmed_at(), (36, 1));
        assert_eq!(Token::variable((34, 5)).trimmed_at(), (36, 1));
        assert_eq!(Token::comment((34, 5)).trimmed_at(), (36, 1));
    }
}
