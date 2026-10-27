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

use chumsky::{
    input::{Stream, ValueInput},
    inspector::TruncateState,
    prelude::*,
};
use logos::Logos;

use super::{
    ast::{
        Arg, BinOp, Expr, Field, FieldValue, Function, ImplFunction, ModulePath, Mutability, Name,
        Param, Program, Stmt, UnOp,
    },
    lexer::Token,
};

pub type ParseError<'src> = Rich<'src, Token<'src>>;

/// A problem that doesn't stop a program from compiling, such as a matter of
/// style. Unlike errors, warnings never stop parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub span: SimpleSpan,
    pub kind: WarningKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    /// A list whose closing bracket is on the same line as its last item ends
    /// with a comma. The span is the comma.
    RedundantTrailingComma,
    /// A list whose closing bracket is on its own line doesn't end with a
    /// comma. The span is empty, just after the last item.
    MissingTrailingComma,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.kind {
            WarningKind::RedundantTrailingComma => {
                write!(f, "redundant trailing comma in a list on one line")
            }
            WarningKind::MissingTrailingComma => {
                write!(f, "missing trailing comma in a list over several lines")
            }
        }
    }
}

/// Parser state that collects warnings. Warnings from parsers that are
/// backtracked out of are dropped.
type Extra<'tok, 'src> = extra::Full<Rich<'tok, Token<'src>>, TruncateState<Warning>, ()>;

/// Lexes and parses `src` into a program.
pub fn parse(src: &str) -> Result<Program, Vec<ParseError<'_>>> {
    parse_with_warnings(src).0
}

/// Lexes and parses `src` into a program, also returning any warnings,
/// which are found even if parsing fails.
pub fn parse_with_warnings(src: &str) -> (Result<Program, Vec<ParseError<'_>>>, Vec<Warning>) {
    let tokens = Token::lexer(src).spanned().map(|(tok, span)| match tok {
        Ok(tok) => (tok, span.into()),
        Err(()) => (Token::Error, span.into()),
    });
    let stream = Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(t, s)| (t, s));

    let mut warnings = TruncateState(vec![]);
    let result = program_parser()
        .parse_with_state(stream, &mut warnings)
        .into_result();
    (result, warnings.0)
}

/// A comma-separated list of `item`s between `open` and `close`, as in a
/// call, constructor or function's parameters. It can be split over several
/// lines after `open`, after each comma, and before `close`.
///
/// A list whose `close` is on its own line should end with a comma, so that
/// adding an item only changes one line. Otherwise it shouldn't. Either way,
/// this is a warning rather than an error.
fn comma_list<'tok, 'src: 'tok, I, T>(
    item: impl Parser<'tok, I, T, Extra<'tok, 'src>> + Clone,
    open: Token<'src>,
    close: Token<'src>,
) -> impl Parser<'tok, I, Vec<T>, Extra<'tok, 'src>> + Clone
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    let line_breaks = just(Token::Newline).repeated();
    let item = item.map_with(|item, e| (item, e.span()));
    let trailing_comma = just(Token::Comma).map_with(|_, e| e.span());

    just(open)
        .then(line_breaks.clone())
        .ignore_then(
            item.separated_by(just(Token::Comma).then(line_breaks.clone()))
                .collect::<Vec<_>>(),
        )
        .then(trailing_comma.or_not())
        .then(line_breaks.count())
        .then_ignore(just(close))
        .validate(|((items, comma), newlines_before_close), e, emitter| {
            let close_on_own_line = newlines_before_close > 0;
            let last = items.last().map(|(_, span): &(T, SimpleSpan)| *span);
            let warning = match (last, comma) {
                (None, Some(comma)) => {
                    emitter.emit(Rich::custom(comma, "expected a value before `,`"));
                    None
                }
                (Some(_), Some(comma)) if !close_on_own_line => Some(Warning {
                    span: comma,
                    kind: WarningKind::RedundantTrailingComma,
                }),
                (Some(last), None) if close_on_own_line => Some(Warning {
                    span: (last.end..last.end).into(),
                    kind: WarningKind::MissingTrailingComma,
                }),
                _ => None,
            };
            if let Some(warning) = warning {
                e.state().0.push(warning);
            }
            items.into_iter().map(|(item, _)| item).collect()
        })
}

