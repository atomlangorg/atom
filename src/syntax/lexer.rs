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
// Comments run to the end of the line. The newline is kept, so a comment
// never hides the end of a statement.
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
pub enum Token<'src> {
    /// Produced for any input the lexer does not recognise, so that
    /// parsing can continue and report errors with spans.
    Error,

    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    Int(i64),

    #[token("true", |_| true)]
    #[token("false", |_| false)]
    Bool(bool),

    #[token("let")]
    Let,
    #[token("var")]
    Var,
    #[token("if")]
    If,
    #[token("else")]
    Else,

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
    #[token("!")]
    Bang,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("=")]
    Eq,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,

    /// Statements are terminated by newlines.
    #[token("\n")]
    Newline,
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Error => write!(f, "<error>"),
            Self::Int(n) => write!(f, "{n}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Let => write!(f, "let"),
            Self::Var => write!(f, "var"),
            Self::If => write!(f, "if"),
            Self::Else => write!(f, "else"),
            Self::Ident(name) => write!(f, "{name}"),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::Bang => write!(f, "!"),
            Self::Amp => write!(f, "&"),
            Self::Pipe => write!(f, "|"),
            Self::Eq => write!(f, "="),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::LBrace => write!(f, "{{"),
            Self::RBrace => write!(f, "}}"),
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
            lex("+-*/!&|"),
            vec![
                Token::Plus,
                Token::Minus,
                Token::Star,
                Token::Slash,
                Token::Bang,
                Token::Amp,
                Token::Pipe,
            ]
        );
    }

    #[test]
    fn keywords_are_not_prefixes() {
        assert_eq!(
            lex("let letter _let var variable true truth false falsey"),
            vec![
                Token::Let,
                Token::Ident("letter"),
                Token::Ident("_let"),
                Token::Var,
                Token::Ident("variable"),
                Token::Bool(true),
                Token::Ident("truth"),
                Token::Bool(false),
                Token::Ident("falsey"),
            ]
        );
        assert_eq!(
            lex("if iffy else elsewhere"),
            vec![
                Token::If,
                Token::Ident("iffy"),
                Token::Else,
                Token::Ident("elsewhere"),
            ]
        );
    }

    #[test]
    fn lexes_braces() {
        assert_eq!(
            lex("if x {}"),
            vec![Token::If, Token::Ident("x"), Token::LBrace, Token::RBrace]
        );
    }

    #[test]
    fn unknown_characters_are_errors() {
        assert_eq!(
            lex("1 $ 2"),
            vec![Token::Int(1), Token::Error, Token::Int(2)]
        );
    }

    #[test]
    fn overflowing_integers_are_errors() {
        assert_eq!(lex("99999999999999999999"), vec![Token::Error]);
    }

    #[test]
    fn skips_comments_but_keeps_newlines() {
        assert_eq!(
            lex("x = 1 // set x\n// whole line\ny"),
            vec![
                Token::Ident("x"),
                Token::Eq,
                Token::Int(1),
                Token::Newline,
                Token::Newline,
                Token::Ident("y"),
            ]
        );
    }

    #[test]
    fn comments_can_contain_anything() {
        assert_eq!(lex("// let x = $ { // \"//"), vec![]);
        assert_eq!(lex("1//2"), vec![Token::Int(1)]);
    }

    #[test]
    fn single_slash_is_still_division() {
        assert_eq!(
            lex("4 / 2 / 1"),
            vec![
                Token::Int(4),
                Token::Slash,
                Token::Int(2),
                Token::Slash,
                Token::Int(1),
            ]
        );
    }
}
