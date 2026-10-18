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
    ast::{BinOp, Expr, Mutability, Program, Stmt, UnOp},
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

        let unary = just(Token::Minus)
            .to(UnOp::Neg)
            .repeated()
            .foldr(atom, |op, rhs| Expr::Unary(op, Box::new(rhs)));

        // Binary operators are left-associative: `a - b + c` is `(a - b) + c`.
        let fold_binary = |lhs, (op, rhs)| Expr::Binary(Box::new(lhs), op, Box::new(rhs));

        let product_op = select! {
            Token::Star => BinOp::Mul,
            Token::Slash => BinOp::Div,
        };
        let product = unary
            .clone()
            .foldl(product_op.then(unary).repeated(), fold_binary);

        let sum_op = select! {
            Token::Plus => BinOp::Add,
            Token::Minus => BinOp::Sub,
        };
        product
            .clone()
            .foldl(sum_op.then(product).repeated(), fold_binary)
    });

    let binding = ident.then_ignore(just(Token::Eq)).then(expr);

    let mutability = select! {
        Token::Let => Mutability::Let,
        Token::Var => Mutability::Var,
    };

    let decl = mutability
        .then(binding.clone())
        .map(|(mutability, (name, value))| Stmt::Decl {
            mutability,
            name,
            value,
        });

    let assign = binding.map(|(name, value)| Stmt::Assign { name, value });

    let stmt = decl.or(assign);

    let newlines = just(Token::Newline).repeated();

    // One statement per line; blank lines are allowed anywhere.
    newlines
        .clone()
        .ignore_then(
            stmt.separated_by(newlines.at_least(1))
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
        Stmt::Assign {
            name: name.to_string(),
            value,
        }
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
            Ok(vec![assign("x", Expr::Int(1)), assign("y", Expr::Int(2)),])
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

    fn neg(expr: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::Unary(UnOp::Neg, expr))
    }

    #[test]
    fn parses_negation() {
        assert_eq!(
            parse("x = -5\ny = -y\nz = --(1)"),
            Ok(vec![
                assign("x", *neg(int(5))),
                assign("y", *neg(var("y"))),
                assign("z", *neg(neg(int(1)))),
            ])
        );
    }

    #[test]
    fn subtraction_of_negative() {
        assert_eq!(
            parse("x = 1 - -2"),
            Ok(vec![assign(
                "x",
                Expr::Binary(int(1), BinOp::Sub, neg(int(2)))
            )])
        );
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        assert_eq!(
            parse("x = 1 + 2 * 3 - 4 / 5"),
            Ok(vec![assign(
                "x",
                Expr::Binary(
                    Box::new(Expr::Binary(
                        int(1),
                        BinOp::Add,
                        Box::new(Expr::Binary(int(2), BinOp::Mul, int(3))),
                    )),
                    BinOp::Sub,
                    Box::new(Expr::Binary(int(4), BinOp::Div, int(5))),
                ),
            )])
        );
    }

    #[test]
    fn division_is_left_associative() {
        assert_eq!(
            parse("x = 8 / 4 * 2"),
            Ok(vec![assign(
                "x",
                Expr::Binary(
                    Box::new(Expr::Binary(int(8), BinOp::Div, int(4))),
                    BinOp::Mul,
                    int(2),
                ),
            )])
        );
    }

    #[test]
    fn negation_binds_tighter_than_multiplication() {
        assert_eq!(
            parse("x = -2 * 3"),
            Ok(vec![assign(
                "x",
                Expr::Binary(neg(int(2)), BinOp::Mul, int(3))
            )])
        );
    }

    #[test]
    fn parses_declarations() {
        assert_eq!(
            parse("let x = 1 + 2\nvar y = x\ny = 3\nlet letter = y"),
            Ok(vec![
                Stmt::Decl {
                    mutability: Mutability::Let,
                    name: "x".to_string(),
                    value: Expr::Binary(int(1), BinOp::Add, int(2)),
                },
                Stmt::Decl {
                    mutability: Mutability::Var,
                    name: "y".to_string(),
                    value: *var("x"),
                },
                assign("y", Expr::Int(3)),
                Stmt::Decl {
                    mutability: Mutability::Let,
                    name: "letter".to_string(),
                    value: *var("y"),
                },
            ])
        );
    }

    #[test]
    fn rejects_keywords_as_identifiers() {
        for src in [
            "let = 1",
            "let let = 1",
            "x = let",
            "var = 1",
            "var var = 1",
            "x = var",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_declaration_without_value() {
        assert!(parse("let x").is_err());
        assert!(parse("var x").is_err());
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
