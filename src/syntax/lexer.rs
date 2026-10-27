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

use logos::{FilterResult, Lexer, Logos};

#[derive(Logos, Debug, Clone, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\r\f]+")]
// Line comments run to the end of the line. The newline is kept, so a
// comment never hides the end of a statement. Block comments are handled by
// `block_comment`.
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
    #[token("fn")]
    Fn,
    #[token("return")]
    Return,
    #[token("group")]
    Group,
    #[token("use")]
    Use,
    #[token("impl")]
    Impl,
    #[token("static")]
    Static,
    /// The value a method was called on, inside an `impl`.
    #[token("self")]
    SelfValue,

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
    #[token(":")]
    Colon,
    /// Separates a module from a name declared in it, as in `other::f`.
    #[token("::")]
    ColonColon,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token("->")]
    Arrow,
    /// Marks a parameter as having no label. Not a valid identifier.
    #[token("_", priority = 3)]
    Underscore,
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

    /// Statements are terminated by newlines, including those inside a block
    /// comment.
    #[token("\n")]
    #[token("/*", block_comment)]
    Newline,
}

/// Skips the rest of a `/* ... */` comment, whose opening `/*` has just been
/// lexed. Block comments nest, so `/* /* */ */` is a single comment.
///
/// A comment spanning more than one line becomes a newline, so like a line
/// comment, it never hides the end of a statement. A comment that is never
/// closed is an error.
fn block_comment<'src>(lex: &mut Lexer<'src, Token<'src>>) -> FilterResult<(), ()> {
    let rest = lex.remainder().as_bytes();
    let mut depth = 1;
    let mut i = 0;
    while i < rest.len() {
        match &rest[i..] {
            [b'/', b'*', ..] => {
                depth += 1;
                i += 2;
            }
            [b'*', b'/', ..] => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    lex.bump(i);
                    return if lex.slice().contains('\n') {
                        FilterResult::Emit(())
                    } else {
                        FilterResult::Skip
                    };
                }
            }
            _ => i += 1,
        }
    }

    lex.bump(rest.len());
    FilterResult::Error(())
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
            Self::Fn => write!(f, "fn"),
            Self::Return => write!(f, "return"),
            Self::Group => write!(f, "group"),
            Self::Use => write!(f, "use"),
            Self::Impl => write!(f, "impl"),
            Self::Static => write!(f, "static"),
            Self::SelfValue => write!(f, "self"),
            Self::Ident(name) => write!(f, "{name}"),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::Bang => write!(f, "!"),
            Self::Amp => write!(f, "&"),
            Self::Pipe => write!(f, "|"),
            Self::Colon => write!(f, ":"),
            Self::ColonColon => write!(f, "::"),
            Self::Comma => write!(f, ","),
            Self::Dot => write!(f, "."),
            Self::Arrow => write!(f, "->"),
            Self::Underscore => write!(f, "_"),
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
    fn lexes_type_annotation() {
        assert_eq!(
            lex("let x: Int"),
            vec![
                Token::Let,
                Token::Ident("x"),
                Token::Colon,
                Token::Ident("Int"),
            ]
        );
    }

    #[test]
    fn lexes_function() {
        assert_eq!(
            lex("fn add(to value: Int, _ b: Int) -> Int { return value }"),
            vec![
                Token::Fn,
                Token::Ident("add"),
                Token::LParen,
                Token::Ident("to"),
                Token::Ident("value"),
                Token::Colon,
                Token::Ident("Int"),
                Token::Comma,
                Token::Underscore,
                Token::Ident("b"),
                Token::Colon,
                Token::Ident("Int"),
                Token::RParen,
                Token::Arrow,
                Token::Ident("Int"),
                Token::LBrace,
                Token::Return,
                Token::Ident("value"),
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn lexes_group() {
        assert_eq!(
            lex("group User { let age: Int }"),
            vec![
                Token::Group,
                Token::Ident("User"),
                Token::LBrace,
                Token::Let,
                Token::Ident("age"),
                Token::Colon,
                Token::Ident("Int"),
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn lexes_impl() {
        assert_eq!(
            lex("impl User { self.f() } implement selfish"),
            vec![
                Token::Impl,
                Token::Ident("User"),
                Token::LBrace,
                Token::SelfValue,
                Token::Dot,
                Token::Ident("f"),
                Token::LParen,
                Token::RParen,
                Token::RBrace,
                Token::Ident("implement"),
                Token::Ident("selfish"),
            ]
        );
    }

    #[test]
    fn lexes_use_paths() {
        assert_eq!(
            lex("use ./dir/other\nuse /main"),
            vec![
                Token::Use,
                Token::Dot,
                Token::Slash,
                Token::Ident("dir"),
                Token::Slash,
                Token::Ident("other"),
                Token::Newline,
                Token::Use,
                Token::Slash,
                Token::Ident("main"),
            ]
        );
    }

    #[test]
    fn lexes_qualified_names() {
        assert_eq!(
            lex("other::f() /dir/other::Int"),
            vec![
                Token::Ident("other"),
                Token::ColonColon,
                Token::Ident("f"),
                Token::LParen,
                Token::RParen,
                Token::Slash,
                Token::Ident("dir"),
                Token::Slash,
                Token::Ident("other"),
                Token::ColonColon,
                Token::Ident("Int"),
            ]
        );
        // A label followed by a space and a colon is not `::`.
        assert_eq!(
            lex("a: :b::"),
            vec![
                Token::Ident("a"),
                Token::Colon,
                Token::Colon,
                Token::Ident("b"),
                Token::ColonColon,
            ]
        );
    }

    #[test]
    fn underscore_alone_is_not_an_identifier() {
        assert_eq!(
            lex("_ _a a_ __"),
            vec![
                Token::Underscore,
                Token::Ident("_a"),
                Token::Ident("a_"),
                Token::Ident("__"),
            ]
        );
    }

    #[test]
    fn arrow_is_not_minus() {
        assert_eq!(
            lex("a->b a - >"),
            vec![
                Token::Ident("a"),
                Token::Arrow,
                Token::Ident("b"),
                Token::Ident("a"),
                Token::Minus,
                Token::Error,
            ]
        );
        assert_eq!(
            lex("fnord returned grouping user struct"),
            vec![
                Token::Ident("fnord"),
                Token::Ident("returned"),
                Token::Ident("grouping"),
                Token::Ident("user"),
                Token::Ident("struct"),
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
    fn skips_block_comments() {
        assert_eq!(
            lex("x /* a comment */ = /**/ 1 /* // */"),
            vec![Token::Ident("x"), Token::Eq, Token::Int(1)]
        );
        assert_eq!(lex("/* let x = $ { \" */"), vec![]);
    }

    #[test]
    fn block_comments_nest() {
        assert_eq!(lex("/* /* */ */ x"), vec![Token::Ident("x")]);
        assert_eq!(
            lex("1 /* a /* b /* c */ b */ a */ 2"),
            vec![Token::Int(1), Token::Int(2)]
        );
        // Each `/*` needs its own `*/`.
        assert_eq!(lex("/* /* */ x"), vec![Token::Error]);
    }

    #[test]
    fn multi_line_block_comments_are_newlines() {
        assert_eq!(
            lex("x = 1 /* one\ntwo\nthree */ y"),
            vec![
                Token::Ident("x"),
                Token::Eq,
                Token::Int(1),
                Token::Newline,
                Token::Ident("y"),
            ]
        );
    }

    #[test]
    fn unclosed_block_comments_are_errors() {
        assert_eq!(lex("1 /* never closed"), vec![Token::Int(1), Token::Error]);
        assert_eq!(lex("/*/"), vec![Token::Error]);
    }

    #[test]
    fn comment_markers_inside_line_comments_are_ignored() {
        assert_eq!(
            lex("// /* not a block comment\nx"),
            vec![Token::Newline, Token::Ident("x"),]
        );
    }

    #[test]
    fn stray_comment_close_is_not_a_comment() {
        assert_eq!(lex("*/"), vec![Token::Star, Token::Slash]);
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
