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

use chumsky::{
    input::{Stream, ValueInput},
    prelude::*,
};
use logos::Logos;

use super::{
    ast::{BinOp, Expr, Program, Stmt},
    lexer::Token,
};

pub type ParseError<'src> = Rich<'src, Token<'src>>;

/// Lexes and parses `src` into a program.
pub fn parse(src: &str) -> Result<Program, Vec<ParseError<'_>>> {
    let tokens = Token::lexer(src).spanned().map(|(tok, span)| match tok {
        Ok(tok) => (tok, span.into()),
        Err(()) => (Token::Error, span.into()),
    });
    let stream = Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(t, s)| (t, s));

    program_parser().parse(stream).into_result()
}

fn program_parser<'tok, 'src: 'tok, I>()
-> impl Parser<'tok, I, Program, extra::Err<Rich<'tok, Token<'src>>>>
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    let ident = select! { Token::Ident(name) => name.to_string() }.labelled("identifier");

    let expr = recursive(|expr| {
        let atom = select! { Token::Int(n) => Expr::Int(n) }
            .or(ident.map(Expr::Var))
            .or(expr.delimited_by(just(Token::LParen), just(Token::RParen)))
            .labelled("expression");

        let op = select! {
            Token::Plus => BinOp::Add,
            Token::Minus => BinOp::Sub,
        };

        // Left-associative: `a - b + c` is `(a - b) + c`.
        atom.clone().foldl(op.then(atom).repeated(), |lhs, (op, rhs)| {
            Expr::Binary(Box::new(lhs), op, Box::new(rhs))
        })
    });

    let assign = ident
        .then_ignore(just(Token::Eq))
        .then(expr)
        .map(|(name, value)| Stmt::Assign { name, value });

    let newlines = just(Token::Newline).repeated();

    // One statement per line; blank lines are allowed anywhere.
    newlines
        .clone()
        .ignore_then(
            assign
                .separated_by(newlines.at_least(1))
                .allow_trailing()
                .collect(),
        )
        .then_ignore(end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(n: i64) -> Box<Expr> {
        Box::new(Expr::Int(n))
    }

    fn var(name: &str) -> Box<Expr> {
        Box::new(Expr::Var(name.to_string()))
    }

    fn assign(name: &str, value: Expr) -> Stmt {
        Stmt::Assign { name: name.to_string(), value }
    }

    #[test]
    fn parses_empty_program() {
        assert_eq!(parse(""), Ok(vec![]));
        assert_eq!(parse("  \n\n  \n"), Ok(vec![]));
    }

    #[test]
    fn parses_assignments() {
        assert_eq!(
            parse("x = 1 + 2\ny = x - 3"),
            Ok(vec![
                assign("x", Expr::Binary(int(1), BinOp::Add, int(2))),
                assign("y", Expr::Binary(var("x"), BinOp::Sub, int(3))),
            ])
        );
    }

    #[test]
    fn allows_blank_lines() {
        assert_eq!(
            parse("\n\nx = 1\n\n\ny = 2\n\n"),
            Ok(vec![
                assign("x", Expr::Int(1)),
                assign("y", Expr::Int(2)),
            ])
        );
    }

    #[test]
    fn binary_ops_are_left_associative() {
        assert_eq!(
            parse("x = 1 - 2 + 3"),
            Ok(vec![assign(
                "x",
                Expr::Binary(
                    Box::new(Expr::Binary(int(1), BinOp::Sub, int(2))),
                    BinOp::Add,
                    int(3),
                ),
            )])
        );
    }

    #[test]
    fn parentheses_group() {
        assert_eq!(
            parse("x = 1 - (2 + 3)"),
            Ok(vec![assign(
                "x",
                Expr::Binary(
                    int(1),
                    BinOp::Sub,
                    Box::new(Expr::Binary(int(2), BinOp::Add, int(3))),
                ),
            )])
        );
    }

    #[test]
    fn rejects_two_statements_on_one_line() {
        assert!(parse("x = 1 y = 2").is_err());
    }

    #[test]
    fn rejects_semicolons() {
        assert!(parse("x = 1;").is_err());
    }

    #[test]
    fn rejects_invalid_token() {
        let errs = parse("x = 1 $ 2").unwrap_err();
        assert_eq!(errs[0].span().into_range(), 6..7);
    }
}
