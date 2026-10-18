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
        let literal = select! {
            Token::Int(n) => Expr::Int(n),
            Token::Bool(b) => Expr::Bool(b),
        };

        let atom = literal
            .or(ident.map(Expr::Var))
            .or(expr.delimited_by(just(Token::LParen), just(Token::RParen)))
            .labelled("expression");

        let unary_op = select! {
            Token::Minus => UnOp::Neg,
            Token::Bang => UnOp::Not,
        };
        let unary = unary_op
            .repeated()
            .foldr(atom, |op, rhs| Expr::Unary(op, Box::new(rhs)));

        // Binary operators are left-associative: `a - b + c` is `(a - b) + c`.
        // Each level below binds less tightly than the one before it.
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
        let sum = product
            .clone()
            .foldl(sum_op.then(product).repeated(), fold_binary);

        let and_op = just(Token::Amp).to(BinOp::And);
        let and = sum.clone().foldl(and_op.then(sum).repeated(), fold_binary);

        let or_op = just(Token::Pipe).to(BinOp::Or);
        and.clone().foldl(or_op.then(and).repeated(), fold_binary)
    });

    let binding = ident.then_ignore(just(Token::Eq)).then(expr.clone());

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

    let newlines = just(Token::Newline).repeated();

    // One statement per line; blank lines are allowed anywhere.
    let stmts = recursive(|stmts| {
        let block = stmts.delimited_by(just(Token::LBrace), just(Token::RBrace));

        // The `{` must be on the same line as the `if`, and `else` on the same
        // line as the preceding `}`, so newlines never need disambiguating.
        let if_ = recursive(|if_| {
            let else_block =
                just(Token::Else).ignore_then(block.clone().or(if_.map(|stmt| vec![stmt])));

            just(Token::If)
                .ignore_then(expr.clone())
                .then(block.clone())
                .then(else_block.or_not())
                .map(|((cond, then_block), else_block)| Stmt::If {
                    cond,
                    then_block,
                    else_block,
                })
        });

        let stmt = decl.clone().or(assign.clone()).or(if_);

        newlines.clone().ignore_then(
            stmt.separated_by(newlines.clone().at_least(1))
                .allow_trailing()
                .collect(),
        )
    });

    stmts.then_ignore(end())
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
            "true = 1",
            "let false = 1",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_boolean_literals() {
        assert_eq!(
            parse("x = true\ny = false"),
            Ok(vec![
                assign("x", Expr::Bool(true)),
                assign("y", Expr::Bool(false)),
            ])
        );
    }

    #[test]
    fn parses_ill_typed_expressions() {
        // Type errors are caught by the checker, not the parser.
        assert_eq!(
            parse("x = -true + 1"),
            Ok(vec![assign(
                "x",
                Expr::Binary(neg(Box::new(Expr::Bool(true))), BinOp::Add, int(1)),
            )])
        );
    }

    #[test]
    fn rejects_declaration_without_value() {
        assert!(parse("let x").is_err());
        assert!(parse("var x").is_err());
    }

    fn bin(lhs: Box<Expr>, op: BinOp, rhs: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::Binary(lhs, op, rhs))
    }

    fn not(expr: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::Unary(UnOp::Not, expr))
    }

    fn parse_expr(src: &str) -> Expr {
        match parse(&format!("x = {src}")).unwrap().remove(0) {
            Stmt::Assign { value, .. } => value,
            stmt => panic!("expected assignment, got {stmt:?}"),
        }
    }

    #[test]
    fn parses_not() {
        assert_eq!(parse_expr("!a"), *not(var("a")));
        assert_eq!(parse_expr("!!a"), *not(not(var("a"))));
        assert_eq!(parse_expr("!-a"), *not(neg(var("a"))));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        assert_eq!(
            parse_expr("a | b & c"),
            *bin(var("a"), BinOp::Or, bin(var("b"), BinOp::And, var("c")))
        );
        assert_eq!(
            parse_expr("a & b | c"),
            *bin(bin(var("a"), BinOp::And, var("b")), BinOp::Or, var("c"))
        );
    }

    #[test]
    fn boolean_operators_are_left_associative() {
        assert_eq!(
            parse_expr("a & b & c"),
            *bin(bin(var("a"), BinOp::And, var("b")), BinOp::And, var("c"))
        );
        assert_eq!(
            parse_expr("a | b | c"),
            *bin(bin(var("a"), BinOp::Or, var("b")), BinOp::Or, var("c"))
        );
    }

    #[test]
    fn not_binds_tightest() {
        assert_eq!(
            parse_expr("!a & b"),
            *bin(not(var("a")), BinOp::And, var("b"))
        );
        assert_eq!(
            parse_expr("!(a & b)"),
            *not(bin(var("a"), BinOp::And, var("b")))
        );
    }

    #[test]
    fn arithmetic_binds_tighter_than_boolean_operators() {
        assert_eq!(
            parse_expr("a + b & c * d"),
            *bin(
                bin(var("a"), BinOp::Add, var("b")),
                BinOp::And,
                bin(var("c"), BinOp::Mul, var("d")),
            )
        );
    }

    #[test]
    fn rejects_doubled_operators() {
        assert!(parse("x = a && b").is_err());
        assert!(parse("x = a || b").is_err());
        assert!(parse("x = a !").is_err());
    }

    fn if_(cond: Expr, then_block: Vec<Stmt>, else_block: Option<Vec<Stmt>>) -> Stmt {
        Stmt::If {
            cond,
            then_block,
            else_block,
        }
    }

    #[test]
    fn parses_if() {
        assert_eq!(
            parse(
                "
                if c {
                    x = 1
                    y = 2
                }
                "
            ),
            Ok(vec![if_(
                *var("c"),
                vec![assign("x", Expr::Int(1)), assign("y", Expr::Int(2))],
                None,
            )])
        );
    }

    #[test]
    fn parses_if_else() {
        assert_eq!(
            parse(
                "
                if c {
                    x = 1
                } else {
                    x = 2
                }
                "
            ),
            Ok(vec![if_(
                *var("c"),
                vec![assign("x", Expr::Int(1))],
                Some(vec![assign("x", Expr::Int(2))]),
            )])
        );
    }

    #[test]
    fn parses_else_if_chain() {
        assert_eq!(
            parse(
                "
                if a {
                    x = 1
                } else if b {
                    x = 2
                } else if c {
                    x = 3
                } else {
                    x = 4
                }
                "
            ),
            Ok(vec![if_(
                *var("a"),
                vec![assign("x", Expr::Int(1))],
                Some(vec![if_(
                    *var("b"),
                    vec![assign("x", Expr::Int(2))],
                    Some(vec![if_(
                        *var("c"),
                        vec![assign("x", Expr::Int(3))],
                        Some(vec![assign("x", Expr::Int(4))]),
                    )]),
                )]),
            )])
        );
    }

    #[test]
    fn parses_condition_expression() {
        assert_eq!(
            parse("if !a & b {}"),
            Ok(vec![if_(
                *bin(not(var("a")), BinOp::And, var("b")),
                vec![],
                None,
            )])
        );
    }

    #[test]
    fn parses_compact_and_empty_blocks() {
        assert_eq!(
            parse("if c { x = 1 } else {}"),
            Ok(vec![if_(
                *var("c"),
                vec![assign("x", Expr::Int(1))],
                Some(vec![]),
            )])
        );
        assert_eq!(parse("if c {\n\n}"), Ok(vec![if_(*var("c"), vec![], None)]));
    }

    #[test]
    fn parses_nested_if_and_surrounding_statements() {
        assert_eq!(
            parse(
                "
                let a = true
                if a {
                    if a {
                        let b = 1
                    }
                }

                x = 2
                "
            ),
            Ok(vec![
                Stmt::Decl {
                    mutability: Mutability::Let,
                    name: "a".to_string(),
                    value: Expr::Bool(true),
                },
                if_(
                    *var("a"),
                    vec![if_(
                        *var("a"),
                        vec![Stmt::Decl {
                            mutability: Mutability::Let,
                            name: "b".to_string(),
                            value: Expr::Int(1),
                        }],
                        None,
                    )],
                    None,
                ),
                assign("x", Expr::Int(2)),
            ])
        );
    }

    #[test]
    fn rejects_malformed_if() {
        for src in [
            // Missing braces or condition.
            "if c x = 1",
            "if {}",
            "if c {",
            "if c }",
            // `{` must be on the same line as `if`.
            "if c\n{}",
            // `else` must be on the same line as `}`.
            "if c {}\nelse {}",
            // `else` needs a block or another `if`.
            "if c {} else",
            "if c {} else x = 1",
            "else {}",
            // Statements after `{` or `}` need their own line.
            "if c {} x = 1",
            "if c { x = 1 y = 2 }",
            // `if` is a keyword.
            "if = 1",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
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
