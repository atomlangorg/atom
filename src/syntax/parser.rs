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
    ast::{Arg, BinOp, Expr, Mutability, Param, Program, Stmt, UnOp},
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

        // `label: value` or just `value`.
        let arg = ident
            .then_ignore(just(Token::Colon))
            .or_not()
            .then(expr.clone())
            .map(|(label, value)| Arg { label, value });
        let args = arg
            .separated_by(just(Token::Comma))
            .collect()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        // A name followed by arguments is a call, otherwise it's a variable.
        let name_or_call = ident.then(args.or_not()).map(|(name, args)| match args {
            Some(args) => Expr::Call { callee: name, args },
            None => Expr::Var(name),
        });

        let atom = literal
            .or(name_or_call)
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

    let mutability = select! {
        Token::Let => Mutability::Let,
        Token::Var => Mutability::Var,
    };

    // Type names are resolved by the checker, not reserved by the lexer.
    let type_name = select! { Token::Ident(name) => name.to_string() }.labelled("type");
    let annotation = just(Token::Colon).ignore_then(type_name);

    let decl = mutability
        .then(ident)
        .then(annotation.or_not())
        .then_ignore(just(Token::Eq))
        .then(expr.clone())
        .map(|(((mutability, name), ty), value)| Stmt::Decl {
            mutability,
            name,
            ty,
            value,
        });

    let assign = ident
        .then_ignore(just(Token::Eq))
        .then(expr.clone())
        .map(|(name, value)| Stmt::Assign { name, value });

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

        // `label name: Type`, `_ name: Type`, or `name: Type`, where the name
        // is also the label.
        let param = just(Token::Underscore)
            .ignore_then(ident)
            .map(|name| (None, name))
            .or(ident.then(ident).map(|(label, name)| (Some(label), name)))
            .or(ident.map(|name| (Some(name.clone()), name)))
            .then_ignore(just(Token::Colon))
            .then(type_name)
            .map(|((label, name), ty)| Param { label, name, ty });
        let params = param
            .separated_by(just(Token::Comma))
            .collect()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        let fn_ = just(Token::Fn)
            .ignore_then(ident)
            .then(params)
            .then(
                just(Token::Arrow)
                    .ignore_then(type_name)
                    .or_not()
                    .map(|ret| ret.unwrap_or_else(|| "Void".to_string())),
            )
            .then(block.clone())
            .map(|(((name, params), ret), body)| Stmt::Fn {
                name,
                params,
                ret,
                body,
            });

        let return_ = just(Token::Return)
            .ignore_then(expr.clone().or_not())
            .map(Stmt::Return);

        // Only calls are useful on their own, since any other expression's
        // value would be thrown away.
        let call = expr.clone().try_map(|expr, span| match expr {
            Expr::Call { .. } => Ok(Stmt::Expr(expr)),
            _ => Err(Rich::custom(
                span,
                "only function calls can be used as statements",
            )),
        });

        let stmt = decl
            .clone()
            .or(assign.clone())
            .or(if_)
            .or(fn_)
            .or(return_)
            .or(call);

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
        assert_eq!(
            parse(
                "



            "
            ),
            Ok(vec![])
        );
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
                    ty: None,
                    value: Expr::Binary(int(1), BinOp::Add, int(2)),
                },
                Stmt::Decl {
                    mutability: Mutability::Var,
                    name: "y".to_string(),
                    ty: None,
                    value: *var("x"),
                },
                assign("y", Expr::Int(3)),
                Stmt::Decl {
                    mutability: Mutability::Let,
                    name: "letter".to_string(),
                    ty: None,
                    value: *var("y"),
                },
            ])
        );
    }

    fn decl(mutability: Mutability, name: &str, ty: Option<&str>, value: Expr) -> Stmt {
        Stmt::Decl {
            mutability,
            name: name.to_string(),
            ty: ty.map(str::to_string),
            value,
        }
    }

    #[test]
    fn parses_explicit_types() {
        assert_eq!(
            parse(
                "
                let x: Int = 3
                var y: Bool = true
                let z = 1
                let w : Int=-x
                "
            ),
            Ok(vec![
                decl(Mutability::Let, "x", Some("Int"), Expr::Int(3)),
                decl(Mutability::Var, "y", Some("Bool"), Expr::Bool(true)),
                decl(Mutability::Let, "z", None, Expr::Int(1)),
                decl(Mutability::Let, "w", Some("Int"), *neg(var("x"))),
            ])
        );
    }

    #[test]
    fn parses_unknown_type_names() {
        // Whether a type exists is decided by the checker.
        assert_eq!(
            parse("let x: Foo = 1"),
            Ok(vec![decl(Mutability::Let, "x", Some("Foo"), Expr::Int(1))])
        );
    }

    #[test]
    fn rejects_malformed_explicit_types() {
        for src in [
            "let x: = 3",
            "let x Int = 3",
            "let x: Int",
            "let x: 3 = 3",
            "let x: Int Bool = 3",
            "let x: true = 3",
            // Only declarations can have an explicit type.
            "x: Int = 3",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
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
                    ty: None,
                    value: Expr::Bool(true),
                },
                if_(
                    *var("a"),
                    vec![if_(
                        *var("a"),
                        vec![Stmt::Decl {
                            mutability: Mutability::Let,
                            name: "b".to_string(),
                            ty: None,
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
    fn parses_comments() {
        assert_eq!(
            parse(
                "
                // A comment on its own line.
                let a = true // A comment after code.
                    // An indented comment.

                if a { // After an opening brace.
                    // Inside a block.
                    x = 1 // Inside a block, after code.
                } else { // After `else`.
                    x = 2
                } // After a closing brace.
                // At the end, without a trailing newline."
            ),
            Ok(vec![
                Stmt::Decl {
                    mutability: Mutability::Let,
                    name: "a".to_string(),
                    ty: None,
                    value: Expr::Bool(true),
                },
                if_(
                    *var("a"),
                    vec![assign("x", Expr::Int(1))],
                    Some(vec![assign("x", Expr::Int(2))]),
                ),
            ])
        );
    }

    #[test]
    fn parses_program_of_only_comments() {
        assert_eq!(parse("// one\n// two"), Ok(vec![]));
        assert_eq!(
            parse(
                "
                if c {
                    // nothing yet
                }
                "
            ),
            Ok(vec![if_(*var("c"), vec![], None)])
        );
    }

    #[test]
    fn comments_hide_the_rest_of_the_line() {
        // Everything after `//` is ignored, including what would be an error.
        assert_eq!(
            parse("x = 1 // y = 2 $ {"),
            Ok(vec![assign("x", Expr::Int(1))])
        );
        // A comment swallows the `{`, so the block is never opened.
        assert!(parse("if c // {\n}").is_err());
        // `else` must still be on the same line as `}`.
        assert!(parse("if c {} // comment\nelse {}").is_err());
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

    fn param(label: Option<&str>, name: &str, ty: &str) -> Param {
        Param {
            label: label.map(str::to_string),
            name: name.to_string(),
            ty: ty.to_string(),
        }
    }

    fn arg(label: Option<&str>, value: Expr) -> Arg {
        Arg {
            label: label.map(str::to_string),
            value,
        }
    }

    fn call(callee: &str, args: Vec<Arg>) -> Box<Expr> {
        Box::new(Expr::Call {
            callee: callee.to_string(),
            args,
        })
    }

    #[test]
    fn parses_function() {
        assert_eq!(
            parse(
                "
                fn addFive(to value: Int) -> Int {
                    return value + 5
                }
                "
            ),
            Ok(vec![Stmt::Fn {
                name: "addFive".to_string(),
                params: vec![param(Some("to"), "value", "Int")],
                ret: "Int".to_string(),
                body: vec![Stmt::Return(Some(Expr::Binary(
                    var("value"),
                    BinOp::Add,
                    int(5),
                )))],
            }])
        );
    }

    #[test]
    fn parses_parameter_label_forms() {
        assert_eq!(
            parse("fn f(a: Int, to b: Bool, _ c: Int) {}"),
            Ok(vec![Stmt::Fn {
                name: "f".to_string(),
                params: vec![
                    // A single name is both the label and the parameter name.
                    param(Some("a"), "a", "Int"),
                    param(Some("to"), "b", "Bool"),
                    // `_` means no label.
                    param(None, "c", "Int"),
                ],
                ret: "Void".to_string(),
                body: vec![],
            }])
        );
    }

    #[test]
    fn parses_function_without_parameters_or_return_type() {
        assert_eq!(
            parse(
                "
                fn reset() {
                    count = 0
                    return
                }
                "
            ),
            Ok(vec![Stmt::Fn {
                name: "reset".to_string(),
                params: vec![],
                ret: "Void".to_string(),
                body: vec![assign("count", Expr::Int(0)), Stmt::Return(None)],
            }])
        );
    }

    #[test]
    fn missing_return_type_is_void() {
        assert_eq!(parse("fn f() {}"), parse("fn f() -> Void {}"));
    }

    #[test]
    fn parses_calls() {
        assert_eq!(parse_expr("f()"), *call("f", vec![]));
        assert_eq!(
            parse_expr("addFive(to: 3)"),
            *call("addFive", vec![arg(Some("to"), *int(3))])
        );
        assert_eq!(
            parse_expr("f(a: 1 + 2, b, c: g(x))"),
            *call(
                "f",
                vec![
                    arg(Some("a"), *bin(int(1), BinOp::Add, int(2))),
                    arg(None, *var("b")),
                    arg(Some("c"), *call("g", vec![arg(None, *var("x"))])),
                ]
            )
        );
    }

    #[test]
    fn calls_bind_tightest() {
        assert_eq!(
            parse_expr("-f(x: 1) * 2"),
            *bin(
                neg(call("f", vec![arg(Some("x"), *int(1))])),
                BinOp::Mul,
                int(2)
            )
        );
    }

    #[test]
    fn parses_call_statement() {
        assert_eq!(
            parse("log(message: 1)"),
            Ok(vec![Stmt::Expr(*call(
                "log",
                vec![arg(Some("message"), *int(1))]
            ))])
        );
    }

    #[test]
    fn rejects_non_call_expression_statements() {
        for src in ["x", "1 + 2", "-f()", "!f() & g()"] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
        let errs = parse("1 + 2").unwrap_err();
        assert_eq!(
            errs[0].to_string(),
            "only function calls can be used as statements"
        );
    }

    #[test]
    fn parses_return_forms() {
        assert_eq!(parse("return"), Ok(vec![Stmt::Return(None)]));
        assert_eq!(
            parse("return 1 + 2"),
            Ok(vec![Stmt::Return(Some(*bin(int(1), BinOp::Add, int(2))))])
        );
    }

    #[test]
    fn rejects_malformed_functions() {
        for src in [
            // Missing parts.
            "fn {}",
            "fn f {}",
            "fn f()",
            "fn f() ->  {}",
            "fn f(x) {}",
            "fn f(x:) {}",
            // Bad parameter labels.
            "fn f(a b c: Int) {}",
            "fn f(_: Int) {}",
            "fn f(_ _ x: Int) {}",
            "fn f(x _: Int) {}",
            // Separators.
            "fn f(a: Int b: Int) {}",
            "fn f(a: Int,) {}",
            // `{` must be on the same line.
            "fn f()\n{}",
            // Keywords.
            "fn fn() {}",
            "fn return() {}",
            "let fn = 1",
            "let _ = 1",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_malformed_calls() {
        for src in [
            "x = f(",
            "x = f(a b)",
            "x = f(a:)",
            "x = f(_: 1)",
            "x = f(1,)",
            "x = f(a: b: 1)",
            "x = 1(2)",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }
}
