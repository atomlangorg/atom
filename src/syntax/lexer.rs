/*
 * Copyright 2026 George Elsham
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::fmt;

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\r\f]+")]
pub enum Token<'src> {
    /// Produced for any input the lexer does not recognise, so that
    /// parsing can continue and report errors with spans.
    Error,

    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    Int(i64),

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident(&'src str),

    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("=")]
    Eq,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,

    /// Statements are terminated by newlines.
    #[token("\n")]
    Newline,
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Error => write!(f, "<error>"),
            Self::Int(n) => write!(f, "{n}"),
            Self::Ident(name) => write!(f, "{name}"),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::Eq => write!(f, "="),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::Newline => write!(f, "<newline>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<Token<'_>> {
        Token::lexer(src)
            .map(|tok| tok.unwrap_or(Token::Error))
            .collect()
    }

    #[test]
    fn lexes_assignment() {
        assert_eq!(
            lex("x = 1 + 23 - y\n"),
            vec![
                Token::Ident("x"),
                Token::Eq,
                Token::Int(1),
                Token::Plus,
                Token::Int(23),
                Token::Minus,
                Token::Ident("y"),
                Token::Newline,
            ]
        );
    }

    #[test]
    fn lexes_operators() {
        assert_eq!(
            lex("+-*/"),
            vec![Token::Plus, Token::Minus, Token::Star, Token::Slash]
        );
    }

    #[test]
    fn unknown_characters_are_errors() {
        assert_eq!(lex("1 $ 2"), vec![Token::Int(1), Token::Error, Token::Int(2)]);
    }

    #[test]
    fn overflowing_integers_are_errors() {
        assert_eq!(lex("99999999999999999999"), vec![Token::Error]);
    }
}
