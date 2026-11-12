//! The `{% if %}` condition expression and its Pratt parser.
use crate::parse::ParseError;
use crate::parse::Parser;
use crate::parse::TagElement;
use crate::parse::parse_numeric;
use crate::types::Text;
use dtl_lexer::TemplateContent;
use dtl_lexer::tag::TagParts;
use dtl_lexer::tag::ifcondition::IfConditionAtom;
use dtl_lexer::tag::ifcondition::IfConditionLexer;
use dtl_lexer::tag::ifcondition::IfConditionOperator;
use dtl_lexer::tag::ifcondition::IfConditionTokenType;
use dtl_lexer::types::At;
use std::iter::Peekable;

#[derive(Clone, Debug, PartialEq)]
pub enum IfCondition {
    Variable(TagElement),
    And(Box<(Self, Self)>),
    Or(Box<(Self, Self)>),
    Not(Box<Self>),
    Equal(Box<(Self, Self)>),
    NotEqual(Box<(Self, Self)>),
    LessThan(Box<(Self, Self)>),
    GreaterThan(Box<(Self, Self)>),
    LessThanEqual(Box<(Self, Self)>),
    GreaterThanEqual(Box<(Self, Self)>),
    In(Box<(Self, Self)>),
    NotIn(Box<(Self, Self)>),
    Is(Box<(Self, Self)>),
    IsNot(Box<(Self, Self)>),
}

pub(super) fn parse_if_condition(
    parser: &mut Parser,
    parts: TagParts,
    at: At,
) -> Result<IfCondition, ParseError> {
    let mut lexer = IfConditionLexer::new(parser.template, parts)
        .with_modern_syntax(parser.engine.django_series >= (6, 0))
        .peekable();
    if lexer.peek().is_none() {
        return Err(ParseError::MissingBooleanExpression { at: at.into() });
    }
    parse_if_binding_power(parser, &mut lexer, 0, at)
}

fn parse_if_binding_power(
    parser: &mut Parser,
    lexer: &mut Peekable<IfConditionLexer>,
    min_binding_power: u8,
    at: At,
) -> Result<IfCondition, ParseError> {
    let Some(token) = lexer.next().transpose()? else {
        return Err(ParseError::UnexpectedEndExpression { at: at.into() });
    };
    let content = token.content(parser.template);
    let token_at = token.content_at();
    let mut lhs = match token.token_type {
        IfConditionTokenType::Atom(IfConditionAtom::Numeric) => {
            IfCondition::Variable(parse_numeric(content, token_at)?)
        }
        IfConditionTokenType::Atom(IfConditionAtom::Text) => {
            IfCondition::Variable(TagElement::Text(Text::new(token_at)))
        }
        IfConditionTokenType::Atom(IfConditionAtom::TranslatedText) => {
            IfCondition::Variable(TagElement::TranslatedText(Text::new(token_at)))
        }
        IfConditionTokenType::Atom(IfConditionAtom::Variable) => {
            IfCondition::Variable(parser.parse_filter_expression(content, token_at, token.at.0)?)
        }
        IfConditionTokenType::Not => {
            let if_condition = parse_if_binding_power(parser, lexer, NOT_BINDING_POWER, token_at)?;
            IfCondition::Not(Box::new(if_condition))
        }
        _ => {
            return Err(ParseError::InvalidIfPosition {
                at: token.at.into(),
                token: content.to_string(),
            });
        }
    };

    loop {
        let token = match lexer.peek() {
            None => break,
            Some(Err(e)) => return Err(e.clone().into()),
            Some(Ok(token)) => token,
        };
        let operator = match &token.token_type {
            IfConditionTokenType::Atom(_) | IfConditionTokenType::Not => {
                return Err(ParseError::UnusedExpression {
                    at: token.at.into(),
                    expression: parser.template.content(token.at).to_string(),
                });
            }
            IfConditionTokenType::Operator(operator) => *operator,
        };
        let binding_power = operator.binding_power();
        if binding_power <= min_binding_power {
            break;
        }

        // We can get the next token properly now, since we have the right binding
        // power and don't need to `break`.
        let token = lexer
            .next()
            .expect("already `break`ed in match peek()")
            .expect("already `return Err` in match peek()");
        let rhs = parse_if_binding_power(parser, lexer, binding_power, token.at)?;

        lhs = operator.build_condition(lhs, rhs);
    }

    Ok(lhs)
}

const NOT_BINDING_POWER: u8 = 8;

trait IfConditionOperatorMethods {
    fn binding_power(&self) -> u8;
    fn build_condition(&self, lhs: IfCondition, rhs: IfCondition) -> IfCondition;
}

impl IfConditionOperatorMethods for IfConditionOperator {
    fn binding_power(&self) -> u8 {
        match self {
            Self::Or => 6,
            Self::And => 7,
            Self::In | Self::NotIn => 9,
            Self::Is
            | Self::IsNot
            | Self::Equal
            | Self::NotEqual
            | Self::GreaterThan
            | Self::GreaterThanEqual
            | Self::LessThan
            | Self::LessThanEqual => 10,
        }
    }

    fn build_condition(&self, lhs: IfCondition, rhs: IfCondition) -> IfCondition {
        let inner = Box::new((lhs, rhs));
        match self {
            Self::And => IfCondition::And(inner),
            Self::Or => IfCondition::Or(inner),
            Self::In => IfCondition::In(inner),
            Self::NotIn => IfCondition::NotIn(inner),
            Self::Is => IfCondition::Is(inner),
            Self::IsNot => IfCondition::IsNot(inner),
            Self::Equal => IfCondition::Equal(inner),
            Self::NotEqual => IfCondition::NotEqual(inner),
            Self::GreaterThan => IfCondition::GreaterThan(inner),
            Self::GreaterThanEqual => IfCondition::GreaterThanEqual(inner),
            Self::LessThan => IfCondition::LessThan(inner),
            Self::LessThanEqual => IfCondition::LessThanEqual(inner),
        }
    }
}