fn program_parser<'tok, 'src: 'tok, I>() -> impl Parser<'tok, I, Program, Extra<'tok, 'src>>
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    let ident = select! { Token::Ident(name) => name.to_string() }.labelled("identifier");

    // `/dir/other`, `./dir/other` or `other`.
    let segments = ident
        .separated_by(just(Token::Slash))
        .at_least(1)
        .collect::<Vec<_>>();
    let module_path = choice((
        just(Token::Slash)
            .ignore_then(segments.clone())
            .map(ModulePath::Absolute),
        just(Token::Dot)
            .then(just(Token::Slash))
            .ignore_then(segments)
            .map(ModulePath::Relative),
        ident.map(ModulePath::Named),
    ))
    .labelled("module path")
    .boxed();

    // `f`, `other::f` or `/dir/other::f`. Which module a name comes from is
    // decided by the checker.
    let name = module_path
        .clone()
        .then_ignore(just(Token::ColonColon))
        .or_not()
        .then(ident)
        .map(|(module, name)| Name { module, name })
        .boxed();

    // Unary and binary operators applied to `atom`, which is the tightest
    // binding part of an expression.
    let operators = |atom: Boxed<'tok, 'tok, I, Expr, Extra<'tok, 'src>>| {
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
        and.clone()
            .foldl(or_op.then(and).repeated(), fold_binary)
            .boxed()
    };

    let mut expr = Recursive::declare();

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
    let args = comma_list(arg, Token::LParen, Token::RParen);

    // A name followed by arguments is a call, otherwise it's a variable.
    let name_or_call = name
        .clone()
        .then(args.clone().or_not())
        .map(|(name, args)| match args {
            Some(args) => Expr::Call { callee: name, args },
            None => Expr::Var(name),
        });

    let parenthesised = expr
        .clone()
        .delimited_by(just(Token::LParen), just(Token::RParen));

    // `Type { field: value, ... }`. Every field must be labelled.
    let field_value = ident
        .then_ignore(just(Token::Colon))
        .then(expr.clone())
        .map(|(name, value)| FieldValue { name, value });
    let construct = name
        .clone()
        .then(comma_list(field_value, Token::LBrace, Token::RBrace))
        .map(|(ty, fields)| Expr::Construct { ty, fields });

    let self_value = just(Token::SelfValue).to(Expr::SelfValue);

    // `.field` or `.method(args)` after any atom, which binds tighter than
    // any operator, so `-a.b` is `-(a.b)`. They can be chained, as in
    // `a.f().b`.
    let member = just(Token::Dot).ignore_then(ident).then(args.or_not());
    let with_members = |atom: Boxed<'tok, 'tok, I, Expr, Extra<'tok, 'src>>| {
        atom.foldl(member.clone().repeated(), |receiver, (name, args)| {
            let receiver = Box::new(receiver);
            match args {
                Some(args) => Expr::MethodCall {
                    receiver,
                    method: name,
                    args,
                },
                None => Expr::Field {
                    receiver,
                    field: name,
                },
            }
        })
        .boxed()
    };

    expr.define(operators(with_members(
        literal
            .or(self_value.clone())
            .or(construct)
            .or(name_or_call.clone())
            .or(parenthesised.clone())
            .labelled("expression")
            .boxed(),
    )));

    // An `if` condition can't construct a group outside of parentheses or
    // arguments, since in `if done { ... }`, the `{` starts the `if`'s block
    // rather than constructing `done`.
    let condition = operators(with_members(
        literal
            .or(self_value)
            .or(name_or_call)
            .or(parenthesised)
            .labelled("expression")
            .boxed(),
    ));

    let mutability = select! {
        Token::Let => Mutability::Let,
        Token::Var => Mutability::Var,
    };

    // Type names are resolved by the checker, not reserved by the lexer.
    let type_name = name.labelled("type");
    let annotation = just(Token::Colon).ignore_then(type_name.clone());

    let decl = mutability
        .then(ident)
        .then(annotation.clone().or_not())
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

    // `let name: Type` or `var name: Type`. A value is parsed only so that it
    // can be reported clearly.
    let field = mutability
        .then(ident)
        .then(annotation)
        .then(just(Token::Eq).ignore_then(expr.clone()).or_not())
        .validate(|(((mutability, name), ty), value), e, emitter| {
            if value.is_some() {
                emitter.emit(Rich::custom(e.span(), "group fields can't have values"));
            }
            Field {
                mutability,
                name,
                ty,
            }
        });

    // Like a block, one field per line with blank lines allowed, and the `{`
    // on the same line as `group`.
    let group_ = just(Token::Group)
        .ignore_then(ident)
        .then(
            newlines
                .clone()
                .ignore_then(
                    field
                        .separated_by(newlines.clone().at_least(1))
                        .allow_trailing()
                        .collect(),
                )
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map(|(name, fields)| Stmt::Group { name, fields });

    // One statement per line; blank lines are allowed anywhere.
    let mut stmt = Recursive::declare();
    let function = {
        let block = newlines
            .clone()
            .ignore_then(
                stmt.clone()
                    .separated_by(newlines.clone().at_least(1))
                    .allow_trailing()
                    .collect(),
            )
            .delimited_by(just(Token::LBrace), just(Token::RBrace));

        // The `{` must be on the same line as the `if`, and `else` on the same
        // line as the preceding `}`, so newlines never need disambiguating.
        let if_ = recursive(|if_| {
            let else_block =
                just(Token::Else).ignore_then(block.clone().or(if_.map(|stmt| vec![stmt])));

            just(Token::If)
                .ignore_then(condition.clone())
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
            .then(type_name.clone())
            .map(|((label, name), ty)| Param { label, name, ty });
        let params = comma_list(param, Token::LParen, Token::RParen);

        let function = just(Token::Fn)
            .ignore_then(ident)
            .then(params)
            .then(
                just(Token::Arrow)
                    .ignore_then(type_name.clone())
                    .or_not()
                    .map(|ret| ret.unwrap_or_else(|| Name::new("Void"))),
            )
            .then(block)
            .map(|(((name, params), ret), body)| Function {
                name,
                params,
                ret,
                body,
            })
            .boxed();

        let return_ = just(Token::Return)
            .ignore_then(expr.clone().or_not())
            .map(Stmt::Return);

        // Only calls are useful on their own, since any other expression's
        // value would be thrown away.
        let call = expr.clone().try_map(|expr, span| match expr {
            Expr::Call { .. } | Expr::MethodCall { .. } => Ok(Stmt::Expr(expr)),
            _ => Err(Rich::custom(
                span,
                "only function calls can be used as statements",
            )),
        });

        stmt.define(
            decl.clone()
                .or(assign)
                .or(if_)
                .or(function.clone().map(Stmt::Fn))
                .or(group_)
                .or(return_)
                .or(call),
        );
        function
    };

    let use_ = just(Token::Use).ignore_then(module_path).map(Stmt::Use);

    // `impl Type { fn ... }`, with one function per line, like a block. Each
    // can be a method, or a `static fn`.
    let impl_function = just(Token::Static)
        .or_not()
        .then(function)
        .map(|(is_static, function)| ImplFunction {
            is_static: is_static.is_some(),
            function,
        });
    let impl_ = just(Token::Impl)
        .ignore_then(type_name.clone())
        .then(
            newlines
                .clone()
                .ignore_then(
                    impl_function
                        .separated_by(newlines.clone().at_least(1))
                        .allow_trailing()
                        .collect(),
                )
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map(|(ty, functions)| Stmt::Impl { ty, functions });

    // The top level can only declare things, so a module never runs code
    // just by being imported. Other statements are parsed only so that they
    // can be reported clearly.
    let top_level_stmt = use_.or(impl_).or(stmt.validate(|stmt, e, emitter| {
        if !matches!(stmt, Stmt::Decl { .. } | Stmt::Fn(_) | Stmt::Group { .. }) {
            emitter.emit(Rich::custom(
                e.span(),
                "only declarations can be used at the top level",
            ));
        }
        stmt
    }));

    newlines
        .clone()
        .ignore_then(
            top_level_stmt
                .separated_by(newlines.at_least(1))
                .allow_trailing()
                .collect(),
        )
        .then_ignore(end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `src` as the body of a function, since statements other than
    /// declarations aren't allowed at the top level.
    fn parse_body(src: &str) -> Result<Vec<Stmt>, Vec<String>> {
        match parse(&format!("fn body() {{\n{src}\n}}")) {
            Ok(mut program) => match program.remove(0) {
                Stmt::Fn(Function { body, .. }) => Ok(body),
                stmt => panic!("expected a function, got {stmt:?}"),
            },
            Err(errs) => Err(errs.iter().map(ToString::to_string).collect()),
        }
    }

    fn int(n: i64) -> Box<Expr> {
        Box::new(Expr::Int(n))
    }

    fn var(name: &str) -> Box<Expr> {
        Box::new(Expr::Var(Name::new(name)))
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
            parse_body("x = 1 + 2\ny = x - 3"),
            Ok(vec![
                assign("x", Expr::Binary(int(1), BinOp::Add, int(2))),
                assign("y", Expr::Binary(var("x"), BinOp::Sub, int(3))),
            ])
        );
    }

    #[test]
    fn allows_blank_lines() {
        assert_eq!(
            parse_body("\n\nx = 1\n\n\ny = 2\n\n"),
            Ok(vec![assign("x", Expr::Int(1)), assign("y", Expr::Int(2)),])
        );
    }

    #[test]
    fn binary_ops_are_left_associative() {
        assert_eq!(
            parse_body("x = 1 - 2 + 3"),
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
            parse_body("x = 1 - (2 + 3)"),
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
            parse_body("x = -5\ny = -y\nz = --(1)"),
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
            parse_body("x = 1 - -2"),
            Ok(vec![assign(
                "x",
                Expr::Binary(int(1), BinOp::Sub, neg(int(2)))
            )])
        );
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        assert_eq!(
            parse_body("x = 1 + 2 * 3 - 4 / 5"),
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
            parse_body("x = 8 / 4 * 2"),
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
            parse_body("x = -2 * 3"),
            Ok(vec![assign(
                "x",
                Expr::Binary(neg(int(2)), BinOp::Mul, int(3))
            )])
        );
    }

    #[test]
    fn parses_declarations() {
        assert_eq!(
            parse_body("let x = 1 + 2\nvar y = x\ny = 3\nlet letter = y"),
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
            ty: ty.map(Name::new),
            value,
        }
    }

    #[test]
    fn parses_explicit_types() {
        assert_eq!(
            parse_body(
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
            parse_body("let x: Foo = 1"),
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
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
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
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_boolean_literals() {
        assert_eq!(
            parse_body("x = true\ny = false"),
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
            parse_body("x = -true + 1"),
            Ok(vec![assign(
                "x",
                Expr::Binary(neg(Box::new(Expr::Bool(true))), BinOp::Add, int(1)),
            )])
        );
    }

    #[test]
    fn rejects_declaration_without_value() {
        assert!(parse_body("let x").is_err());
        assert!(parse_body("var x").is_err());
    }

    fn bin(lhs: Box<Expr>, op: BinOp, rhs: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::Binary(lhs, op, rhs))
    }

    fn not(expr: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::Unary(UnOp::Not, expr))
    }

    fn parse_expr(src: &str) -> Expr {
        match parse_body(&format!("x = {src}")).unwrap().remove(0) {
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
        assert!(parse_body("x = a && b").is_err());
        assert!(parse_body("x = a || b").is_err());
        assert!(parse_body("x = a !").is_err());
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
            parse_body(
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
            parse_body(
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
            parse_body(
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
            parse_body("if !a & b {}"),
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
            parse_body("if c { x = 1 } else {}"),
            Ok(vec![if_(
                *var("c"),
                vec![assign("x", Expr::Int(1))],
                Some(vec![]),
            )])
        );
        assert_eq!(
            parse_body("if c {\n\n}"),
            Ok(vec![if_(*var("c"), vec![], None)])
        );
    }

    #[test]
    fn parses_nested_if_and_surrounding_statements() {
        assert_eq!(
            parse_body(
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
            parse_body(
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
        assert_eq!(parse_body("// one\n// two"), Ok(vec![]));
        assert_eq!(
            parse_body(
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
    fn parses_block_comments() {
        assert_eq!(
            parse_body(
                "
                /*
                 * A comment over several lines.
                 */
                x = /* inline */ 1 + /**/ 2
                /* Commented-out code, with its own comment:
                if c {
                    x = 2 /* nested */
                }
                */
                y = 3 /* after code */
                "
            ),
            Ok(vec![
                assign("x", Expr::Binary(int(1), BinOp::Add, int(2))),
                assign("y", Expr::Int(3)),
            ])
        );
    }

    #[test]
    fn block_comments_spanning_lines_end_statements() {
        assert_eq!(
            parse_body("x = 1 /*\n*/ y = 2"),
            Ok(vec![assign("x", Expr::Int(1)), assign("y", Expr::Int(2))])
        );
        for src in [
            // On one line, a comment doesn't separate statements.
            "x = 1 /* */ y = 2",
            // A newline inside a comment still splits a statement.
            "x = /*\n*/ 1",
            "if c /*\n*/ {}",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_unbalanced_block_comments() {
        let errs = parse("let x = 1 /* /* */").unwrap_err();
        assert_eq!(errs[0].span().into_range(), 10..18);
        assert!(parse("let x = 1 /* */ */").is_err());
    }

    #[test]
    fn comments_hide_the_rest_of_the_line() {
        // Everything after `//` is ignored, including what would be an error.
        assert_eq!(
            parse_body("x = 1 // y = 2 $ {"),
            Ok(vec![assign("x", Expr::Int(1))])
        );
        // A comment swallows the `{`, so the block is never opened.
        assert!(parse_body("if c // {\n}").is_err());
        // `else` must still be on the same line as `}`.
        assert!(parse_body("if c {} // comment\nelse {}").is_err());
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
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_two_statements_on_one_line() {
        assert!(parse_body("x = 1 y = 2").is_err());
    }

    #[test]
    fn rejects_semicolons() {
        assert!(parse_body("x = 1;").is_err());
    }

    #[test]
    fn rejects_invalid_token() {
        let errs = parse("let x = 1 $ 2").unwrap_err();
        assert_eq!(errs[0].span().into_range(), 10..11);
    }

    fn param(label: Option<&str>, name: &str, ty: &str) -> Param {
        Param {
            label: label.map(str::to_string),
            name: name.to_string(),
            ty: Name::new(ty),
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
            callee: Name::new(callee),
            args,
        })
    }

    #[test]
    fn parses_function() {
        assert_eq!(
            parse_body(
                "
                fn addFive(to value: Int) -> Int {
                    return value + 5
                }
                "
            ),
            Ok(vec![Stmt::Fn(Function {
                name: "addFive".to_string(),
                params: vec![param(Some("to"), "value", "Int")],
                ret: Name::new("Int"),
                body: vec![Stmt::Return(Some(Expr::Binary(
                    var("value"),
                    BinOp::Add,
                    int(5),
                )))],
            })])
        );
    }

    #[test]
    fn parses_parameter_label_forms() {
        assert_eq!(
            parse_body("fn f(a: Int, to b: Bool, _ c: Int) {}"),
            Ok(vec![Stmt::Fn(Function {
                name: "f".to_string(),
                params: vec![
                    // A single name is both the label and the parameter name.
                    param(Some("a"), "a", "Int"),
                    param(Some("to"), "b", "Bool"),
                    // `_` means no label.
                    param(None, "c", "Int"),
                ],
                ret: Name::new("Void"),
                body: vec![],
            })])
        );
    }

    #[test]
    fn parses_function_without_parameters_or_return_type() {
        assert_eq!(
            parse_body(
                "
                fn reset() {
                    count = 0
                    return
                }
                "
            ),
            Ok(vec![Stmt::Fn(Function {
                name: "reset".to_string(),
                params: vec![],
                ret: Name::new("Void"),
                body: vec![assign("count", Expr::Int(0)), Stmt::Return(None)],
            })])
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
            parse_body("log(message: 1)"),
            Ok(vec![Stmt::Expr(*call(
                "log",
                vec![arg(Some("message"), *int(1))]
            ))])
        );
    }

    #[test]
    fn rejects_non_call_expression_statements() {
        for src in ["x", "1 + 2", "-f()", "!f() & g()"] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
        let errs = parse_body("1 + 2").unwrap_err();
        assert_eq!(
            errs[0].to_string(),
            "only function calls can be used as statements"
        );
    }

    #[test]
    fn parses_return_forms() {
        assert_eq!(parse_body("return"), Ok(vec![Stmt::Return(None)]));
        assert_eq!(
            parse_body("return 1 + 2"),
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
            "fn f(,) {}",
            "fn f(a: Int,,) {}",
            // `{` must be on the same line.
            "fn f()\n{}",
            // Keywords.
            "fn fn() {}",
            "fn return() {}",
            "let fn = 1",
            "let _ = 1",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    fn field(mutability: Mutability, name: &str, ty: &str) -> Field {
        Field {
            mutability,
            name: name.to_string(),
            ty: Name::new(ty),
        }
    }

    fn group_(name: &str, fields: Vec<Field>) -> Stmt {
        Stmt::Group {
            name: name.to_string(),
            fields,
        }
    }

    #[test]
    fn parses_group() {
        assert_eq!(
            parse_body(
                "
                group User {
                    let age: Int
                    var isAdmin: Bool
                }
                "
            ),
            Ok(vec![group_(
                "User",
                vec![
                    field(Mutability::Let, "age", "Int"),
                    field(Mutability::Var, "isAdmin", "Bool"),
                ],
            )])
        );
    }

    #[test]
    fn parses_group_layouts() {
        assert_eq!(
            parse_body("group Empty {}"),
            Ok(vec![group_("Empty", vec![])])
        );
        assert_eq!(
            parse_body("group Point { let x: Int }"),
            Ok(vec![group_(
                "Point",
                vec![field(Mutability::Let, "x", "Int")]
            )])
        );
        assert_eq!(
            parse_body(
                "
                group Post { // A comment.

                    // Another comment.
                    let author: User

                    var likes : Int
                }
                "
            ),
            Ok(vec![group_(
                "Post",
                vec![
                    field(Mutability::Let, "author", "User"),
                    field(Mutability::Var, "likes", "Int"),
                ],
            )])
        );
    }

    #[test]
    fn parses_group_inside_blocks() {
        assert_eq!(
            parse_body(
                "
                fn f() {
                    group Inner {}
                }
                "
            ),
            Ok(vec![Stmt::Fn(Function {
                name: "f".to_string(),
                params: vec![],
                ret: Name::new("Void"),
                body: vec![group_("Inner", vec![])],
            })])
        );
    }

    #[test]
    fn rejects_group_field_values() {
        for src in [
            "group User { let age: Int = 3 }",
            "group User { var isAdmin: Bool = true }",
        ] {
            let errs = parse_body(src).unwrap_err();
            assert_eq!(
                errs[0].to_string(),
                "group fields can't have values",
                "{src:?}"
            );
        }
        assert!(parse_body("group User { let age = 3 }").is_err());
    }

    #[test]
    fn rejects_malformed_groups() {
        for src in [
            // Missing parts.
            "group {}",
            "group User",
            "group User {",
            "group User { let }",
            "group User { let age }",
            "group User { let age: }",
            // Fields need `let` or `var`.
            "group User { age: Int }",
            // Only fields are allowed.
            "group User { x = 1 }",
            "group User { f() }",
            "group User { return }",
            "group User { if c {} }",
            "group User { fn f() {} }",
            "group User { group Inner {} }",
            // Separators.
            "group User { let a: Int var b: Int }",
            "group User { let a: Int, var b: Int }",
            "group User { let a: Int; }",
            // `{` must be on the same line.
            "group User\n{}",
            // Statements after `}` need their own line.
            "group User {} x = 1",
            // Keywords.
            "group group {}",
            "let group = 1",
            "group User { let let: Int }",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_malformed_calls() {
        for src in [
            "x = f(",
            "x = f(a b)",
            "x = f(a:)",
            "x = f(_: 1)",
            "x = f(,)",
            "x = f(1,,)",
            "x = f(1, ,)",
            "x = f(a: b: 1)",
            "x = 1(2)",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_top_level_declarations() {
        assert_eq!(
            parse(
                "
                use /dir/other
                use ./dir/other
                use stdlib
                let x = 1
                var y: Int = 2
                fn f() {}
                group S {}
                "
            ),
            Ok(vec![
                Stmt::Use(ModulePath::Absolute(vec![
                    "dir".to_string(),
                    "other".to_string()
                ])),
                Stmt::Use(ModulePath::Relative(vec![
                    "dir".to_string(),
                    "other".to_string()
                ])),
                Stmt::Use(ModulePath::Named("stdlib".to_string())),
                decl(Mutability::Let, "x", None, Expr::Int(1)),
                decl(Mutability::Var, "y", Some("Int"), Expr::Int(2)),
                Stmt::Fn(Function {
                    name: "f".to_string(),
                    params: vec![],
                    ret: Name::new("Void"),
                    body: vec![],
                }),
                group_("S", vec![]),
            ])
        );
    }

    #[test]
    fn rejects_other_statements_at_the_top_level() {
        for src in ["x = 1", "f()", "if c {}", "return", "return 1"] {
            let errs = parse(src).unwrap_err();
            assert_eq!(
                errs[0].to_string(),
                "only declarations can be used at the top level",
                "{src:?}"
            );
        }
    }

    #[test]
    fn rejects_use_outside_the_top_level() {
        assert!(parse_body("use /lib").is_err());
        assert!(parse("if c {\nuse /lib\n}").is_err());
    }

    fn qualified(module: ModulePath, name: &str) -> Name {
        Name {
            module: Some(module),
            name: name.to_string(),
        }
    }

    #[test]
    fn parses_qualified_names() {
        let other = || ModulePath::Named("other".to_string());
        let path = |segments: &[&str]| segments.iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_expr("other::x"), Expr::Var(qualified(other(), "x")));
        assert_eq!(
            parse_expr("/dir/other::f() / ./other::y"),
            Expr::Binary(
                Box::new(Expr::Call {
                    callee: qualified(ModulePath::Absolute(path(&["dir", "other"])), "f"),
                    args: vec![],
                }),
                BinOp::Div,
                Box::new(Expr::Var(qualified(
                    ModulePath::Relative(path(&["other"])),
                    "y"
                ))),
            )
        );
        assert_eq!(
            parse_body("let x: stdlib::Int = 1"),
            Ok(vec![Stmt::Decl {
                mutability: Mutability::Let,
                name: "x".to_string(),
                ty: Some(qualified(ModulePath::Named("stdlib".to_string()), "Int")),
                value: Expr::Int(1),
            }])
        );
        // Division still works without spaces.
        assert_eq!(parse_expr("a/b"), *bin(var("a"), BinOp::Div, var("b")));
    }

    #[test]
    fn rejects_malformed_modules() {
        for src in [
            "use",
            "use /",
            "use ./",
            "use /a/",
            "use a/b",
            "use .a",
            "use ../a",
            "use /a::b",
            "use /a\n/b",
            "use /a use /b",
            "use a::b",
            "let x = a::",
            "let x = ::a",
            "let x = a::b::c",
            "let x = a::b.c::d",
            "let x = a:::b",
            "let x = a: :b",
            "let x: a:: = 1",
            "let x = /a::",
            "let x = ./a/::b",
            // `.` doesn't qualify names.
            "let x = /a.b",
            "let x: a.B = 1",
            // Assignments can't be qualified.
            "fn f() {\na::x = 1\n}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    fn field_value(name: &str, value: Expr) -> FieldValue {
        FieldValue {
            name: name.to_string(),
            value,
        }
    }

    fn construct(ty: Name, fields: Vec<FieldValue>) -> Expr {
        Expr::Construct { ty, fields }
    }

    #[test]
    fn parses_constructors() {
        assert_eq!(
            parse_expr("User { age: 42, isAdmin: true }"),
            construct(
                Name::new("User"),
                vec![
                    field_value("age", Expr::Int(42)),
                    field_value("isAdmin", Expr::Bool(true)),
                ],
            )
        );
        assert_eq!(
            parse_expr("Empty {}"),
            construct(Name::new("Empty"), vec![])
        );
        assert_eq!(
            parse_expr("user::User {age:1+2,isAdmin:f()}"),
            construct(
                qualified(ModulePath::Named("user".to_string()), "User"),
                vec![
                    field_value("age", *bin(int(1), BinOp::Add, int(2))),
                    field_value("isAdmin", *call("f", vec![])),
                ],
            )
        );
    }

    #[test]
    fn constructors_nest_and_bind_tightest() {
        assert_eq!(
            parse_expr("-Team { lead: User { age: 1 } } * 2"),
            *bin(
                neg(Box::new(construct(
                    Name::new("Team"),
                    vec![field_value(
                        "lead",
                        construct(Name::new("User"), vec![field_value("age", Expr::Int(1))]),
                    )],
                ))),
                BinOp::Mul,
                int(2),
            )
        );
        assert_eq!(
            parse_expr("f(user: User {})"),
            *call(
                "f",
                vec![arg(Some("user"), construct(Name::new("User"), vec![]))]
            )
        );
    }

    #[test]
    fn if_conditions_need_parentheses_to_construct() {
        // The `{` starts the block, not a constructor.
        assert_eq!(
            parse_body("if done {}"),
            Ok(vec![if_(*var("done"), vec![], None)])
        );
        assert_eq!(
            parse_body("if (Flag {}) {}"),
            Ok(vec![if_(
                construct(Name::new("Flag"), vec![]),
                vec![],
                None
            )])
        );
        assert_eq!(
            parse_body("if f(flag: Flag {}) {}"),
            Ok(vec![if_(
                *call(
                    "f",
                    vec![arg(Some("flag"), construct(Name::new("Flag"), vec![]))]
                ),
                vec![],
                None,
            )])
        );
        assert!(parse_body("if Flag { on: true } {}").is_err());
    }

    #[test]
    fn rejects_constructor_statements() {
        // Only calls can be statements.
        assert!(parse_body("User {}").is_err());
        assert!(parse_body("User { age: 1 }").is_err());
    }

    #[test]
    fn rejects_malformed_constructors() {
        for src in [
            "x = User {",
            "x = User { age }",
            "x = User { age: }",
            "x = User { 42 }",
            "x = User { age = 42 }",
            "x = User { age: 1,, }",
            "x = User { age: 1 isAdmin: true }",
            "x = User { , }",
            "x = User { _: 1 }",
            "x = User\n{}",
            "x = 1 {}",
            "x = (User) {}",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_multiline_constructors() {
        let user = || {
            construct(
                Name::new("User"),
                vec![
                    field_value("age", Expr::Int(42)),
                    field_value("isAdmin", Expr::Bool(true)),
                ],
            )
        };
        assert_eq!(
            parse_body(
                "
                let a = User {
                    age: 42,
                    isAdmin: true
                }
                let b = User { age: 42,
                    isAdmin: true }
                let c = User {

                    // The age in years.
                    age: 42, /* Never negative. */

                    isAdmin: true
                }
                "
            ),
            Ok(vec![
                decl(Mutability::Let, "a", None, user()),
                decl(Mutability::Let, "b", None, user()),
                decl(Mutability::Let, "c", None, user()),
            ])
        );
        assert_eq!(
            parse_expr("Empty {\n\n}"),
            construct(Name::new("Empty"), vec![])
        );
    }

    #[test]
    fn parses_nested_multiline_constructors() {
        assert_eq!(
            parse_body(
                "
                let t = Team {
                    lead: User {
                        age: 30
                    },
                    size: f(x: Point {
                        x: 1
                    })
                }
                "
            ),
            Ok(vec![decl(
                Mutability::Let,
                "t",
                None,
                construct(
                    Name::new("Team"),
                    vec![
                        field_value(
                            "lead",
                            construct(Name::new("User"), vec![field_value("age", Expr::Int(30))]),
                        ),
                        field_value(
                            "size",
                            *call(
                                "f",
                                vec![arg(
                                    Some("x"),
                                    construct(
                                        Name::new("Point"),
                                        vec![field_value("x", Expr::Int(1))]
                                    ),
                                )],
                            ),
                        ),
                    ],
                ),
            )])
        );
    }

    #[test]
    fn rejects_malformed_multiline_constructors() {
        for src in [
            // Fields still need commas between them.
            "
            x = User {
                age: 42
                isAdmin: true
            }
            ",
            // The comma must come straight after the value.
            "
            x = User {
                age: 42
                , isAdmin: true
            }
            ",
            // A field can't be split across lines.
            "
            x = User {
                age:
                    42
            }
            ",
            // The `{` must be on the same line as the type.
            "
            x = User
            {
                age: 42
            }
            ",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_multiline_calls() {
        let expected = || {
            *call(
                "bump",
                vec![
                    arg(None, *var("y")),
                    arg(Some("by"), Expr::Int(2)),
                    arg(Some("twice"), Expr::Bool(true)),
                ],
            )
        };
        for src in [
            "
            bump(
                y,
                by: 2,
                twice: true
            )
            ",
            "
            bump(y,
                by: 2, twice: true)
            ",
            "
            bump(

                // The value to bump.
                y, /* By how much. */ by: 2,

                twice: true
            )
            ",
        ] {
            assert_eq!(parse_body(src), Ok(vec![Stmt::Expr(expected())]), "{src:?}");
        }
        assert_eq!(parse_expr("f(\n\n)"), *call("f", vec![]));
    }

    #[test]
    fn parses_nested_multiline_calls() {
        assert_eq!(
            parse_body(
                "
                let x = f(
                    a: g(
                        1
                    ),
                    b: User {
                        age: h(
                            c: 2
                        )
                    }
                ) + 1
                "
            ),
            Ok(vec![decl(
                Mutability::Let,
                "x",
                None,
                *bin(
                    call(
                        "f",
                        vec![
                            arg(Some("a"), *call("g", vec![arg(None, Expr::Int(1))])),
                            arg(
                                Some("b"),
                                construct(
                                    Name::new("User"),
                                    vec![field_value(
                                        "age",
                                        *call("h", vec![arg(Some("c"), Expr::Int(2))]),
                                    )],
                                ),
                            ),
                        ],
                    ),
                    BinOp::Add,
                    int(1),
                ),
            )])
        );
    }

    #[test]
    fn rejects_malformed_multiline_calls() {
        for src in [
            // Arguments still need commas between them.
            "
            f(
                a: 1
                b: 2
            )
            ",
            // The comma must come straight after the value.
            "
            f(
                a: 1
                , b: 2
            )
            ",
            // An argument can't be split across lines.
            "
            f(
                a:
                    1
            )
            ",
            // The `(` must be on the same line as the function's name.
            "
            f
            (a: 1)
            ",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    /// The kinds of warnings from parsing `src` as the body of a function.
    fn warnings(src: &str) -> Vec<WarningKind> {
        let src = format!("fn body() {{\n{src}\n}}");
        let (result, warnings) = parse_with_warnings(&src);
        assert!(result.is_ok(), "{src:?} should parse");
        warnings.into_iter().map(|warning| warning.kind).collect()
    }

    #[test]
    fn allows_trailing_commas() {
        assert_eq!(parse_expr("f(a: 1,)"), parse_expr("f(a: 1)"));
        assert_eq!(
            parse_expr("User { age: 1, }"),
            parse_expr("User { age: 1 }")
        );
        assert_eq!(
            parse_body(
                "
                f(
                    a: 1,
                    b: 2,
                )
                "
            ),
            parse_body("f(a: 1, b: 2)")
        );
    }

    #[test]
    fn warns_about_trailing_commas_on_one_line() {
        assert_eq!(
            warnings("f(a: 1,)\nx = User { age: 1, }"),
            [
                WarningKind::RedundantTrailingComma,
                WarningKind::RedundantTrailingComma,
            ]
        );
        // The closing bracket is on the same line as the last item.
        assert_eq!(
            warnings(
                "
                f(a: 1,
                    b: 2,)
                "
            ),
            [WarningKind::RedundantTrailingComma]
        );
    }

    #[test]
    fn warns_about_missing_trailing_commas_over_several_lines() {
        assert_eq!(
            warnings(
                "
                f(
                    a: 1,
                    b: 2
                )
                x = User {
                    age: 1
                }
                "
            ),
            [
                WarningKind::MissingTrailingComma,
                WarningKind::MissingTrailingComma,
            ]
        );
    }

    #[test]
    fn does_not_warn_about_conventional_commas() {
        assert_eq!(
            warnings(
                "
                f(a: 1, b: 2)
                f(
                    a: 1,
                    b: 2, // A comment after the comma.
                )
                f(a: 1,
                    b: 2)
                x = User {
                    age: 1,

                }
                f()
                f(
                )
                x = Empty {}
                "
            ),
            []
        );
    }

    #[test]
    fn warns_about_each_nested_list() {
        // Only the lists whose own layout is wrong.
        assert_eq!(
            warnings(
                "
                f(a: g(
                    b: 1
                ), c: h(d: 2,))
                "
            ),
            [
                WarningKind::MissingTrailingComma,
                WarningKind::RedundantTrailingComma,
            ]
        );
    }

    #[test]
    fn warning_spans_point_at_the_comma() {
        let (_, warnings) = parse_with_warnings("let x = f(a: 1,)");
        assert_eq!(
            warnings,
            [Warning {
                span: (14..15).into(),
                kind: WarningKind::RedundantTrailingComma,
            }]
        );
        let (_, warnings) = parse_with_warnings("let x = f(\na: 1\n)");
        assert_eq!(
            warnings,
            [Warning {
                span: (15..15).into(),
                kind: WarningKind::MissingTrailingComma,
            }]
        );
        assert_eq!(
            warnings[0].to_string(),
            "missing trailing comma in a list over several lines"
        );
    }

    #[test]
    fn warnings_from_abandoned_parses_are_dropped() {
        // `f(b: 1,)` is parsed while trying to read a constructor, which then
        // fails at `c`, so its warning is dropped along with it.
        let (result, warnings) = parse_with_warnings("let x = User { a: f(b: 1,), c }");
        assert!(result.is_err());
        assert_eq!(warnings, []);
    }

    #[test]
    fn warns_even_when_parsing_fails() {
        let (result, warnings) = parse_with_warnings("let x = f(a: 1,)\nlet y =");
        assert!(result.is_err());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn parses_multiline_parameters() {
        let expected = parse("fn bump(_ value: Int, by amount: Int, twice: Bool) {}");
        for src in [
            "
            fn bump(
                _ value: Int,
                by amount: Int,
                twice: Bool,
            ) {}
            ",
            "
            fn bump(_ value: Int,
                by amount: Int, twice: Bool) {}
            ",
            "
            fn bump(

                // The value to bump.
                _ value: Int, /* By how much. */ by amount: Int,

                twice: Bool,
            ) {}
            ",
            "fn bump(_ value: Int, by amount: Int, twice: Bool,) {}",
        ] {
            assert_eq!(parse(src), expected, "{src:?}");
        }
        assert_eq!(parse("fn f(\n\n) {}"), parse("fn f() {}"));
    }

    #[test]
    fn warns_about_trailing_commas_in_parameters() {
        assert_eq!(
            warnings(
                "
                fn a(x: Int,) {}
                fn b(
                    x: Int
                ) -> Int {
                    return x
                }
                fn c(
                    x: Int,
                ) {}
                fn d(x: Int) {}
                "
            ),
            [
                WarningKind::RedundantTrailingComma,
                WarningKind::MissingTrailingComma,
            ]
        );
    }

    #[test]
    fn rejects_malformed_multiline_parameters() {
        for src in [
            // Parameters still need commas between them.
            "
            fn f(
                a: Int
                b: Int,
            ) {}
            ",
            // The comma must come straight after the parameter.
            "
            fn f(
                a: Int
                , b: Int,
            ) {}
            ",
            // A parameter can't be split across lines.
            "
            fn f(
                a:
                    Int,
            ) {}
            ",
            "
            fn f(
                to
                    value: Int,
            ) {}
            ",
            // The `(` must be on the same line as the function's name.
            "
            fn f
            (a: Int) {}
            ",
            // The `{` must be on the same line as the `)`, or return type.
            "
            fn f(
                a: Int,
            )
            {}
            ",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    fn method_call(receiver: Expr, method: &str, args: Vec<Arg>) -> Expr {
        Expr::MethodCall {
            receiver: Box::new(receiver),
            method: method.to_string(),
            args,
        }
    }

    #[test]
    fn parses_impl() {
        assert_eq!(
            parse(
                "
                impl User {
                    fn score() -> Int {
                        return self.base()
                    }

                    // Another method.
                    fn rename(to name: Int) {}

                    static fn make() -> User {
                        return User.make()
                    }
                }
                impl user::User {}
                "
            ),
            Ok(vec![
                Stmt::Impl {
                    ty: Name::new("User"),
                    functions: vec![
                        ImplFunction {
                            is_static: false,
                            function: Function {
                                name: "score".to_string(),
                                params: vec![],
                                ret: Name::new("Int"),
                                body: vec![Stmt::Return(Some(method_call(
                                    Expr::SelfValue,
                                    "base",
                                    vec![],
                                )))],
                            },
                        },
                        ImplFunction {
                            is_static: false,
                            function: Function {
                                name: "rename".to_string(),
                                params: vec![param(Some("to"), "name", "Int")],
                                ret: Name::new("Void"),
                                body: vec![],
                            },
                        },
                        ImplFunction {
                            is_static: true,
                            function: Function {
                                name: "make".to_string(),
                                params: vec![],
                                ret: Name::new("User"),
                                body: vec![Stmt::Return(Some(method_call(
                                    *var("User"),
                                    "make",
                                    vec![],
                                )))],
                            },
                        },
                    ],
                },
                Stmt::Impl {
                    ty: qualified(ModulePath::Named("user".to_string()), "User"),
                    functions: vec![],
                },
            ])
        );
    }

    #[test]
    fn parses_method_calls() {
        assert_eq!(
            parse_expr("a.f(x: 1).g()"),
            method_call(
                method_call(*var("a"), "f", vec![arg(Some("x"), Expr::Int(1))]),
                "g",
                vec![],
            )
        );
        assert_eq!(
            parse_expr("f().g() + (1 + 2).h() + 3.i()"),
            *bin(
                bin(
                    Box::new(method_call(*call("f", vec![]), "g", vec![])),
                    BinOp::Add,
                    Box::new(method_call(*bin(int(1), BinOp::Add, int(2)), "h", vec![])),
                ),
                BinOp::Add,
                Box::new(method_call(Expr::Int(3), "i", vec![])),
            )
        );
        assert_eq!(
            parse_expr("User {}.f()"),
            method_call(construct(Name::new("User"), vec![]), "f", vec![])
        );
        assert_eq!(
            parse_expr(
                "other::x.f(
                    a: 1,
                )"
            ),
            method_call(
                Expr::Var(qualified(ModulePath::Named("other".to_string()), "x")),
                "f",
                vec![arg(Some("a"), Expr::Int(1))],
            )
        );
    }

    #[test]
    fn method_calls_bind_tightest() {
        assert_eq!(
            parse_expr("-a.f() * 2"),
            *bin(
                neg(Box::new(method_call(*var("a"), "f", vec![]))),
                BinOp::Mul,
                int(2),
            )
        );
    }

    #[test]
    fn parses_method_call_statements_and_conditions() {
        assert_eq!(
            parse_body("user.promote()\nif user.isAdmin() {}"),
            Ok(vec![
                Stmt::Expr(method_call(*var("user"), "promote", vec![])),
                if_(method_call(*var("user"), "isAdmin", vec![]), vec![], None),
            ])
        );
    }

    #[test]
    fn rejects_malformed_impls() {
        for src in [
            "impl {}",
            "impl User",
            "impl User\n{}",
            "impl 1 {}",
            "impl User { fn f() {} fn g() {} }",
            // Only functions are allowed inside.
            "impl User {\nlet x = 1\n}",
            "impl User {\ngroup S {}\n}",
            "impl User {\nimpl User {}\n}",
            "impl User {\nuse /lib\n}",
            "impl User {\nf()\n}",
            // Only at the top level.
            "fn f() {\nimpl User {}\n}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_malformed_method_calls() {
        for src in [
            "x = a.",
            "x = .f()",
            "x = a.1()",
            "x = a..f()",
            "x = a.f\n()",
            "x = a.self()",
            "x = a.f().",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn self_is_a_keyword() {
        assert_eq!(parse_expr("self"), Expr::SelfValue);
        for src in [
            "let self = 1",
            "self = 1",
            "fn self() {}",
            "fn f(self: Int) {}",
            "fn f(_ self: Int) {}",
            "f(self: 1)",
            "x = self::f()",
            "group self {}",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    fn field_access(receiver: Expr, field: &str) -> Expr {
        Expr::Field {
            receiver: Box::new(receiver),
            field: field.to_string(),
        }
    }

    #[test]
    fn parses_field_access() {
        assert_eq!(parse_expr("user.age"), field_access(*var("user"), "age"));
        assert_eq!(
            parse_expr("team.lead.age"),
            field_access(field_access(*var("team"), "lead"), "age")
        );
        assert_eq!(parse_expr("self.age"), field_access(Expr::SelfValue, "age"));
        assert_eq!(
            parse_expr("User { age: 1 }.age"),
            field_access(
                construct(Name::new("User"), vec![field_value("age", Expr::Int(1))]),
                "age"
            )
        );
        assert_eq!(
            parse_expr("other::user.age"),
            field_access(
                Expr::Var(qualified(ModulePath::Named("other".to_string()), "user")),
                "age"
            )
        );
    }

    #[test]
    fn mixes_fields_and_method_calls() {
        assert_eq!(
            parse_expr("a.f().b.g(x: c.d)"),
            method_call(
                field_access(method_call(*var("a"), "f", vec![]), "b"),
                "g",
                vec![arg(Some("x"), field_access(*var("c"), "d"))],
            )
        );
    }

    #[test]
    fn field_access_binds_tightest() {
        assert_eq!(
            parse_expr("-a.b * c.d"),
            *bin(
                neg(Box::new(field_access(*var("a"), "b"))),
                BinOp::Mul,
                Box::new(field_access(*var("c"), "d")),
            )
        );
        assert_eq!(
            parse_body("if user.isAdmin {}"),
            Ok(vec![if_(
                field_access(*var("user"), "isAdmin"),
                vec![],
                None
            )])
        );
    }

    #[test]
    fn rejects_malformed_field_access() {
        for src in [
            // Fields can't be assigned to.
            "user.age = 1",
            // A field on its own isn't a statement.
            "user.age",
            "x = a.b.",
            "x = a.1",
            "x = a.b\n.c",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_static_outside_impls() {
        for src in [
            "static fn f() {}",
            "fn f() {\nstatic fn g() {}\n}",
            "impl Int {\nstatic\n}",
            "impl Int {\nstatic static fn f() {}\n}",
            "impl Int {\nstatic let x = 1\n}",
            "impl Int {\nfn static() {}\n}",
            "let static = 1",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }
}
