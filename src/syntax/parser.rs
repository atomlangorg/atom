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
    extra::ParserExtra,
    input::{MapExtra, Stream, ValueInput},
    pratt::{infix, left, prefix},
    prelude::*,
};

use super::{
    ast::{
        Alias, Arg, BinOp, ChoiceOption, Expr, ExprKind, Field, FieldValue, Function, Import,
        ImportPath, MatchArm, Mutability, Name, Pattern, PatternKind, Program, Span, Stmt,
        StmtKind, TupleElement, TuplePattern, TypeName, UnOp,
    },
    lexer::{self, Token},
};

pub type ParseError<'src> = Rich<'src, Token<'src>>;

impl From<SimpleSpan> for Span {
    fn from(span: SimpleSpan) -> Self {
        Self {
            start: span.start,
            end: span.end,
        }
    }
}

type Extra<'tok, 'src> = extra::Err<Rich<'tok, Token<'src>>>;

/// Lexes and parses `src` into a program.
pub fn parse(src: &str) -> Result<Program, Vec<ParseError<'_>>> {
    let tokens = lexer::tokens(src).map(|(tok, span)| (tok, span.into()));
    parse_tokens(tokens, src.len())
}

/// Parses `tokens` into a program. The source ends at the byte `end`.
fn parse_tokens<'src>(
    tokens: impl Iterator<Item = (Token<'src>, SimpleSpan)> + 'src,
    end: usize,
) -> Result<Program, Vec<ParseError<'src>>> {
    let stream = Stream::from_iter(tokens).map((end..end).into(), |(t, s)| (t, s));
    program_parser().parse(stream).into_result()
}

/// Combines the operands of a binary operator into an expression.
fn fold_binary<'src, I: Input<'src>, E: ParserExtra<'src, I>>(
    lhs: Expr,
    op: BinOp,
    rhs: Expr,
    _: &mut MapExtra<'src, '_, I, E>,
) -> Expr {
    Expr {
        span: lhs.span.to(rhs.span),
        kind: ExprKind::Binary(Box::new(lhs), op, Box::new(rhs)),
    }
}

/// Whether `expr` is a comparison that isn't in brackets.
fn is_comparison(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::Binary(_, op, _) if op.is_comparison())
}

/// The type written `Void`, which is what's meant where no type is written, at
/// `span`.
fn void_type(span: Span) -> TypeName {
    TypeName::Named(Name {
        span,
        ..Name::new("Void")
    })
}

/// A line in an `impl` or `trait`, which are collected into its aliases and
/// functions.
enum Member {
    Alias(Alias),
    Function(Function),
}

/// The aliases and functions of `members`, in order.
fn split(members: Vec<Member>) -> (Vec<Alias>, Vec<Function>) {
    let mut aliases = vec![];
    let mut functions = vec![];
    for member in members {
        match member {
            Member::Alias(alias) => aliases.push(alias),
            Member::Function(function) => functions.push(function),
        }
    }
    (aliases, functions)
}

/// Where a function or alias is declared, which decides how it's written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// A block, or the top level of a file.
    Stmt,
    Impl,
    Trait,
}

impl Place {
    /// What's wrong with `function` here, if anything, which is a `primitive
    /// fn` if `is_primitive`.
    fn function_error(self, function: &Function, is_primitive: bool) -> Option<&'static str> {
        match self {
            Self::Stmt if function.is_static => {
                Some("only an `impl` or `trait` can have a `static fn`")
            }
            Self::Trait if is_primitive => Some("a `trait` can't have a `primitive fn`"),
            Self::Trait => None,
            _ if is_primitive && function.body.is_some() => {
                Some("a `primitive fn` has no body, since the compiler implements it")
            }
            _ if !is_primitive && function.body.is_none() => Some("expected the function's body"),
            _ => None,
        }
    }

    /// What's wrong with `alias` here, if anything.
    fn alias_error(self, alias: &Alias) -> Option<&'static str> {
        match (self, &alias.ty) {
            (Self::Trait, Some(_)) => {
                Some("a `trait`'s alias has no type, since each `impl` gives one")
            }
            (Self::Stmt | Self::Impl, None) => Some("expected `=` and the type it stands for"),
            _ => None,
        }
    }
}

/// `item`s, one per line, with blank lines allowed anywhere, as in a block
/// or a file.
fn lines<'tok, 'src: 'tok, I, T>(
    item: impl Parser<'tok, I, T, Extra<'tok, 'src>> + Clone,
) -> impl Parser<'tok, I, Vec<T>, Extra<'tok, 'src>> + Clone
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    let newlines = just(Token::Newline).repeated();
    newlines.clone().ignore_then(
        item.separated_by(newlines.at_least(1))
            .allow_trailing()
            .collect(),
    )
}

/// `lines` of `item` between `{` and `}`, as in a block, group or `impl`.
/// The `{` must be on the same line as what comes before it.
fn braced_lines<'tok, 'src: 'tok, I, T>(
    item: impl Parser<'tok, I, T, Extra<'tok, 'src>> + Clone,
) -> impl Parser<'tok, I, Vec<T>, Extra<'tok, 'src>> + Clone
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    lines(item).delimited_by(just(Token::LBrace), just(Token::RBrace))
}

/// A comma-separated list of `item`s between `open` and `close`, as in a
/// call, constructor or function's parameters, which can end with a comma.
/// It can be split over several lines after `open`, after each comma, and
/// before `close`.
fn comma_list<'tok, 'src: 'tok, I, T>(
    item: impl Parser<'tok, I, T, Extra<'tok, 'src>> + Clone,
    open: Token<'src>,
    close: Token<'src>,
) -> impl Parser<'tok, I, Vec<T>, Extra<'tok, 'src>> + Clone
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    let line_breaks = just(Token::Newline).repeated();
    let comma = just(Token::Comma).then(line_breaks.clone());
    just(open)
        .then(line_breaks.clone())
        .ignore_then(item.separated_by(comma.clone()).collect::<Vec<_>>())
        .then(comma.map_with(|_, e| e.span()).or_not())
        .then_ignore(line_breaks)
        .then_ignore(just(close))
        .validate(|(items, comma), _, emitter| {
            if let (true, Some(comma)) = (items.is_empty(), comma) {
                emitter.emit(Rich::custom(comma, "expected a value before `,`"));
            }
            items
        })
}

/// The file or module that `path` in a `use` refers to, or `None` if it isn't
/// a path. `/dir/other` starts from the root, `./dir/other` from the current
/// directory, `../dir/other` from the one above it, and `stdlib/core` from a
/// package. Ending in a slash, as in `/dir/`, `./`, `../` or `stdlib/`, is a
/// module: every file in a directory and its subdirectories. A name alone,
/// as in `other`, is a file, so that the checker can explain why it isn't
/// found.
fn import(path: &str) -> Option<Import> {
    let after_up = path.trim_start_matches("../");
    let up = (path.len() - after_up.len()) / 3;
    let (rest, start): (_, Box<dyn FnOnce(Vec<String>) -> ImportPath>) = if up > 0 {
        (
            after_up,
            Box::new(move |segments| ImportPath::Parent(up, segments)),
        )
    } else if let Some(rest) = path.strip_prefix("./") {
        (rest, Box::new(ImportPath::Relative))
    } else if let Some(rest) = path.strip_prefix('/') {
        (rest, Box::new(ImportPath::Absolute))
    } else {
        (path, Box::new(ImportPath::Package))
    };
    let (names, is_module) = match rest.strip_suffix('/') {
        Some(names) => (names, true),
        None => (rest, rest.is_empty()),
    };
    // Each directory and file is named like anything else, so `..` can only
    // be at the start.
    let is_name = |name: &str| {
        matches!(
            &lexer::tokens(name).collect::<Vec<_>>()[..],
            [(Token::Ident(_), _)]
        )
    };
    let segments = match names {
        "" => vec![],
        names => names
            .split('/')
            .map(|name| is_name(name).then(|| name.to_string()))
            .collect::<Option<_>>()?,
    };
    let path = match start(segments) {
        // Only a module can be just a package's name.
        ImportPath::Package(mut segments) if !is_module && segments.len() == 1 => {
            ImportPath::Named(segments.remove(0))
        }
        path => path,
    };
    Some(if is_module {
        Import::Module(path)
    } else {
        Import::File { path, alias: None }
    })
}

fn program_parser<'tok, 'src: 'tok, I>() -> impl Parser<'tok, I, Program, Extra<'tok, 'src>>
where
    I: ValueInput<'tok, Token = Token<'src>, Span = SimpleSpan>,
{
    let ident = select! { Token::Ident(name) => name.to_string() }.labelled("identifier");

    // `f` or `other::f`. Which file a name comes from is decided by the
    // checker. `Self` is a keyword, so it can't be qualified, and the checker
    // decides what it refers to.
    let name = just(Token::SelfType)
        .map_with(|_, e| Name {
            span: Span::from(e.span()),
            ..Name::new("Self")
        })
        .or(ident
            .then_ignore(just(Token::ColonColon))
            .or_not()
            .then(ident)
            .map_with(|(file, name), e| Name {
                file,
                name,
                span: Span::from(e.span()),
            }))
        .boxed();

    // Type names are resolved by the checker, not reserved by the lexer. A
    // tuple type is written like a function's parameters, as in
    // `(at x: Int, y: Int)`. A generic type is given its arguments, as in
    // `Optional<Int>`, and in a generic type's `impl`, `Self.T` is what its
    // parameter `T` stands for.
    let mut type_name = Recursive::declare();
    let type_args = type_name
        .clone()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just(Token::Lt), just(Token::Gt));
    // A name that's a type, which can be given generic arguments.
    let named_type = name
        .clone()
        .then(type_args.clone().or_not())
        .map(|(name, args)| match args {
            Some(args) => TypeName::Generic { name, args },
            None => TypeName::Named(name),
        });
    // An element of a tuple type, a function's parameters or an option's
    // value: a label, then optionally a name, then its type, where `_` is
    // neither. With only a label, as in `x: Int`, it's also the name, so
    // `_: Int` has neither.
    let label_or_blank = ident.map(Some).or(just(Token::Underscore).to(None));
    let element = label_or_blank
        .clone()
        .then(label_or_blank.or_not())
        .then_ignore(just(Token::Colon))
        .then(type_name.clone())
        .try_map(|((label, name), ty), span| match (label, name) {
            (None, Some(None)) => Err(Rich::custom(
                span,
                "`_ _` is written `_`, since neither has a name",
            )),
            (label, name) => Ok(TupleElement {
                name: name.unwrap_or_else(|| label.clone()),
                label,
                ty,
                span: Span::from(span),
            }),
        })
        .boxed();
    let self_param = just(Token::SelfType)
        .ignore_then(just(Token::Dot))
        .ignore_then(ident)
        .map_with(|param, e| TypeName::SelfParam(param, Span::from(e.span())));
    type_name.define(
        self_param
            .or(named_type.clone())
            .or(comma_list(element.clone(), Token::LParen, Token::RParen).map(TypeName::Tuple))
            .labelled("type"),
    );

    // Unary and binary operators applied to `atom`, which is the tightest
    // binding part of an expression.
    let operators = |atom: Boxed<'tok, 'tok, I, Expr, Extra<'tok, 'src>>| {
        let unary_op = select! {
            Token::Minus => UnOp::Neg,
            Token::Bang => UnOp::Not,
            Token::DotBang => UnOp::BitNot,
        };
        let product_op = select! {
            Token::Star => BinOp::Mul,
            Token::Slash => BinOp::Div,
        };
        let sum_op = select! {
            Token::Plus => BinOp::Add,
            Token::Minus => BinOp::Sub,
        };
        let shift_op = select! {
            Token::DotLt => BinOp::ShiftLeft,
            Token::DotGt => BinOp::ShiftRight,
        };
        let bit_and_op = just(Token::DotAmp).to(BinOp::BitAnd);
        let bit_xor_op = just(Token::DotCaret).to(BinOp::BitXor);
        let bit_or_op = just(Token::DotPipe).to(BinOp::BitOr);
        let comparison_op = select! {
            Token::EqEq => BinOp::Eq,
            Token::BangEq => BinOp::Ne,
            Token::Lt => BinOp::Lt,
            Token::LtEq => BinOp::Le,
            Token::Gt => BinOp::Gt,
            Token::GtEq => BinOp::Ge,
        };
        let and_op = just(Token::Amp).to(BinOp::And);
        let or_op = just(Token::Pipe).to(BinOp::Or);

        // Binary operators are left-associative: `a - b + c` is `(a - b) + c`.
        // A higher number binds more tightly.
        atom.pratt((
            prefix(10, unary_op, |op, rhs: Expr, e| Expr {
                span: Span::from(e.span()),
                kind: ExprKind::Unary(op, Box::new(rhs)),
            }),
            infix(left(9), product_op, fold_binary),
            infix(left(8), sum_op, fold_binary),
            infix(left(7), shift_op, fold_binary),
            infix(left(6), bit_and_op, fold_binary),
            infix(left(5), bit_xor_op, fold_binary),
            infix(left(4), bit_or_op, fold_binary),
            // Comparisons can't be chained, since `a == b == c` would compare
            // the result of `a == b` with `c`, which is rarely what's meant.
            // They're parsed as left-associative so that a chain gets a clear
            // error, reported once, at its second comparison.
            infix(
                left(3),
                comparison_op.map_with(|op, e| (op, Span::from(e.span()))),
                |lhs: Expr, (op, op_span): (BinOp, Span), rhs: Expr, e| {
                    if let ExprKind::Binary(first, ..) = &lhs.kind
                        && is_comparison(&lhs)
                        && !is_comparison(first)
                    {
                        e.emit(Rich::custom(
                            (op_span.start..rhs.span.end).into(),
                            "comparisons can't be chained, but can be combined with `&` or `|`",
                        ));
                    }
                    fold_binary(lhs, op, rhs, e)
                },
            ),
            infix(left(2), and_op, fold_binary),
            infix(left(1), or_op, fold_binary),
        ))
        .boxed()
    };

    let mut expr = Recursive::declare();

    let literal = select! {
        Token::Int(n) => ExprKind::Int(n),
        Token::Bool(b) => ExprKind::Bool(b),
    };

    // `label: value` or just `value`.
    let arg = ident
        .then_ignore(just(Token::Colon))
        .or_not()
        .then(expr.clone())
        .map_with(|(label, value), e| Arg {
            label,
            value,
            span: Span::from(e.span()),
        });
    let args = comma_list(arg, Token::LParen, Token::RParen);

    // A name followed by arguments is a call, otherwise it's a variable.
    let name_or_call = name
        .clone()
        .then(args.clone().or_not())
        .map(|(name, args)| match args {
            Some(args) => ExprKind::Call { callee: name, args },
            None => ExprKind::Var(name),
        });

    // `(value)` is a value in brackets, or a tuple with one unlabelled
    // element, which the checker decides. Anything else in brackets, such as
    // `()`, `(x: 1)` or `(1, 2)`, is a tuple.
    let parenthesised = args.clone().map(|mut values| match &values[..] {
        [Arg { label: None, .. }] => ExprKind::Paren(Box::new(values.remove(0).value)),
        _ => ExprKind::Tuple(values),
    });

    // `Self { field: value, ... }`. Every field must be labelled. Only `Self`
    // can be constructed, so in `if done { ... }`, the `{` always starts the
    // `if`'s block.
    let field_value = ident
        .then_ignore(just(Token::Colon))
        .then(expr.clone())
        .map_with(|(name, value), e| FieldValue {
            name,
            value,
            span: Span::from(e.span()),
        });
    let construct = just(Token::SelfType)
        .ignore_then(comma_list(field_value, Token::LBrace, Token::RBrace))
        .map(ExprKind::Construct);

    // `Optional<Int>` before a `.`, as in `Optional<Int>.none`. In `a < b`,
    // there's no `>` and `.`, so it's a comparison.
    let generic_type = name
        .clone()
        .then(type_args.clone())
        .then_ignore(just(Token::Dot).rewind())
        .map(|(name, args)| ExprKind::GenericType { name, args });

    let self_value = just(Token::SelfValue).to(ExprKind::SelfValue);

    // `.field` or `.method(args)` after any atom, which binds tighter than
    // any operator, so `-a.b` is `-(a.b)`. They can be chained, as in
    // `a.f().b`.
    let member = just(Token::Dot)
        .ignore_then(ident)
        .then(args.or_not())
        .map_with(|member, e| (member, Span::from(e.span())));
    let with_members = |atom: Boxed<'tok, 'tok, I, Expr, Extra<'tok, 'src>>| {
        atom.foldl(
            member.clone().repeated(),
            |receiver, ((name, args), end)| {
                let span = receiver.span.to(end);
                let receiver = Box::new(receiver);
                let kind = match args {
                    Some(args) => ExprKind::MethodCall {
                        receiver,
                        method: name,
                        args,
                    },
                    None => ExprKind::Field {
                        receiver,
                        field: name,
                    },
                };
                Expr { kind, span }
            },
        )
        .boxed()
    };

    expr.define(operators(with_members(
        literal
            .or(self_value)
            .or(construct)
            .or(generic_type)
            .or(name_or_call)
            .or(parenthesised)
            .map_with(|kind, e| Expr {
                kind,
                span: Span::from(e.span()),
            })
            .labelled("expression")
            .boxed(),
    )));

    // `_`, `let name`, a literal, a tuple of patterns, or an option written
    // like it's created, with a pattern for its value, as in
    // `Shape.circle.new(let radius)`.
    let pattern = recursive(|pattern| {
        let int = just(Token::Minus)
            .or_not()
            .then(select! { Token::Int(n) => n })
            .map(|(minus, value)| PatternKind::Int {
                value,
                negative: minus.is_some(),
            });
        let element = ident
            .then_ignore(just(Token::Colon))
            .or_not()
            .then(pattern.clone())
            .map(|(label, pattern)| TuplePattern { label, pattern });
        let new = select! { Token::Ident("new") => () };
        let option = named_type
            .clone()
            .then_ignore(just(Token::Dot))
            .then(ident)
            .then_ignore(just(Token::Dot))
            .then_ignore(new)
            .then(
                pattern
                    .clone()
                    .delimited_by(just(Token::LParen), just(Token::RParen)),
            )
            .map(|((ty, option), value)| PatternKind::Option {
                ty,
                option,
                value: Box::new(value),
            });
        choice((
            just(Token::Underscore).to(PatternKind::Any),
            just(Token::Let)
                .ignore_then(ident)
                .map(PatternKind::Binding),
            select! { Token::Bool(b) => PatternKind::Bool(b) },
            int,
            comma_list(element, Token::LParen, Token::RParen).map(PatternKind::Tuple),
            option,
        ))
        .map_with(|kind, e| Pattern {
            kind,
            span: Span::from(e.span()),
        })
        .labelled("pattern")
    })
    .boxed();

    let mutability = select! {
        Token::Let => Mutability::Let,
        Token::Var => Mutability::Var,
    };

    let annotation = just(Token::Colon).ignore_then(type_name.clone());

    let decl = mutability
        .then(ident)
        .then(annotation.clone().or_not())
        .then_ignore(just(Token::Eq))
        .then(expr.clone())
        .map_with(|(((mutability, name), ty), value), e| Stmt {
            kind: StmtKind::Decl {
                mutability,
                name,
                ty,
                value,
            },
            span: Span::from(e.span()),
        });

    let assign = ident
        .then_ignore(just(Token::Eq))
        .then(expr.clone())
        .map_with(|(name, value), e| Stmt {
            kind: StmtKind::Assign { name, value },
            span: Span::from(e.span()),
        });

    // `let name: Type` or `var name: Type`. A value is parsed only so that it
    // can be reported clearly.
    let field = mutability
        .then(ident)
        .then(annotation.clone())
        .then(just(Token::Eq).ignore_then(expr.clone()).or_not())
        .validate(|(((mutability, name), ty), value), e, emitter| {
            if value.is_some() {
                emitter.emit(Rich::custom(e.span(), "group fields can't have values"));
            }
            Field {
                mutability,
                name,
                ty,
                span: Span::from(e.span()),
            }
        });

    // `<T, U>` after a type's name, naming its generic parameters.
    let generic_params = ident
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just(Token::Lt), just(Token::Gt))
        .or_not()
        .map(Option::unwrap_or_default);

    // Like a block, one field per line.
    let group_ = just(Token::Group)
        .ignore_then(ident)
        .then(generic_params.clone())
        .map_with(|head, e| (head, Span::from(e.span())))
        .then(braced_lines(field))
        .map(|(((name, params), span), fields)| Stmt {
            kind: StmtKind::Group {
                name,
                params,
                fields,
            },
            span,
        });

    // `alias Name = Type`, or `alias Name` in a `trait`.
    let any_alias = just(Token::Alias)
        .ignore_then(ident)
        .then(just(Token::Eq).ignore_then(type_name.clone()).or_not())
        .map_with(|(name, ty), e| Alias {
            name,
            ty,
            span: Span::from(e.span()),
        })
        .boxed();
    let alias_in = |place: Place| {
        any_alias.clone().validate(move |alias, e, emitter| {
            if let Some(message) = place.alias_error(&alias) {
                emitter.emit(Rich::custom(e.span(), message));
            }
            alias
        })
    };

    // `name: Type`, which holds a value of that type, or `name`, which is
    // the same as `name: Void`, so it holds nothing useful.
    let option = ident
        .then(just(Token::Colon).ignore_then(type_name.clone()).or_not())
        .map_with(|(name, ty), e| {
            let span = Span::from(e.span());
            ChoiceOption {
                name,
                ty: ty.unwrap_or_else(|| void_type(span)),
                span,
            }
        });

    // Laid out like a group, with one option per line.
    let choice_ = just(Token::Choice)
        .ignore_then(ident)
        .then(generic_params.clone())
        .map_with(|head, e| (head, Span::from(e.span())))
        .then(braced_lines(option))
        .map(|(((name, params), span), options)| Stmt {
            kind: StmtKind::Choice {
                name,
                params,
                options,
            },
            span,
        });

    // Written like a tuple type, as in `(to value: Int)`.
    let params = comma_list(element, Token::LParen, Token::RParen);

    // `fn name(params) -> Type`, or `fn name(params)`, which returns `Void`.
    let signature = just(Token::Fn)
        .ignore_then(ident)
        .then(params)
        .then(just(Token::Arrow).ignore_then(type_name.clone()).or_not())
        .map_with(|((name, params), ret), e| {
            let span = Span::from(e.span());
            let ret = ret.unwrap_or_else(|| void_type(span));
            (name, params, ret, span)
        })
        .boxed();

    // One statement per line. Only the top level can have a `primitive fn`.
    let mut stmt = Recursive::declare();
    let function_in = {
        let block = braced_lines(stmt.clone().validate(|stmt: Stmt, e, emitter| {
            if let StmtKind::Fn(Function { body: None, .. }) = &stmt.kind {
                emitter.emit(Rich::custom(
                    e.span(),
                    "only the top level can have a `primitive fn`",
                ));
            }
            stmt
        }))
        .boxed();

        // The `{` must be on the same line as the `if`, and `else` on the same
        // line as the preceding `}`, so newlines never need disambiguating.
        let if_ = recursive(|if_| {
            let else_block =
                just(Token::Else).ignore_then(block.clone().or(if_.map(|stmt| vec![stmt])));

            just(Token::If)
                .ignore_then(expr.clone())
                .map_with(|cond, e| (cond, Span::from(e.span())))
                .then(block.clone())
                .then(else_block.or_not())
                .map(|(((cond, span), then_block), else_block)| Stmt {
                    kind: StmtKind::If {
                        cond,
                        then_block,
                        else_block,
                    },
                    span,
                })
        });

        // A pattern, then its block, with the `{` on the same line.
        let match_arm = pattern
            .clone()
            .then_ignore(just(Token::FatArrow))
            .then(block.clone())
            .map(|(pattern, body)| MatchArm { pattern, body });

        // Like a block, one arm per line.
        let match_ = just(Token::Match)
            .ignore_then(expr.clone())
            .map_with(|value, e| (value, Span::from(e.span())))
            .then(braced_lines(match_arm))
            .map(|((value, span), arms)| Stmt {
                kind: StmtKind::Match { value, arms },
                span,
            });

        // `fn name(params) -> Type { ... }`, which can be a `static fn` in an
        // `impl` or `trait`, or a `primitive fn`, which has no body. In a
        // `trait`, its body is optional.
        // It has a body only if one starts, so a body with an error isn't
        // left out instead.
        let body = block
            .clone()
            .map(Some)
            .or(just(Token::LBrace).not().to(None));
        let any_function = just(Token::Static)
            .or_not()
            .then(just(Token::Primitive).or_not())
            .then(signature.clone())
            .then(body)
            .map(
                |(((is_static, primitive), (name, params, ret, span)), body)| {
                    let function = Function {
                        is_static: is_static.is_some(),
                        name,
                        params,
                        ret,
                        body,
                        span,
                    };
                    (function, primitive.is_some())
                },
            )
            .boxed();
        let function_in = move |place: Place| {
            any_function
                .clone()
                .validate(move |(function, is_primitive), e, emitter| {
                    if let Some(message) = place.function_error(&function, is_primitive) {
                        emitter.emit(Rich::custom(e.span(), message));
                    }
                    function
                })
        };

        let return_ = just(Token::Return)
            .ignore_then(expr.clone().or_not())
            .map_with(|value, e| Stmt {
                kind: StmtKind::Return(value),
                span: Span::from(e.span()),
            });

        // Only calls are useful on their own, since any other expression's
        // value would be thrown away.
        let call = expr.clone().try_map(|expr, span| match expr.kind {
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => Ok(Stmt {
                span: expr.span,
                kind: StmtKind::Expr(expr),
            }),
            _ => Err(Rich::custom(
                span,
                "only function calls can be used as statements",
            )),
        });

        // A tuple pattern, then a value for it to match, as in
        // `(let a, let b) = (y, x)`. Only a tuple pattern starts with `(`,
        // which a call such as `(x: 1).f()` can too, so it's tried first.
        let destructure = just(Token::LParen)
            .rewind()
            .ignore_then(pattern.clone())
            .then_ignore(just(Token::Eq))
            .then(expr.clone())
            .map_with(|(pattern, value), e| Stmt {
                kind: StmtKind::Destructure { pattern, value },
                span: Span::from(e.span()),
            });

        stmt.define(
            decl.clone()
                .or(assign)
                .or(destructure)
                .or(if_)
                .or(match_)
                .or(function_in(Place::Stmt).map(|function| Stmt {
                    span: function.span,
                    kind: StmtKind::Fn(function),
                }))
                .or(group_)
                .or(choice_)
                .or(alias_in(Place::Stmt).map(|alias| Stmt {
                    span: alias.span,
                    kind: StmtKind::Alias(alias),
                }))
                .or(return_)
                .or(call),
        );
        function_in
    };

    // `use /dir/other`, which can have an alias, as in `use /dir/other as
    // renamed`, or `use /dir/`, for a module. `as` only means this here, so
    // it can still be used as a name or label, as in `isSame(as: other)`.
    // The path is one token, so anything else after it on the line means it
    // contains a space or comment.
    let path = select! { Token::Path(path) => path }
        .map_with(|path, e| (path, e.span()))
        .labelled("path");
    let alias = just(Token::Ident("as")).ignore_then(ident).or_not();
    let rest_of_line = any()
        .and_is(just(Token::Newline).not())
        .map_with(|_, e| e.span())
        .repeated()
        .collect::<Vec<SimpleSpan>>();
    let use_ = just(Token::Use)
        .ignore_then(path)
        .then(alias)
        .then(rest_of_line)
        .validate(|(((path, path_span), alias), rest), e, emitter| {
            if let (Some(first), Some(last)) = (rest.first(), rest.last()) {
                let message = match alias {
                    None => "paths can't contain spaces or comments",
                    Some(_) => "expected the end of the line after the alias",
                };
                emitter.emit(Rich::custom((first.start..last.end).into(), message));
            }
            let import = match (import(path), alias) {
                (Some(Import::File { path, .. }), alias) => Import::File { path, alias },
                (Some(module @ Import::Module(_)), None) => module,
                (Some(module @ Import::Module(_)), Some(_)) => {
                    emitter.emit(Rich::custom(
                        e.span(),
                        format!("`{module}` is a module, so it can't have an alias"),
                    ));
                    module
                }
                (None, _) => {
                    emitter.emit(Rich::custom(
                        path_span,
                        format!(
                            "`{path}` isn't a path, such as `./dir/other`, `../other`, \
                             `/dir/` or `stdlib/core`"
                        ),
                    ));
                    Import::Module(ImportPath::Named(path.to_string()))
                }
            };
            Stmt {
                kind: StmtKind::Use(import),
                span: Span::from(e.span()),
            }
        });

    // `impl Type { fn ... }`, with one function per line, like a block. Each
    // can be a method, or a `static fn`, and the standard library's can be a
    // `primitive fn`, which has no body.
    let impl_member = alias_in(Place::Impl)
        .map(Member::Alias)
        .or(function_in(Place::Impl).map(Member::Function));
    // `with` is only special here, so it can still be a label. Only a named
    // type can have an `impl`, and only a trait can follow `with`.
    let impl_ = just(Token::Impl)
        .ignore_then(name.clone().labelled("type"))
        .then(
            just(Token::Ident("with"))
                .ignore_then(name.clone().labelled("trait"))
                .or_not(),
        )
        .map_with(|head, e| (head, Span::from(e.span())))
        .then(braced_lines(impl_member))
        .map(|(((ty, with), span), members)| {
            let (aliases, functions) = split(members);
            Stmt {
                kind: StmtKind::Impl {
                    ty,
                    with,
                    aliases,
                    functions,
                },
                span,
            }
        });

    // Like an `impl`, one function per line, but each only needs its
    // signature. A body is its default implementation. Each alias is only
    // named, since each `impl` gives its type.
    let trait_member = alias_in(Place::Trait)
        .map(Member::Alias)
        .or(function_in(Place::Trait).map(Member::Function));
    let trait_ = just(Token::Trait)
        .ignore_then(ident)
        .map_with(|name, e| (name, Span::from(e.span())))
        .then(braced_lines(trait_member))
        .map(|((name, span), members)| {
            let (aliases, functions) = split(members);
            Stmt {
                kind: StmtKind::Trait {
                    name,
                    aliases,
                    functions,
                },
                span,
            }
        });

    // `primitive Name` or `primitive Name<T, U>`, which the checker only
    // allows in the standard library, like a `primitive fn`.
    let primitive = just(Token::Primitive)
        .ignore_then(ident)
        .then(generic_params)
        .map_with(|(name, params), e| Stmt {
            kind: StmtKind::Primitive { name, params },
            span: Span::from(e.span()),
        });

    // The top level can only declare things, so a file never runs code
    // just by being imported. Other statements are parsed only so that they
    // can be reported clearly.
    let top_level_stmt =
        use_.or(impl_)
            .or(trait_)
            .or(primitive)
            .or(stmt.validate(|stmt, e, emitter| {
                if !matches!(
                    stmt.kind,
                    StmtKind::Decl { .. }
                        | StmtKind::Fn(_)
                        | StmtKind::Group { .. }
                        | StmtKind::Choice { .. }
                        | StmtKind::Alias { .. }
                ) {
                    emitter.emit(Rich::custom(
                        e.span(),
                        "only declarations can be used at the top level",
                    ));
                }
                stmt
            }));

    lines(top_level_stmt).then_ignore(end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `src` with every span empty, like the ASTs these tests expect,
    /// so that they're compared by what they say rather than where it's
    /// written. `super::parse` keeps the real spans.
    fn parse(src: &str) -> Result<Program, Vec<ParseError<'_>>> {
        let tokens = lexer::tokens(src).map(|(tok, _)| (tok, (0..0).into()));
        parse_tokens(tokens, 0)
    }

    fn expr_node(kind: ExprKind) -> Expr {
        Expr {
            kind,
            span: Span::default(),
        }
    }

    fn stmt_node(kind: StmtKind) -> Stmt {
        Stmt {
            kind,
            span: Span::default(),
        }
    }

    fn pattern_node(kind: PatternKind) -> Pattern {
        Pattern {
            kind,
            span: Span::default(),
        }
    }

    /// Parses `src` as the body of a function, since statements other than
    /// declarations aren't allowed at the top level.
    fn parse_body(src: &str) -> Result<Vec<Stmt>, Vec<String>> {
        match parse(&format!("fn body() {{\n{src}\n}}")) {
            Ok(mut program) => match program.remove(0).kind {
                StmtKind::Fn(Function {
                    body: Some(body), ..
                }) => Ok(body),
                stmt => panic!("expected a function, got {stmt:?}"),
            },
            Err(errs) => Err(errs.iter().map(ToString::to_string).collect()),
        }
    }

    fn int(n: u64) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Int(n)))
    }

    fn var(name: &str) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Var(Name::new(name))))
    }

    fn assign(name: &str, value: Expr) -> Stmt {
        stmt_node(StmtKind::Assign {
            name: name.to_string(),
            value,
        })
    }

    /// Parses `src`, keeping its real spans.
    fn parse_spanned(src: &str) -> Program {
        super::parse(src).unwrap_or_else(|errs| panic!("{src:?} should parse: {errs:?}"))
    }

    /// The source that `span` points at in `src`.
    fn text(src: &str, span: Span) -> &str {
        &src[span.start..span.end]
    }

    /// The source that the value of `let x = {value}` points at.
    fn spanned_value(value: &str) -> String {
        let src = format!("let x = {value}");
        let program = parse_spanned(&src);
        match &program[0].kind {
            StmtKind::Decl { value, .. } => text(&src, value.span).to_string(),
            stmt => panic!("expected a declaration, got {stmt:?}"),
        }
    }

    #[test]
    fn spans_tell_apart_the_same_code_in_different_places() {
        assert_ne!(Span { start: 0, end: 1 }, Span { start: 0, end: 2 });
        assert_ne!(parse_spanned("let x = 1"), parse_spanned("let x  = 1"));
        assert_eq!(parse_spanned("let x = 1"), parse_spanned("let x = 1"));
    }

    #[test]
    fn expressions_span_all_of_their_source() {
        for value in [
            "1",
            "true",
            "self",
            "name",
            "other::name",
            "-1",
            "!x",
            "- - x",
            "1 + 2",
            "1 + 2 * 3",
            "(1 + 2) * 3",
            "1 < 2",
            "a & b | c",
            "f()",
            "f(to: 1, 2)",
            "(1)",
            "()",
            "(x: 1, y: 2)",
            "a.b",
            "a.b.c",
            "a.f(to: 1).b",
            "-a.b",
            "Self { x: 1 }",
            "Box<Int>.none",
        ] {
            assert_eq!(spanned_value(value), value);
        }
    }

    #[test]
    fn nested_expressions_span_their_own_source() {
        let src = "let x = (1 + 2) * -a.b";
        let mut program = parse_spanned(src);
        let StmtKind::Decl { value, .. } = program.remove(0).kind else {
            panic!("expected a declaration");
        };
        let ExprKind::Binary(lhs, _, rhs) = value.kind else {
            panic!("expected a binary operator");
        };
        assert_eq!(text(src, lhs.span), "(1 + 2)");
        assert_eq!(text(src, rhs.span), "-a.b");
        let ExprKind::Paren(inner) = lhs.kind else {
            panic!("expected brackets");
        };
        assert_eq!(text(src, inner.span), "1 + 2");
        let ExprKind::Unary(_, operand) = rhs.kind else {
            panic!("expected a unary operator");
        };
        assert_eq!(text(src, operand.span), "a.b");
        let ExprKind::Field { receiver, .. } = operand.kind else {
            panic!("expected a field");
        };
        assert_eq!(text(src, receiver.span), "a");
    }

    #[test]
    fn arguments_and_field_values_span_their_labels() {
        let src = "let x = f(to: 1 + 2, 3)";
        let mut program = parse_spanned(src);
        let StmtKind::Decl { value, .. } = program.remove(0).kind else {
            panic!("expected a declaration");
        };
        let ExprKind::Call { callee, args } = value.kind else {
            panic!("expected a call");
        };
        assert_eq!(text(src, callee.span), "f");
        assert_eq!(text(src, args[0].span), "to: 1 + 2");
        assert_eq!(text(src, args[0].value.span), "1 + 2");
        assert_eq!(text(src, args[1].span), "3");

        let src = "let x = Self { x: 1, y: 2 }";
        let mut program = parse_spanned(src);
        let StmtKind::Decl { value, .. } = program.remove(0).kind else {
            panic!("expected a declaration");
        };
        let ExprKind::Construct(fields) = value.kind else {
            panic!("expected a group being constructed");
        };
        assert_eq!(text(src, fields[1].span), "y: 2");
        assert_eq!(text(src, fields[1].value.span), "2");
    }

    #[test]
    fn statements_without_blocks_span_all_of_their_source() {
        for stmt in [
            "let x = 1",
            "var x: Int = 1",
            "x = 1",
            "(let a, let b) = (1, 2)",
            "alias Number = Int",
            "return",
            "return 1",
            "f(to: 1)",
            "a.f()",
        ] {
            let src = format!("fn body() {{\n{stmt}\n}}");
            let program = parse_spanned(&src);
            let StmtKind::Fn(function) = &program[0].kind else {
                panic!("expected a function");
            };
            assert_eq!(
                text(&src, function.body.as_ref().expect("has a body")[0].span),
                stmt
            );
        }
        for stmt in [
            "use /dir/other",
            "use /dir/other as renamed",
            "use ./",
            "use ./users/",
            "use ../users/other",
            "use ../../",
            "primitive Int",
            "primitive Fn<T, U>",
        ] {
            let program = parse_spanned(stmt);
            assert_eq!(text(stmt, program[0].span), stmt);
        }
    }

    #[test]
    fn statements_with_blocks_span_up_to_their_blocks() {
        for (stmt, head) in [
            ("if a + b {}", "if a + b"),
            ("if a {} else {}", "if a"),
            ("match a {}", "match a"),
            ("fn f() {}", "fn f()"),
            (
                "fn f(to value: Int) -> Int {}",
                "fn f(to value: Int) -> Int",
            ),
            ("group Point {}", "group Point"),
            ("group Box<T> {}", "group Box<T>"),
            ("choice Answer {}", "choice Answer"),
            ("choice Optional<T> {}", "choice Optional<T>"),
        ] {
            let src = format!("fn body() {{\n{stmt}\n}}");
            let program = parse_spanned(&src);
            let StmtKind::Fn(function) = &program[0].kind else {
                panic!("expected a function");
            };
            assert_eq!(
                text(&src, function.body.as_ref().expect("has a body")[0].span),
                head
            );
        }
        for (stmt, head) in [
            ("impl Point {}", "impl Point"),
            ("impl Point with Named {}", "impl Point with Named"),
            ("trait Named {}", "trait Named"),
        ] {
            let program = parse_spanned(stmt);
            assert_eq!(text(stmt, program[0].span), head);
        }
    }

    #[test]
    fn else_if_spans_up_to_its_own_block() {
        let src = "fn body() {\nif a {} else if b {}\n}";
        let program = parse_spanned(src);
        let StmtKind::Fn(function) = &program[0].kind else {
            panic!("expected a function");
        };
        let StmtKind::If {
            else_block: Some(else_block),
            ..
        } = &function.body.as_ref().expect("has a body")[0].kind
        else {
            panic!("expected an `if` with an `else`");
        };
        assert_eq!(text(src, else_block[0].span), "if b");
    }

    #[test]
    fn parts_of_declarations_span_their_own_source() {
        let src = "
            fn f(to value: Int, other: Bool) {}
            group Point {
                let x: Int
                var y: other::Number
            }
            choice Answer {
                yes
                no: other::Reason
            }
        ";
        let program = parse_spanned(src);
        let StmtKind::Fn(function) = &program[0].kind else {
            panic!("expected a function");
        };
        assert_eq!(text(src, function.params[0].span), "to value: Int");
        assert_eq!(text(src, function.params[1].span), "other: Bool");
        let StmtKind::Group { fields, .. } = &program[1].kind else {
            panic!("expected a group");
        };
        assert_eq!(text(src, fields[0].span), "let x: Int");
        assert_eq!(text(src, fields[1].span), "var y: other::Number");
        let TypeName::Named(name) = &fields[1].ty else {
            panic!("expected a named type");
        };
        assert_eq!(text(src, name.span), "other::Number");
        let StmtKind::Choice { options, .. } = &program[2].kind else {
            panic!("expected a choice");
        };
        assert_eq!(text(src, options[0].span), "yes");
        // Its `Void` is where it would be written.
        let TypeName::Named(name) = &options[0].ty else {
            panic!("expected a named type");
        };
        assert_eq!(text(src, name.span), "yes");
        assert_eq!(text(src, options[1].span), "no: other::Reason");
        let TypeName::Named(name) = &options[1].ty else {
            panic!("expected a named type");
        };
        assert_eq!(text(src, name.span), "other::Reason");
    }

    #[test]
    fn parts_of_impls_and_traits_span_their_own_source() {
        let src = "
            impl Point with Named {
                alias Rhs = Int
                static fn f() -> Int {}
            }
            trait Named {
                fn name() -> Int
                fn other() {}
            }
        ";
        let program = parse_spanned(src);
        let StmtKind::Impl {
            ty,
            with,
            aliases,
            functions,
        } = &program[0].kind
        else {
            panic!("expected an `impl`");
        };
        assert_eq!(text(src, ty.span), "Point");
        assert_eq!(text(src, with.as_ref().expect("has a trait").span), "Named");
        assert_eq!(text(src, aliases[0].span), "alias Rhs = Int");
        assert_eq!(text(src, functions[0].span), "fn f() -> Int");
        let StmtKind::Trait { functions, .. } = &program[1].kind else {
            panic!("expected a trait");
        };
        assert_eq!(text(src, functions[0].span), "fn name() -> Int");
        assert_eq!(text(src, functions[1].span), "fn other()");
    }

    #[test]
    fn types_that_are_not_written_span_what_they_belong_to() {
        let src = "
            fn f() {}
        ";
        let program = parse_spanned(src);
        let StmtKind::Fn(function) = &program[0].kind else {
            panic!("expected a function");
        };
        let TypeName::Named(name) = &function.ret else {
            panic!("expected a named type");
        };
        assert_eq!(text(src, name.span), "fn f()");
    }

    #[test]
    fn self_type_names_and_params_span_their_source() {
        let src = "
            impl Box {
                fn f(other: Self) -> Self.T {}
            }
        ";
        let program = parse_spanned(src);
        let StmtKind::Impl { functions, .. } = &program[0].kind else {
            panic!("expected an `impl`");
        };
        let function = &functions[0];
        let TypeName::Named(name) = &function.params[0].ty else {
            panic!("expected a named type");
        };
        assert!(name.is_self());
        assert_eq!(text(src, name.span), "Self");
        let TypeName::SelfParam(_, span) = &function.ret else {
            panic!("expected `Self.T`");
        };
        assert_eq!(text(src, *span), "Self.T");
    }

    #[test]
    fn patterns_span_their_own_source() {
        let src = "
            fn f() {
                match a {
                    (x: let b, y: Shape.circle.new(1)) => {}
                }
            }
        ";
        let program = parse_spanned(src);
        let StmtKind::Fn(function) = &program[0].kind else {
            panic!("expected a function");
        };
        let StmtKind::Match { arms, .. } = &function.body.as_ref().expect("has a body")[0].kind
        else {
            panic!("expected a match");
        };
        let pattern = &arms[0].pattern;
        assert_eq!(
            text(src, pattern.span),
            "(x: let b, y: Shape.circle.new(1))"
        );
        let PatternKind::Tuple(elements) = &pattern.kind else {
            panic!("expected a tuple");
        };
        assert_eq!(text(src, elements[0].pattern.span), "let b");
        let option = &elements[1].pattern;
        assert_eq!(text(src, option.span), "Shape.circle.new(1)");
        let PatternKind::Option { value, .. } = &option.kind else {
            panic!("expected an option");
        };
        assert_eq!(text(src, value.span), "1");
    }

    #[test]
    fn spans_count_bytes() {
        let src = "/* é 日本語 */ let x = 1";
        let program = parse_spanned(src);
        assert_eq!(text(src, program[0].span), "let x = 1");
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
                assign("x", expr_node(ExprKind::Binary(int(1), BinOp::Add, int(2)))),
                assign(
                    "y",
                    expr_node(ExprKind::Binary(var("x"), BinOp::Sub, int(3)))
                ),
            ])
        );
    }

    #[test]
    fn allows_blank_lines() {
        assert_eq!(
            parse_body("\n\nx = 1\n\n\ny = 2\n\n"),
            Ok(vec![
                assign("x", expr_node(ExprKind::Int(1))),
                assign("y", expr_node(ExprKind::Int(2))),
            ])
        );
    }

    #[test]
    fn binary_ops_are_left_associative() {
        assert_eq!(
            parse_body("x = 1 - 2 + 3"),
            Ok(vec![assign(
                "x",
                expr_node(ExprKind::Binary(
                    Box::new(expr_node(ExprKind::Binary(int(1), BinOp::Sub, int(2)))),
                    BinOp::Add,
                    int(3),
                )),
            )])
        );
    }

    /// `expr` in brackets, which the checker decides is either just `expr`,
    /// or a tuple holding it.
    fn paren(expr: Box<Expr>) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Paren(expr)))
    }

    fn decl_type(src: &str) -> TypeName {
        match parse_body(src).unwrap().remove(0).kind {
            StmtKind::Decl { ty: Some(ty), .. } => ty,
            stmt => panic!("expected a declaration with a type, got {stmt:?}"),
        }
    }

    #[test]
    fn parses_tuple_types() {
        let element = |label: Option<&str>, name: Option<&str>, ty| TupleElement {
            label: label.map(str::to_string),
            name: name.map(str::to_string),
            ty,
            span: Span::default(),
        };
        assert_eq!(
            decl_type("let t: (at x: Int, y: Bool, _: Int, _ z: Int) = t"),
            TypeName::Tuple(vec![
                element(Some("at"), Some("x"), TypeName::new("Int")),
                element(Some("y"), Some("y"), TypeName::new("Bool")),
                element(None, None, TypeName::new("Int")),
                element(None, Some("z"), TypeName::new("Int")),
            ])
        );
        assert_eq!(decl_type("let t: () = t"), TypeName::Tuple(vec![]));
        assert_eq!(
            decl_type("let t: (inner: (x: Int)) = t"),
            TypeName::Tuple(vec![element(
                Some("inner"),
                Some("inner"),
                TypeName::Tuple(vec![element(Some("x"), Some("x"), TypeName::new("Int"))]),
            )])
        );
        for src in [
            "let t: (Int) = t",
            "let t: (x) = t",
            "let t: (x: ) = t",
            "let t: (a b c: Int) = t",
            "let t: (_ _: Int) = t",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_generic_declarations() {
        assert_eq!(
            parse("choice Optional<T> {\nnone: Void\nsome: T\n}\ngroup Pair<A, B> {}"),
            Ok(vec![
                stmt_node(StmtKind::Choice {
                    name: "Optional".to_string(),
                    params: vec!["T".to_string()],
                    options: vec![
                        option("none", TypeName::new("Void")),
                        option("some", TypeName::new("T")),
                    ],
                }),
                stmt_node(StmtKind::Group {
                    name: "Pair".to_string(),
                    params: vec!["A".to_string(), "B".to_string()],
                    fields: vec![],
                }),
            ])
        );
        for src in [
            "group Box<> {}",
            "group Box<T {}",
            "group Box<Int<T>> {}",
            "group Box<T,> {}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_generic_types() {
        let generic = |name: &str, args| TypeName::Generic {
            name: Name::new(name),
            args,
        };
        assert_eq!(
            decl_type("let t: Pair<Int, Optional<(x: Int)>> = t"),
            generic(
                "Pair",
                vec![
                    TypeName::new("Int"),
                    generic(
                        "Optional",
                        vec![TypeName::Tuple(vec![TupleElement {
                            label: Some("x".to_string()),
                            name: Some("x".to_string()),
                            ty: TypeName::new("Int"),
                            span: Span::default(),
                        }])],
                    ),
                ],
            )
        );
        assert_eq!(
            decl_type("let t: Self.T = t"),
            TypeName::SelfParam("T".to_string(), Span::default())
        );
        for src in ["let t: Box<> = t", "let t: Box<Int = t", "let t: Self. = t"] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_generic_types_in_expressions() {
        assert_eq!(
            parse_expr("Optional<Int>.none"),
            field_access(
                expr_node(ExprKind::GenericType {
                    name: Name::new("Optional"),
                    args: vec![TypeName::new("Int")],
                }),
                "none"
            )
        );
        // Without a `.` after the `>`, they're comparisons.
        assert_eq!(parse_expr("a < b"), *bin(var("a"), BinOp::Lt, var("b")));
        assert_eq!(
            parse_expr("f(a < b, c > d)"),
            *call(
                "f",
                vec![
                    arg(None, *bin(var("a"), BinOp::Lt, var("b"))),
                    arg(None, *bin(var("c"), BinOp::Gt, var("d"))),
                ]
            )
        );
        assert_eq!(
            parse_body("if a < b {}"),
            Ok(vec![if_(*bin(var("a"), BinOp::Lt, var("b")), vec![], None)])
        );
    }

    #[test]
    fn parses_tuple_values() {
        assert_eq!(parse_expr("()"), expr_node(ExprKind::Tuple(vec![])));
        assert_eq!(
            parse_expr("(x: 1)"),
            expr_node(ExprKind::Tuple(vec![arg(
                Some("x"),
                expr_node(ExprKind::Int(1))
            )]))
        );
        assert_eq!(
            parse_expr("(at: 1, y: true, 2)"),
            expr_node(ExprKind::Tuple(vec![
                arg(Some("at"), expr_node(ExprKind::Int(1))),
                arg(Some("y"), expr_node(ExprKind::Bool(true))),
                arg(None, expr_node(ExprKind::Int(2))),
            ]))
        );
        // A value in brackets might be a tuple, which the checker decides.
        assert_eq!(parse_expr("(1)"), *paren(int(1)));
        assert_eq!(
            parse_expr("(x: 1).x"),
            field_access(
                expr_node(ExprKind::Tuple(vec![arg(
                    Some("x"),
                    expr_node(ExprKind::Int(1))
                )])),
                "x"
            )
        );
    }

    #[test]
    fn parses_patterns() {
        let pattern = |src: &str| match parse_body(&format!("match v {{\n{src} => {{}}\n}}")) {
            Ok(mut stmts) => match stmts.remove(0).kind {
                StmtKind::Match { mut arms, .. } => arms.remove(0).pattern,
                stmt => panic!("expected a match, got {stmt:?}"),
            },
            Err(errs) => panic!("{src:?} should parse: {errs:?}"),
        };
        let element = |label: Option<&str>, pattern| TuplePattern {
            label: label.map(str::to_string),
            pattern,
        };
        assert_eq!(pattern("_"), pattern_node(PatternKind::Any));
        assert_eq!(
            pattern("let x"),
            pattern_node(PatternKind::Binding("x".to_string()))
        );
        assert_eq!(pattern("true"), pattern_node(PatternKind::Bool(true)));
        assert_eq!(
            pattern("-12"),
            pattern_node(PatternKind::Int {
                value: 12,
                negative: true,
            })
        );
        assert_eq!(pattern("()"), pattern_node(PatternKind::Tuple(vec![])));
        assert_eq!(
            pattern("Optional<Int>.none.new(())"),
            pattern_node(PatternKind::Option {
                ty: TypeName::Generic {
                    name: Name::new("Optional"),
                    args: vec![TypeName::new("Int")],
                },
                option: "none".to_string(),
                value: Box::new(pattern_node(PatternKind::Tuple(vec![]))),
            })
        );
        assert_eq!(
            pattern("(x: 0, y: let y, _)"),
            pattern_node(PatternKind::Tuple(vec![
                element(
                    Some("x"),
                    pattern_node(PatternKind::Int {
                        value: 0,
                        negative: false,
                    })
                ),
                element(
                    Some("y"),
                    pattern_node(PatternKind::Binding("y".to_string()))
                ),
                element(None, pattern_node(PatternKind::Any)),
            ]))
        );
        assert_eq!(
            pattern("Shape.pair.new((left: Shape.empty.new(_), right: _))"),
            pattern_node(PatternKind::Option {
                ty: TypeName::new("Shape"),
                option: "pair".to_string(),
                value: Box::new(pattern_node(PatternKind::Tuple(vec![
                    element(
                        Some("left"),
                        pattern_node(PatternKind::Option {
                            ty: TypeName::new("Shape"),
                            option: "empty".to_string(),
                            value: Box::new(pattern_node(PatternKind::Any)),
                        })
                    ),
                    element(Some("right"), pattern_node(PatternKind::Any)),
                ]))),
            })
        );
        for src in [
            "match v { x => {} }",
            "match v { let => {} }",
            "match v { (let x let y) => {} }",
            "match v { 1 + 1 => {} }",
            "match v { -true => {} }",
            "match v { (x: ) => {} }",
            // An option's value is always matched, with exactly one pattern.
            "match v { Shape.empty => {} }",
            "match v { Shape.circle => {} }",
            "match v { (Shape.empty, _) => {} }",
            "match v { Shape.empty.new => {} }",
            "match v { Shape.empty.new() => {} }",
            "match v { Shape.pair.new(_, _) => {} }",
            "match v { Shape.pair.new(left: _) => {} }",
            // Options are matched like they're created, with `new`.
            "match v { Shape.circle(_) => {} }",
            "match v { Shape.circle.make(_) => {} }",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_destructuring() {
        assert_eq!(
            parse_body("(let a, _) = (y, x)"),
            Ok(vec![stmt_node(StmtKind::Destructure {
                pattern: pattern_node(PatternKind::Tuple(vec![
                    TuplePattern {
                        label: None,
                        pattern: pattern_node(PatternKind::Binding("a".to_string())),
                    },
                    TuplePattern {
                        label: None,
                        pattern: pattern_node(PatternKind::Any),
                    },
                ])),
                value: expr_node(ExprKind::Tuple(vec![
                    arg(None, *var("y")),
                    arg(None, *var("x"))
                ])),
            })])
        );
        // A call on a tuple still starts with `(`.
        assert_eq!(
            parse_body("(x: 1).f()"),
            Ok(vec![stmt_node(StmtKind::Expr(method_call(
                expr_node(ExprKind::Tuple(vec![arg(
                    Some("x"),
                    expr_node(ExprKind::Int(1))
                )])),
                "f",
                vec![],
            )))])
        );
        // Only a tuple pattern can be assigned to, and only in a function.
        for src in [
            "_ = x",
            "Shape.a.new(let r) = s",
            "(let a) =",
            "(let a) = (1) = (2)",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
        assert!(parse("(let a, let b) = (1, 2)").is_err());
    }

    #[test]
    fn parentheses_group() {
        assert_eq!(
            parse_body("x = 1 - (2 + 3)"),
            Ok(vec![assign(
                "x",
                expr_node(ExprKind::Binary(
                    int(1),
                    BinOp::Sub,
                    paren(bin(int(2), BinOp::Add, int(3)))
                )),
            )])
        );
    }

    fn neg(expr: Box<Expr>) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Unary(UnOp::Neg, expr)))
    }

    #[test]
    fn parses_negation() {
        assert_eq!(
            parse_body("x = -5\ny = -y\nz = --(1)"),
            Ok(vec![
                assign("x", *neg(int(5))),
                assign("y", *neg(var("y"))),
                assign("z", *neg(neg(paren(int(1))))),
            ])
        );
    }

    #[test]
    fn subtraction_of_negative() {
        assert_eq!(
            parse_body("x = 1 - -2"),
            Ok(vec![assign(
                "x",
                expr_node(ExprKind::Binary(int(1), BinOp::Sub, neg(int(2))))
            )])
        );
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        assert_eq!(
            parse_body("x = 1 + 2 * 3 - 4 / 5"),
            Ok(vec![assign(
                "x",
                expr_node(ExprKind::Binary(
                    Box::new(expr_node(ExprKind::Binary(
                        int(1),
                        BinOp::Add,
                        Box::new(expr_node(ExprKind::Binary(int(2), BinOp::Mul, int(3)))),
                    ))),
                    BinOp::Sub,
                    Box::new(expr_node(ExprKind::Binary(int(4), BinOp::Div, int(5)))),
                )),
            )])
        );
    }

    #[test]
    fn division_is_left_associative() {
        assert_eq!(
            parse_body("x = 8 / 4 * 2"),
            Ok(vec![assign(
                "x",
                expr_node(ExprKind::Binary(
                    Box::new(expr_node(ExprKind::Binary(int(8), BinOp::Div, int(4)))),
                    BinOp::Mul,
                    int(2),
                )),
            )])
        );
    }

    #[test]
    fn negation_binds_tighter_than_multiplication() {
        assert_eq!(
            parse_body("x = -2 * 3"),
            Ok(vec![assign(
                "x",
                expr_node(ExprKind::Binary(neg(int(2)), BinOp::Mul, int(3)))
            )])
        );
    }

    #[test]
    fn parses_declarations() {
        assert_eq!(
            parse_body("let x = 1 + 2\nvar y = x\ny = 3\nlet letter = y"),
            Ok(vec![
                stmt_node(StmtKind::Decl {
                    mutability: Mutability::Let,
                    name: "x".to_string(),
                    ty: None,
                    value: expr_node(ExprKind::Binary(int(1), BinOp::Add, int(2))),
                }),
                stmt_node(StmtKind::Decl {
                    mutability: Mutability::Var,
                    name: "y".to_string(),
                    ty: None,
                    value: *var("x"),
                }),
                assign("y", expr_node(ExprKind::Int(3))),
                stmt_node(StmtKind::Decl {
                    mutability: Mutability::Let,
                    name: "letter".to_string(),
                    ty: None,
                    value: *var("y"),
                }),
            ])
        );
    }

    fn decl(mutability: Mutability, name: &str, ty: Option<&str>, value: Expr) -> Stmt {
        stmt_node(StmtKind::Decl {
            mutability,
            name: name.to_string(),
            ty: ty.map(TypeName::new),
            value,
        })
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
                decl(
                    Mutability::Let,
                    "x",
                    Some("Int"),
                    expr_node(ExprKind::Int(3))
                ),
                decl(
                    Mutability::Var,
                    "y",
                    Some("Bool"),
                    expr_node(ExprKind::Bool(true))
                ),
                decl(Mutability::Let, "z", None, expr_node(ExprKind::Int(1))),
                decl(Mutability::Let, "w", Some("Int"), *neg(var("x"))),
            ])
        );
    }

    #[test]
    fn parses_unknown_type_names() {
        // Whether a type exists is decided by the checker.
        assert_eq!(
            parse_body("let x: Foo = 1"),
            Ok(vec![decl(
                Mutability::Let,
                "x",
                Some("Foo"),
                expr_node(ExprKind::Int(1))
            )])
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
                assign("x", expr_node(ExprKind::Bool(true))),
                assign("y", expr_node(ExprKind::Bool(false))),
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
                expr_node(ExprKind::Binary(
                    neg(Box::new(expr_node(ExprKind::Bool(true)))),
                    BinOp::Add,
                    int(1)
                )),
            )])
        );
    }

    #[test]
    fn rejects_declaration_without_value() {
        assert!(parse_body("let x").is_err());
        assert!(parse_body("var x").is_err());
    }

    fn bin(lhs: Box<Expr>, op: BinOp, rhs: Box<Expr>) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Binary(lhs, op, rhs)))
    }

    fn not(expr: Box<Expr>) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Unary(UnOp::Not, expr)))
    }

    fn parse_expr(src: &str) -> Expr {
        match parse_body(&format!("x = {src}")).unwrap().remove(0).kind {
            StmtKind::Assign { value, .. } => value,
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
            *not(paren(bin(var("a"), BinOp::And, var("b"))))
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
    fn parses_bitwise_operators() {
        let bit_not = |expr| Box::new(expr_node(ExprKind::Unary(UnOp::BitNot, expr)));
        assert_eq!(
            parse_expr("a .& b"),
            *bin(var("a"), BinOp::BitAnd, var("b"))
        );
        assert_eq!(parse_expr("a .| b"), *bin(var("a"), BinOp::BitOr, var("b")));
        assert_eq!(
            parse_expr("a .^ b"),
            *bin(var("a"), BinOp::BitXor, var("b"))
        );
        assert_eq!(
            parse_expr("a .< b"),
            *bin(var("a"), BinOp::ShiftLeft, var("b"))
        );
        assert_eq!(
            parse_expr("a .> b"),
            *bin(var("a"), BinOp::ShiftRight, var("b"))
        );
        assert_eq!(parse_expr(".!a"), *bit_not(var("a")));
        assert_eq!(parse_expr(".!-a"), *bit_not(neg(var("a"))));
        assert_eq!(
            parse_expr(".!a .& b"),
            *bin(bit_not(var("a")), BinOp::BitAnd, var("b"))
        );
    }

    #[test]
    fn bitwise_operators_bind_between_arithmetic_and_comparisons() {
        // From tightest: arithmetic, shifts, `.&`, `.^`, `.|`, comparisons.
        assert_eq!(
            parse_expr("a .< b + 1"),
            *bin(
                var("a"),
                BinOp::ShiftLeft,
                bin(var("b"), BinOp::Add, int(1))
            )
        );
        assert_eq!(
            parse_expr("a .& b .< c"),
            *bin(
                var("a"),
                BinOp::BitAnd,
                bin(var("b"), BinOp::ShiftLeft, var("c"))
            )
        );
        assert_eq!(
            parse_expr("a .| b .^ c .& d"),
            *bin(
                var("a"),
                BinOp::BitOr,
                bin(
                    var("b"),
                    BinOp::BitXor,
                    bin(var("c"), BinOp::BitAnd, var("d"))
                ),
            )
        );
        assert_eq!(
            parse_expr("a .& b == c .| d"),
            *bin(
                bin(var("a"), BinOp::BitAnd, var("b")),
                BinOp::Eq,
                bin(var("c"), BinOp::BitOr, var("d")),
            )
        );
    }

    #[test]
    fn bitwise_operators_are_left_associative() {
        assert_eq!(
            parse_expr("a .< b .> c"),
            *bin(
                bin(var("a"), BinOp::ShiftLeft, var("b")),
                BinOp::ShiftRight,
                var("c"),
            )
        );
        assert_eq!(
            parse_expr("a .| b .| c"),
            *bin(
                bin(var("a"), BinOp::BitOr, var("b")),
                BinOp::BitOr,
                var("c")
            )
        );
    }

    #[test]
    fn parses_comparisons() {
        assert_eq!(parse_expr("a == b"), *bin(var("a"), BinOp::Eq, var("b")));
        assert_eq!(parse_expr("a != b"), *bin(var("a"), BinOp::Ne, var("b")));
        assert_eq!(parse_expr("a < b"), *bin(var("a"), BinOp::Lt, var("b")));
        assert_eq!(parse_expr("a <= b"), *bin(var("a"), BinOp::Le, var("b")));
        assert_eq!(parse_expr("a > b"), *bin(var("a"), BinOp::Gt, var("b")));
        assert_eq!(parse_expr("a >= b"), *bin(var("a"), BinOp::Ge, var("b")));
    }

    #[test]
    fn comparisons_bind_between_arithmetic_and_boolean_operators() {
        assert_eq!(
            parse_expr("a + 1 == b & c != d * 2"),
            *bin(
                bin(bin(var("a"), BinOp::Add, int(1)), BinOp::Eq, var("b")),
                BinOp::And,
                bin(var("c"), BinOp::Ne, bin(var("d"), BinOp::Mul, int(2))),
            )
        );
        assert_eq!(
            parse_expr("!a == b | c"),
            *bin(bin(not(var("a")), BinOp::Eq, var("b")), BinOp::Or, var("c"))
        );
    }

    #[test]
    fn rejects_chained_comparisons() {
        for src in [
            "x = a == b == c",
            "x = a != b == c",
            "x = a == b != c != d",
            "x = a < b < c",
            "x = a <= b == c",
            "x = a > b >= c",
        ] {
            let errs = parse_body(src).unwrap_err();
            assert_eq!(
                errs[0].to_string(),
                "comparisons can't be chained, but can be combined with `&` or `|`",
                "{src:?}"
            );
        }
    }

    #[test]
    fn reports_chained_comparisons_once() {
        for src in [
            "x = a == b == c == d",
            "x = a < b < c < d < e",
            "x = (a == b) == c == d",
            "x = a == (b == c) == d",
        ] {
            assert_eq!(parse_body(src).unwrap_err().len(), 1, "{src:?}");
        }
    }

    #[test]
    fn bracketed_comparisons_can_be_compared() {
        assert_eq!(
            parse_expr("(a == b) == c"),
            *bin(
                paren(bin(var("a"), BinOp::Eq, var("b"))),
                BinOp::Eq,
                var("c")
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
        stmt_node(StmtKind::If {
            cond,
            then_block,
            else_block,
        })
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
                vec![
                    assign("x", expr_node(ExprKind::Int(1))),
                    assign("y", expr_node(ExprKind::Int(2)))
                ],
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
                vec![assign("x", expr_node(ExprKind::Int(1)))],
                Some(vec![assign("x", expr_node(ExprKind::Int(2)))]),
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
                vec![assign("x", expr_node(ExprKind::Int(1)))],
                Some(vec![if_(
                    *var("b"),
                    vec![assign("x", expr_node(ExprKind::Int(2)))],
                    Some(vec![if_(
                        *var("c"),
                        vec![assign("x", expr_node(ExprKind::Int(3)))],
                        Some(vec![assign("x", expr_node(ExprKind::Int(4)))]),
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
                vec![assign("x", expr_node(ExprKind::Int(1)))],
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
                stmt_node(StmtKind::Decl {
                    mutability: Mutability::Let,
                    name: "a".to_string(),
                    ty: None,
                    value: expr_node(ExprKind::Bool(true)),
                }),
                if_(
                    *var("a"),
                    vec![if_(
                        *var("a"),
                        vec![stmt_node(StmtKind::Decl {
                            mutability: Mutability::Let,
                            name: "b".to_string(),
                            ty: None,
                            value: expr_node(ExprKind::Int(1)),
                        })],
                        None,
                    )],
                    None,
                ),
                assign("x", expr_node(ExprKind::Int(2))),
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
                stmt_node(StmtKind::Decl {
                    mutability: Mutability::Let,
                    name: "a".to_string(),
                    ty: None,
                    value: expr_node(ExprKind::Bool(true)),
                }),
                if_(
                    *var("a"),
                    vec![assign("x", expr_node(ExprKind::Int(1)))],
                    Some(vec![assign("x", expr_node(ExprKind::Int(2)))]),
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
                assign("x", expr_node(ExprKind::Binary(int(1), BinOp::Add, int(2)))),
                assign("y", expr_node(ExprKind::Int(3))),
            ])
        );
    }

    #[test]
    fn block_comments_spanning_lines_end_statements() {
        assert_eq!(
            parse_body("x = 1 /*\n*/ y = 2"),
            Ok(vec![
                assign("x", expr_node(ExprKind::Int(1))),
                assign("y", expr_node(ExprKind::Int(2)))
            ])
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
        let errs = super::parse("let x = 1 /* /* */").unwrap_err();
        assert_eq!(errs[0].span().into_range(), 10..18);
        assert!(parse("let x = 1 /* */ */").is_err());
    }

    #[test]
    fn comments_hide_the_rest_of_the_line() {
        // Everything after `//` is ignored, including what would be an error.
        assert_eq!(
            parse_body("x = 1 // y = 2 $ {"),
            Ok(vec![assign("x", expr_node(ExprKind::Int(1)))])
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
        let errs = super::parse("let x = 1 $ 2").unwrap_err();
        assert_eq!(errs[0].span().into_range(), 10..11);
    }

    /// A parameter with `label` and `name`, which are `None` for `_`.
    fn param(label: Option<&str>, name: Option<&str>, ty: &str) -> TupleElement {
        TupleElement {
            label: label.map(str::to_string),
            name: name.map(str::to_string),
            ty: TypeName::new(ty),
            span: Span::default(),
        }
    }

    fn arg(label: Option<&str>, value: Expr) -> Arg {
        Arg {
            label: label.map(str::to_string),
            value,
            span: Span::default(),
        }
    }

    fn call(callee: &str, args: Vec<Arg>) -> Box<Expr> {
        Box::new(expr_node(ExprKind::Call {
            callee: Name::new(callee),
            args,
        }))
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
            Ok(vec![stmt_node(StmtKind::Fn(Function {
                is_static: false,
                name: "addFive".to_string(),
                params: vec![param(Some("to"), Some("value"), "Int")],
                ret: TypeName::new("Int"),
                body: Some(vec![stmt_node(StmtKind::Return(Some(expr_node(
                    ExprKind::Binary(var("value"), BinOp::Add, int(5),)
                ))))]),
                span: Span::default(),
            }))])
        );
    }

    #[test]
    fn parses_parameter_label_forms() {
        assert_eq!(
            parse_body("fn f(a: Int, to b: Bool, _ c: Int, _: Int, at _: Int) {}"),
            Ok(vec![stmt_node(StmtKind::Fn(Function {
                is_static: false,
                name: "f".to_string(),
                params: vec![
                    // A single name is both the label and the parameter name.
                    param(Some("a"), Some("a"), "Int"),
                    param(Some("to"), Some("b"), "Bool"),
                    // `_` means no label, or no name.
                    param(None, Some("c"), "Int"),
                    param(None, None, "Int"),
                    param(Some("at"), None, "Int"),
                ],
                ret: TypeName::new("Void"),
                body: Some(vec![]),
                span: Span::default(),
            }))])
        );
        for src in ["fn f(_ _: Int) {}", "fn f(a b c: Int) {}", "fn f(a) {}"] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
        let errs = parse_body("fn f(_ _: Int) {}").unwrap_err();
        assert_eq!(errs[0], "`_ _` is written `_`, since neither has a name");
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
            Ok(vec![stmt_node(StmtKind::Fn(Function {
                is_static: false,
                name: "reset".to_string(),
                params: vec![],
                ret: TypeName::new("Void"),
                body: Some(vec![
                    assign("count", expr_node(ExprKind::Int(0))),
                    stmt_node(StmtKind::Return(None))
                ]),
                span: Span::default(),
            }))])
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
            Ok(vec![stmt_node(StmtKind::Expr(*call(
                "log",
                vec![arg(Some("message"), *int(1))]
            )))])
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
        assert_eq!(
            parse_body("return"),
            Ok(vec![stmt_node(StmtKind::Return(None))])
        );
        assert_eq!(
            parse_body("return 1 + 2"),
            Ok(vec![stmt_node(StmtKind::Return(Some(*bin(
                int(1),
                BinOp::Add,
                int(2)
            ))))])
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
            "fn f(_ _: Int) {}",
            "fn f(_ _ x: Int) {}",
            "fn f(Int) {}",
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
            ty: TypeName::new(ty),
            span: Span::default(),
        }
    }

    fn group_(name: &str, fields: Vec<Field>) -> Stmt {
        stmt_node(StmtKind::Group {
            name: name.to_string(),
            params: vec![],
            fields,
        })
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
            Ok(vec![stmt_node(StmtKind::Fn(Function {
                is_static: false,
                name: "f".to_string(),
                params: vec![],
                ret: TypeName::new("Void"),
                body: Some(vec![group_("Inner", vec![])]),
                span: Span::default(),
            }))])
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

    fn choice_(name: &str, options: Vec<ChoiceOption>) -> Stmt {
        stmt_node(StmtKind::Choice {
            name: name.to_string(),
            params: vec![],
            options,
        })
    }

    /// An option called `name` holding a value of type `ty`.
    fn option(name: &str, ty: TypeName) -> ChoiceOption {
        ChoiceOption {
            name: name.to_string(),
            ty,
            span: Span::default(),
        }
    }

    #[test]
    fn parses_choice() {
        assert_eq!(
            parse_body(
                "
                choice Shape {
                    circle: Int
                    empty: Void
                    rectangle: (width: Int, height: Int)
                    nested: Optional<Int>
                }
                "
            ),
            Ok(vec![choice_(
                "Shape",
                vec![
                    option("circle", TypeName::new("Int")),
                    option("empty", TypeName::new("Void")),
                    option(
                        "rectangle",
                        TypeName::Tuple(vec![
                            TupleElement {
                                label: Some("width".to_string()),
                                name: Some("width".to_string()),
                                ty: TypeName::new("Int"),
                                span: Span::default(),
                            },
                            TupleElement {
                                label: Some("height".to_string()),
                                name: Some("height".to_string()),
                                ty: TypeName::new("Int"),
                                span: Span::default(),
                            },
                        ]),
                    ),
                    option(
                        "nested",
                        TypeName::Generic {
                            name: Name::new("Optional"),
                            args: vec![TypeName::new("Int")],
                        },
                    ),
                ],
            )])
        );
    }

    #[test]
    fn parses_choice_layouts() {
        assert_eq!(
            parse_body("choice Never {}"),
            Ok(vec![choice_("Never", vec![])])
        );
        assert_eq!(
            parse_body("choice Unit { only: Void }"),
            Ok(vec![choice_(
                "Unit",
                vec![option("only", TypeName::new("Void"))]
            )])
        );
        // Without a type, it holds `Void`, like a function without a return
        // type returns it.
        assert_eq!(
            parse_body("choice Unit { only }"),
            parse_body("choice Unit { only: Void }")
        );
        assert_eq!(
            parse_body("choice Optional<T> {\nnone\nsome: T\n}"),
            parse_body("choice Optional<T> {\nnone: Void\nsome: T\n}")
        );
        assert_eq!(
            parse_body(
                "
                choice Result { // A comment.

                    // Another comment.
                    success: Int

                    failure : other::Error
                    split: (
                        first: Int,
                        second: Int,
                    )
                }
                "
            ),
            Ok(vec![choice_(
                "Result",
                vec![
                    option("success", TypeName::new("Int")),
                    option("failure", TypeName::Named(qualified("other", "Error"))),
                    option(
                        "split",
                        TypeName::Tuple(vec![
                            TupleElement {
                                label: Some("first".to_string()),
                                name: Some("first".to_string()),
                                ty: TypeName::new("Int"),
                                span: Span::default(),
                            },
                            TupleElement {
                                label: Some("second".to_string()),
                                name: Some("second".to_string()),
                                ty: TypeName::new("Int"),
                                span: Span::default(),
                            },
                        ]),
                    ),
                ],
            )])
        );
    }

    #[test]
    fn parses_choice_inside_blocks() {
        assert_eq!(
            parse_body(
                "
                fn f() {
                    choice Inner {}
                }
                "
            ),
            Ok(vec![stmt_node(StmtKind::Fn(Function {
                is_static: false,
                name: "f".to_string(),
                params: vec![],
                ret: TypeName::new("Void"),
                body: Some(vec![choice_("Inner", vec![])]),
                span: Span::default(),
            }))])
        );
    }

    #[test]
    fn rejects_malformed_choices() {
        for src in [
            // Missing parts.
            "choice {}",
            "choice Shape",
            "choice Shape {",
            "choice Shape { circle: }",
            "choice Shape { : Int }",
            // The old syntax, holding a tuple of what's in brackets.
            "choice Shape { circle(radius: Int) }",
            "choice Shape { empty() }",
            "choice Shape { circle(): Int }",
            // An option holds one type.
            "choice Shape { pair: Int, Int }",
            "choice Shape { pair: Int Bool }",
            "choice Shape { _: Int }",
            // Options aren't fields, and can't have values.
            "choice Shape { let circle: Int }",
            "choice Shape { circle = 1 }",
            "choice Shape { circle: Int = 1 }",
            "choice Shape { circle: 1 }",
            // Only options are allowed.
            "choice Shape { f() {} }",
            "choice Shape { fn f() {} }",
            "choice Shape { choice Inner {} }",
            // Separators.
            "choice Shape { circle: Int square: Int }",
            "choice Shape { circle: Int, square: Int }",
            "choice Shape { circle: Int; }",
            "choice Shape { circle empty }",
            "choice Shape { circle, empty }",
            // `{` must be on the same line.
            "choice Shape\n{}",
            // Statements after `}` need their own line.
            "choice Shape {} x = 1",
            // Keywords.
            "choice choice {}",
            "let choice = 1",
            "choice Shape { group: Int }",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    /// An arm matching the option `name` of `ty`, binding its value to
    /// `binding` if given, as in `Shape.circle.new(let radius)`, or matching
    /// any value otherwise, as in `Shape.empty.new(_)`.
    fn match_option(ty: &str, name: &str, binding: Option<&str>, body: Vec<Stmt>) -> MatchArm {
        let value = match binding {
            Some(binding) => PatternKind::Binding(binding.to_string()),
            None => PatternKind::Any,
        };
        MatchArm {
            pattern: pattern_node(PatternKind::Option {
                ty: TypeName::new(ty),
                option: name.to_string(),
                value: Box::new(pattern_node(value)),
            }),
            body,
        }
    }

    #[test]
    fn parses_match() {
        assert_eq!(
            parse_body(
                "
                match shape {
                    Shape.circle.new(let radius) => {
                        draw(radius)
                    }
                    Shape.empty.new(_) => {}
                }
                "
            ),
            Ok(vec![stmt_node(StmtKind::Match {
                value: *var("shape"),
                arms: vec![
                    match_option(
                        "Shape",
                        "circle",
                        Some("radius"),
                        vec![stmt_node(StmtKind::Expr(*call(
                            "draw",
                            vec![arg(None, *var("radius"))]
                        )))],
                    ),
                    match_option("Shape", "empty", None, vec![]),
                ],
            })])
        );
    }

    #[test]
    fn parses_match_layouts() {
        assert_eq!(
            parse_body("match s {}"),
            Ok(vec![stmt_node(StmtKind::Match {
                value: *var("s"),
                arms: vec![],
            })])
        );
        assert_eq!(
            parse_body("match s { A.a.new(_) => {} }"),
            Ok(vec![stmt_node(StmtKind::Match {
                value: *var("s"),
                arms: vec![match_option("A", "a", None, vec![])],
            })])
        );
        assert_eq!(
            parse_body(
                "
                match s.shape() { // A comment.

                    // Another comment.
                    other::Shape.circle.new( let r ) => { return r }

                    Shape.empty.new(_) => {
                    }
                }
                "
            ),
            Ok(vec![stmt_node(StmtKind::Match {
                value: method_call(*var("s"), "shape", vec![]),
                arms: vec![
                    MatchArm {
                        pattern: pattern_node(PatternKind::Option {
                            ty: TypeName::Named(qualified("other", "Shape",)),
                            option: "circle".to_string(),
                            value: Box::new(pattern_node(PatternKind::Binding("r".to_string()))),
                        }),
                        body: vec![stmt_node(StmtKind::Return(Some(*var("r"))))],
                    },
                    match_option("Shape", "empty", None, vec![]),
                ],
            })])
        );
    }

    #[test]
    fn rejects_malformed_matches() {
        for src in [
            // Missing parts.
            "match {}",
            "match s",
            "match s {",
            "match s { Shape.a.new(_) }",
            "match s { Shape.a.new(_) => }",
            "match s { Shape.a.new(_) {} }",
            "match s { Shape => {} }",
            "match s { Shape() => {} }",
            "match s { .a() => {} }",
            "match s { => {} }",
            // An option's value is always matched.
            "match s { Shape.a => {} }",
            "match s { Shape.a.new => {} }",
            // Only `let` bindings of a name are allowed.
            "match s { Shape.a.new(r) => {} }",
            "match s { Shape.a.new(var r) => {} }",
            "match s { Shape.a.new(let) => {} }",
            "match s { Shape.a.new(_)(_) => {} }",
            // Options need blocks.
            "match s { Shape.a.new(_) => f() }",
            // Separators.
            "match s { Shape.a.new(_) => {} Shape.b.new(_) => {} }",
            "match s { Shape.a.new(_) => {}, Shape.b.new(_) => {} }",
            // `{` must be on the same line.
            "match s\n{}",
            "match s { Shape.a.new(_) =>\n{} }",
            // Statements after `}` need their own line.
            "match s {} x = 1",
            // Keywords.
            "let match = 1",
            "match s { Shape.match.new(_) => {} }",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_match_at_the_top_level() {
        let errs = parse("match s {}").unwrap_err();
        assert_eq!(
            errs[0].to_string(),
            "only declarations can be used at the top level"
        );
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
        let errs = parse_body("x = f(,)").unwrap_err();
        assert_eq!(errs[0], "expected a value before `,`");
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
                choice C {}
                alias A = S
                "
            ),
            Ok(vec![
                use_file(ImportPath::Absolute(vec![
                    "dir".to_string(),
                    "other".to_string()
                ])),
                use_file(ImportPath::Relative(vec![
                    "dir".to_string(),
                    "other".to_string()
                ])),
                use_file(ImportPath::Named("stdlib".to_string())),
                decl(Mutability::Let, "x", None, expr_node(ExprKind::Int(1))),
                decl(
                    Mutability::Var,
                    "y",
                    Some("Int"),
                    expr_node(ExprKind::Int(2))
                ),
                stmt_node(StmtKind::Fn(Function {
                    is_static: false,
                    name: "f".to_string(),
                    params: vec![],
                    ret: TypeName::new("Void"),
                    body: Some(vec![]),
                    span: Span::default(),
                })),
                group_("S", vec![]),
                choice_("C", vec![]),
                alias("A", TypeName::new("S")),
            ])
        );
    }

    fn alias(name: &str, ty: TypeName) -> Stmt {
        stmt_node(StmtKind::Alias(Alias {
            name: name.to_string(),
            ty: Some(ty),
            span: Span::default(),
        }))
    }

    #[test]
    fn parses_aliases() {
        assert_eq!(
            parse_body("alias ID = Int"),
            Ok(vec![alias("ID", TypeName::new("Int"))])
        );
        assert_eq!(
            parse_body("alias Shape = shape::Shape"),
            Ok(vec![alias(
                "Shape",
                TypeName::Named(qualified("shape", "Shape")),
            )])
        );
    }

    #[test]
    fn rejects_malformed_aliases() {
        for src in [
            "alias",
            "alias ID",
            "alias ID =",
            "alias = Int",
            "alias ID: Int",
            "alias ID = 1",
            "alias ID = Int Bool",
            "alias ID = Int.zero",
            "alias other::ID = Int",
            "alias alias = Int",
            "alias ID = alias",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_primitives_only_at_the_top_level() {
        assert_eq!(
            parse("primitive Int\nprimitive Bool"),
            Ok(vec![
                stmt_node(StmtKind::Primitive {
                    name: "Int".to_string(),
                    params: vec![],
                }),
                stmt_node(StmtKind::Primitive {
                    name: "Bool".to_string(),
                    params: vec![],
                }),
            ])
        );
        assert_eq!(
            parse("primitive Fn<Input, Output>"),
            Ok(vec![stmt_node(StmtKind::Primitive {
                name: "Fn".to_string(),
                params: vec!["Input".to_string(), "Output".to_string()],
            })])
        );
        for src in [
            "primitive",
            "primitive Int {}",
            "primitive Int: Bool",
            "primitive other::Int",
            "primitive Int Bool",
            "primitive primitive",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
        assert!(parse_body("primitive Int").is_err());
    }

    #[test]
    fn parses_primitive_functions_only_at_the_top_level() {
        assert_eq!(
            parse("primitive fn exit() -> Never\nprimitive fn wait(for seconds: Int)"),
            Ok(vec![
                stmt_node(StmtKind::Fn(Function {
                    is_static: false,
                    name: "exit".to_string(),
                    params: vec![],
                    ret: TypeName::new("Never"),
                    body: None,
                    span: Span::default(),
                })),
                stmt_node(StmtKind::Fn(Function {
                    is_static: false,
                    name: "wait".to_string(),
                    params: vec![param(Some("for"), Some("seconds"), "Int")],
                    ret: TypeName::new("Void"),
                    body: None,
                    span: Span::default(),
                })),
            ])
        );
        for src in [
            "primitive fn",
            "primitive fn exit",
            "primitive fn exit() {}",
            "primitive fn exit() -> Never {}",
            "primitive fn other::exit()",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
        assert!(parse_body("primitive fn exit()").is_err());
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

    fn use_file(path: ImportPath) -> Stmt {
        stmt_node(StmtKind::Use(Import::File { path, alias: None }))
    }

    fn qualified(file: &str, name: &str) -> Name {
        Name {
            file: Some(file.to_string()),
            name: name.to_string(),
            span: Span::default(),
        }
    }

    #[test]
    fn parses_qualified_names() {
        assert_eq!(
            parse_expr("other::x"),
            expr_node(ExprKind::Var(qualified("other", "x")))
        );
        assert_eq!(
            parse_expr("other::f() / other::y"),
            expr_node(ExprKind::Binary(
                Box::new(expr_node(ExprKind::Call {
                    callee: qualified("other", "f"),
                    args: vec![],
                })),
                BinOp::Div,
                Box::new(expr_node(ExprKind::Var(qualified("other", "y")))),
            ))
        );
        assert_eq!(
            parse_body("let x: stdlib::Int = 1"),
            Ok(vec![stmt_node(StmtKind::Decl {
                mutability: Mutability::Let,
                name: "x".to_string(),
                ty: Some(TypeName::Named(qualified("stdlib", "Int"))),
                value: expr_node(ExprKind::Int(1)),
            })])
        );
        // Division still works without spaces.
        assert_eq!(parse_expr("a/b"), *bin(var("a"), BinOp::Div, var("b")));
        // A slash before a qualified name is always division.
        assert_eq!(
            parse_expr("a/b::c"),
            *bin(
                var("a"),
                BinOp::Div,
                Box::new(expr_node(ExprKind::Var(qualified("b", "c"))))
            )
        );
    }

    #[test]
    fn parses_aliased_imports() {
        let alias = |path, alias: &str| {
            stmt_node(StmtKind::Use(Import::File {
                path,
                alias: Some(alias.to_string()),
            }))
        };
        let segments = |segments: &[&str]| segments.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            parse("use ./a as x\nuse /dir/a as y\nuse stdlib/core as z"),
            Ok(vec![
                alias(ImportPath::Relative(segments(&["a"])), "x"),
                alias(ImportPath::Absolute(segments(&["dir", "a"])), "y"),
                alias(ImportPath::Package(segments(&["stdlib", "core"])), "z"),
            ])
        );
    }

    #[test]
    fn as_is_only_a_keyword_in_uses() {
        assert_eq!(parse_expr("as"), *var("as"));
        assert!(parse("fn same(as other: Int) {}\nlet as = same(as: 1)").is_ok());
    }

    #[test]
    fn rejects_paths_outside_uses() {
        for src in [
            "let x = /a::b",
            "let x = ./a::b",
            "let x = /dir/a::b",
            "let x = ./dir/a::f()",
            "let x: /a::B = 1",
            "let x: ./a::B = 1",
            "let x: dir/a::B = 1",
            "alias B = /a::B",
            "alias B = dir/a::B",
            "let x = /a::B { y: 1 }",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_malformed_paths() {
        for src in [
            "use",
            "use .",
            "use .a",
            "use /a::b",
            "use /a\n/b",
            "use /a use /b",
            "use a::b",
            "use /a as",
            "use /a as b as c",
            "use /a as b/c",
            "use /a as /b",
            "use /a as b::c",
            "use /a\nas b",
            "use as b",
            "use /a as use",
            "use /a as alias",
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
            span: Span::default(),
        }
    }

    fn construct(fields: Vec<FieldValue>) -> Expr {
        expr_node(ExprKind::Construct(fields))
    }

    #[test]
    fn parses_constructors() {
        assert_eq!(
            parse_expr("Self { age: 42, isAdmin: true }"),
            construct(vec![
                field_value("age", expr_node(ExprKind::Int(42))),
                field_value("isAdmin", expr_node(ExprKind::Bool(true))),
            ],)
        );
        assert_eq!(parse_expr("Self {}"), construct(vec![]));
        assert_eq!(
            parse_expr("Self {age:1+2,isAdmin:f()}"),
            construct(vec![
                field_value("age", *bin(int(1), BinOp::Add, int(2))),
                field_value("isAdmin", *call("f", vec![])),
            ],)
        );
    }

    #[test]
    fn constructors_nest_and_bind_tightest() {
        assert_eq!(
            parse_expr("-Self { lead: Self { age: 1 } } * 2"),
            *bin(
                neg(Box::new(construct(vec![field_value(
                    "lead",
                    construct(vec![field_value("age", expr_node(ExprKind::Int(1)))]),
                )],))),
                BinOp::Mul,
                int(2),
            )
        );
        assert_eq!(
            parse_expr("f(user: Self {})"),
            *call("f", vec![arg(Some("user"), construct(vec![]))])
        );
    }

    #[test]
    fn conditions_are_any_expression() {
        // Only `Self` can be constructed, so after any other value, the `{`
        // starts the block or the options.
        assert_eq!(
            parse_body("if done {}"),
            Ok(vec![if_(*var("done"), vec![], None)])
        );
        assert_eq!(
            parse_body("match s {}"),
            Ok(vec![stmt_node(StmtKind::Match {
                value: *var("s"),
                arms: vec![],
            })])
        );
        let on = || construct(vec![field_value("on", expr_node(ExprKind::Bool(true)))]);
        assert_eq!(
            parse_body("if Self { on: true } {}"),
            Ok(vec![if_(on(), vec![], None)])
        );
        assert_eq!(
            parse_body("if f(flag: Self { on: true }) {}"),
            Ok(vec![if_(
                *call("f", vec![arg(Some("flag"), on())]),
                vec![],
                None,
            )])
        );
    }

    #[test]
    fn rejects_constructor_statements() {
        // Only calls can be statements.
        assert!(parse_body("Self {}").is_err());
        assert!(parse_body("Self { age: 1 }").is_err());
    }

    #[test]
    fn rejects_malformed_constructors() {
        for src in [
            "x = Self {",
            "x = Self { age }",
            "x = Self { age: }",
            "x = Self { 42 }",
            "x = Self { age = 42 }",
            "x = Self { age: 1,, }",
            "x = Self { age: 1 isAdmin: true }",
            "x = Self { , }",
            "x = Self { _: 1 }",
            "x = Self\n{}",
            "x = 1 {}",
            "x = (Self) {}",
            // Only `Self` can be constructed.
            "x = User { age: 1 }",
            "x = Box<Int> { value: 1 }",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_multiline_constructors() {
        let user = || {
            construct(vec![
                field_value("age", expr_node(ExprKind::Int(42))),
                field_value("isAdmin", expr_node(ExprKind::Bool(true))),
            ])
        };
        assert_eq!(
            parse_body(
                "
                let a = Self {
                    age: 42,
                    isAdmin: true
                }
                let b = Self { age: 42,
                    isAdmin: true }
                let c = Self {

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
        assert_eq!(parse_expr("Self {\n\n}"), construct(vec![]));
    }

    #[test]
    fn parses_nested_multiline_constructors() {
        assert_eq!(
            parse_body(
                "
                let t = Self {
                    lead: Self {
                        age: 30
                    },
                    size: f(x: Self {
                        x: 1
                    })
                }
                "
            ),
            Ok(vec![decl(
                Mutability::Let,
                "t",
                None,
                construct(vec![
                    field_value(
                        "lead",
                        construct(vec![field_value("age", expr_node(ExprKind::Int(30)))]),
                    ),
                    field_value(
                        "size",
                        *call(
                            "f",
                            vec![arg(
                                Some("x"),
                                construct(vec![field_value("x", expr_node(ExprKind::Int(1)))]),
                            )],
                        ),
                    ),
                ],),
            )])
        );
    }

    #[test]
    fn rejects_malformed_multiline_constructors() {
        for src in [
            // Fields still need commas between them.
            "
            x = Self {
                age: 42
                isAdmin: true
            }
            ",
            // The comma must come straight after the value.
            "
            x = Self {
                age: 42
                , isAdmin: true
            }
            ",
            // A field can't be split across lines.
            "
            x = Self {
                age:
                    42
            }
            ",
            // The `{` must be on the same line as `Self`.
            "
            x = Self
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
                    arg(Some("by"), expr_node(ExprKind::Int(2))),
                    arg(Some("twice"), expr_node(ExprKind::Bool(true))),
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
            assert_eq!(
                parse_body(src),
                Ok(vec![stmt_node(StmtKind::Expr(expected()))]),
                "{src:?}"
            );
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
                    b: Self {
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
                            arg(
                                Some("a"),
                                *call("g", vec![arg(None, expr_node(ExprKind::Int(1)))])
                            ),
                            arg(
                                Some("b"),
                                construct(vec![field_value(
                                    "age",
                                    *call("h", vec![arg(Some("c"), expr_node(ExprKind::Int(2)))]),
                                )],),
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

    #[test]
    fn allows_trailing_commas() {
        assert_eq!(parse_expr("f(a: 1,)"), parse_expr("f(a: 1)"));
        assert_eq!(
            parse_expr("Self { age: 1, }"),
            parse_expr("Self { age: 1 }")
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
        expr_node(ExprKind::MethodCall {
            receiver: Box::new(receiver),
            method: method.to_string(),
            args,
        })
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
                stmt_node(StmtKind::Impl {
                    ty: Name::new("User"),
                    with: None,
                    aliases: vec![],
                    functions: vec![
                        Function {
                            is_static: false,
                            name: "score".to_string(),
                            params: vec![],
                            ret: TypeName::new("Int"),
                            body: Some(vec![stmt_node(StmtKind::Return(Some(method_call(
                                expr_node(ExprKind::SelfValue),
                                "base",
                                vec![],
                            ))))]),
                            span: Span::default(),
                        },
                        Function {
                            is_static: false,
                            name: "rename".to_string(),
                            params: vec![param(Some("to"), Some("name"), "Int")],
                            ret: TypeName::new("Void"),
                            body: Some(vec![]),
                            span: Span::default(),
                        },
                        Function {
                            is_static: true,
                            name: "make".to_string(),
                            params: vec![],
                            ret: TypeName::new("User"),
                            body: Some(vec![stmt_node(StmtKind::Return(Some(method_call(
                                *var("User"),
                                "make",
                                vec![],
                            ))))]),
                            span: Span::default(),
                        },
                    ],
                }),
                stmt_node(StmtKind::Impl {
                    ty: qualified("user", "User"),
                    with: None,
                    aliases: vec![],
                    functions: vec![],
                }),
            ])
        );
    }

    #[test]
    fn parses_method_calls() {
        assert_eq!(
            parse_expr("a.f(x: 1).g()"),
            method_call(
                method_call(
                    *var("a"),
                    "f",
                    vec![arg(Some("x"), expr_node(ExprKind::Int(1)))]
                ),
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
                    Box::new(method_call(
                        *paren(bin(int(1), BinOp::Add, int(2))),
                        "h",
                        vec![],
                    )),
                ),
                BinOp::Add,
                Box::new(method_call(expr_node(ExprKind::Int(3)), "i", vec![])),
            )
        );
        assert_eq!(
            parse_expr("Self {}.f()"),
            method_call(construct(vec![]), "f", vec![])
        );
        assert_eq!(
            parse_expr(
                "other::x.f(
                    a: 1,
                )"
            ),
            method_call(
                expr_node(ExprKind::Var(qualified("other", "x"))),
                "f",
                vec![arg(Some("a"), expr_node(ExprKind::Int(1)))],
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
                stmt_node(StmtKind::Expr(method_call(*var("user"), "promote", vec![]))),
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
        assert_eq!(parse_expr("self"), expr_node(ExprKind::SelfValue));
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

    #[test]
    fn parses_traits() {
        assert_eq!(
            parse(
                "
                trait And {
                    fn and(_ rhs: Self) -> Bool

                    static fn zero() -> Self
                    fn reset(to value: Int)
                }
                trait Marker {}
                "
            ),
            Ok(vec![
                stmt_node(StmtKind::Trait {
                    name: "And".to_string(),
                    aliases: vec![],
                    functions: vec![
                        Function {
                            is_static: false,
                            name: "and".to_string(),
                            params: vec![param(None, Some("rhs"), "Self")],
                            ret: TypeName::new("Bool"),
                            body: None,
                            span: Span::default()
                        },
                        Function {
                            is_static: true,
                            name: "zero".to_string(),
                            params: vec![],
                            ret: TypeName::new("Self"),
                            body: None,
                            span: Span::default()
                        },
                        Function {
                            is_static: false,
                            name: "reset".to_string(),
                            params: vec![param(Some("to"), Some("value"), "Int")],
                            ret: TypeName::new("Void"),
                            body: None,
                            span: Span::default()
                        },
                    ],
                }),
                stmt_node(StmtKind::Trait {
                    name: "Marker".to_string(),
                    aliases: vec![],
                    functions: vec![],
                }),
            ])
        );
    }

    #[test]
    fn parses_impls_with_traits() {
        assert_eq!(
            parse("impl Custom with And {}\nimpl Int with logic::And {}"),
            Ok(vec![
                stmt_node(StmtKind::Impl {
                    ty: Name::new("Custom"),
                    with: Some(Name::new("And")),
                    aliases: vec![],
                    functions: vec![],
                }),
                stmt_node(StmtKind::Impl {
                    ty: Name::new("Int"),
                    with: Some(qualified("logic", "And")),
                    aliases: vec![],
                    functions: vec![],
                }),
            ])
        );
        // `with` is only special in an `impl`, so it can still be a label.
        assert!(parse("fn f(with value: Int) {}\nlet x = f(with: 1)").is_ok());
    }

    #[test]
    fn parses_primitive_functions_in_impls() {
        assert_eq!(
            parse("impl Bool with And {\nprimitive fn and(_ rhs: Self) -> Bool\n}"),
            Ok(vec![stmt_node(StmtKind::Impl {
                ty: Name::new("Bool"),
                with: Some(Name::new("And")),
                aliases: vec![],
                functions: vec![Function {
                    is_static: false,
                    name: "and".to_string(),
                    params: vec![TupleElement {
                        label: None,
                        name: Some("rhs".to_string()),
                        ty: TypeName::new("Self"),
                        span: Span::default()
                    }],
                    ret: TypeName::new("Bool"),
                    body: None,
                    span: Span::default(),
                }],
            })])
        );
        for src in [
            "impl Bool {\nprimitive fn and(_ rhs: Self) -> Bool {}\n}",
            "impl Bool {\nprimitive and(_ rhs: Self) -> Bool\n}",
            "impl Bool {\nprimitive Int\n}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn rejects_functions_and_aliases_where_they_cannot_be_written() {
        for (src, message) in [
            (
                "static fn f() {}",
                "only an `impl` or `trait` can have a `static fn`",
            ),
            (
                "fn f() {\nprimitive fn g()\n}",
                "only the top level can have a `primitive fn`",
            ),
            ("fn f()", "expected the function's body"),
            ("impl Bool {\nfn f()\n}", "expected the function's body"),
            (
                "impl Bool {\nprimitive fn f() {}\n}",
                "a `primitive fn` has no body, since the compiler implements it",
            ),
            (
                "trait T {\nprimitive fn f()\n}",
                "a `trait` can't have a `primitive fn`",
            ),
            ("alias A", "expected `=` and the type it stands for"),
            (
                "impl Bool with T {\nalias A\n}",
                "expected `=` and the type it stands for",
            ),
            (
                "trait T {\nalias A = Int\n}",
                "a `trait`'s alias has no type, since each `impl` gives one",
            ),
        ] {
            let errs = parse(src).unwrap_err();
            assert_eq!(errs[0].to_string(), message, "{src:?}");
        }
    }

    #[test]
    fn parses_aliases_in_traits_and_impls() {
        let src = "
            trait And {
                alias Rhs
                fn and(_ rhs: Rhs) -> Output
                alias Output
            }
            impl Bool with And {
                alias Rhs = Self

                primitive fn and(_ rhs: Rhs) -> Output
                alias Output = core::Bool
            }
            ";
        let Ok(program) = parse(src) else {
            panic!("should parse");
        };
        let [
            Stmt {
                kind: StmtKind::Trait { aliases, .. },
                ..
            },
            Stmt {
                kind: StmtKind::Impl { aliases: given, .. },
                ..
            },
        ] = &program[..]
        else {
            panic!("expected a trait and an impl, got {program:?}");
        };
        let alias = |name: &str, ty| Alias {
            name: name.to_string(),
            ty,
            span: Span::default(),
        };
        assert_eq!(aliases, &[alias("Rhs", None), alias("Output", None)]);
        assert_eq!(
            given,
            &[
                alias("Rhs", Some(TypeName::new("Self"))),
                alias("Output", Some(TypeName::Named(qualified("core", "Bool")))),
            ]
        );
        for src in [
            "trait And {\nalias Rhs = Int\n}",
            "trait And {\nalias\n}",
            "trait And {\nalias Rhs Output\n}",
            "impl Bool with And {\nalias Rhs\n}",
            "impl Bool with And {\nalias Rhs =\n}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }

    #[test]
    fn parses_default_trait_functions() {
        let src = "
            trait Equal {
                fn isEqual(to rhs: Self) -> Bool
                fn isNotEqual(to rhs: Self) -> Bool {
                    return !self.isEqual(to: rhs)
                }
            }
            ";
        let Ok(program) = parse(src) else {
            panic!("should parse");
        };
        let [
            Stmt {
                kind: StmtKind::Trait { functions, .. },
                ..
            },
        ] = &program[..]
        else {
            panic!("expected a trait, got {program:?}");
        };
        assert_eq!(functions[0].body, None);
        let call = method_call(
            expr_node(ExprKind::SelfValue),
            "isEqual",
            vec![Arg {
                label: Some("to".to_string()),
                value: *var("rhs"),
                span: Span::default(),
            }],
        );
        assert_eq!(
            functions[1].body,
            Some(vec![stmt_node(StmtKind::Return(Some(*not(Box::new(
                call
            )))))])
        );
        assert!(parse("trait Equal {\nfn isEqual(to rhs: Self) -> Bool {\n}").is_err());
    }

    #[test]
    fn rejects_malformed_traits() {
        for src in [
            "trait",
            "trait {}",
            "trait And",
            "trait And { let x: Int }",
            "trait And { fn and(_ rhs: Self) -> Bool fn or() }",
            "trait other::And {}",
            "impl Custom with {}",
            "impl Custom with And, Self {}",
            "impl Custom with And with Or {}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
        assert!(parse_body("trait And {}").is_err());
        assert!(parse_body("trait Marker {}").is_err());
    }

    #[test]
    fn self_type_is_a_keyword() {
        assert_eq!(
            parse_body("let p: Self = Self { x: 1 }"),
            Ok(vec![stmt_node(StmtKind::Decl {
                mutability: Mutability::Let,
                name: "p".to_string(),
                ty: Some(TypeName::new("Self")),
                value: construct(vec![FieldValue {
                    name: "x".to_string(),
                    value: *int(1),
                    span: Span::default()
                }],),
            })])
        );
        assert_eq!(
            parse_expr("Self.new()"),
            method_call(*var("Self"), "new", vec![])
        );
        for src in [
            "let Self = 1",
            "Self = 1",
            "fn Self() {}",
            "fn f(Self: Int) {}",
            "x = other::Self",
            "let x: other::Self = 1",
            "group Self {}",
            "choice Self {}",
            "alias Self = Int",
        ] {
            assert!(parse_body(src).is_err(), "{src:?} should not parse");
        }
    }

    fn field_access(receiver: Expr, field: &str) -> Expr {
        expr_node(ExprKind::Field {
            receiver: Box::new(receiver),
            field: field.to_string(),
        })
    }

    #[test]
    fn parses_field_access() {
        assert_eq!(parse_expr("user.age"), field_access(*var("user"), "age"));
        assert_eq!(
            parse_expr("team.lead.age"),
            field_access(field_access(*var("team"), "lead"), "age")
        );
        assert_eq!(
            parse_expr("self.age"),
            field_access(expr_node(ExprKind::SelfValue), "age")
        );
        assert_eq!(
            parse_expr("Self { age: 1 }.age"),
            field_access(
                construct(vec![field_value("age", expr_node(ExprKind::Int(1)))]),
                "age"
            )
        );
        assert_eq!(
            parse_expr("other::user.age"),
            field_access(expr_node(ExprKind::Var(qualified("other", "user"))), "age")
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

    #[test]
    fn parses_package_paths() {
        assert_eq!(
            parse("use stdlib/core\nlet x: core::Int = core::Int.zero()"),
            Ok(vec![
                use_file(ImportPath::Package(vec![
                    "stdlib".to_string(),
                    "core".to_string()
                ])),
                stmt_node(StmtKind::Decl {
                    mutability: Mutability::Let,
                    name: "x".to_string(),
                    ty: Some(TypeName::Named(qualified("core", "Int"))),
                    value: method_call(
                        expr_node(ExprKind::Var(qualified("core", "Int"))),
                        "zero",
                        vec![]
                    ),
                }),
            ])
        );
    }

    #[test]
    fn parses_parent_paths() {
        let segments = |segments: &[&str]| segments.iter().map(|s| s.to_string()).collect();
        let file = |path, alias: Option<&str>| {
            stmt_node(StmtKind::Use(Import::File {
                path,
                alias: alias.map(str::to_string),
            }))
        };
        let module = |path| stmt_node(StmtKind::Use(Import::Module(path)));
        assert_eq!(
            parse("use ../a\nuse ../../a/b\nuse ../a as c\nuse ../\nuse ../../\nuse ../a/"),
            Ok(vec![
                file(ImportPath::Parent(1, segments(&["a"])), None),
                file(ImportPath::Parent(2, segments(&["a", "b"])), None),
                file(ImportPath::Parent(1, segments(&["a"])), Some("c")),
                module(ImportPath::Parent(1, vec![])),
                module(ImportPath::Parent(2, vec![])),
                module(ImportPath::Parent(1, segments(&["a"]))),
            ])
        );
        // As a real path, so that spaces inside one would be noticed.
        for src in ["use ../a", "use ../../a/b as c", "use ../", "use ../../a/"] {
            assert!(super::parse(src).is_ok(), "{src:?}");
        }
    }

    #[test]
    fn parent_directories_are_only_allowed_at_the_start_of_a_path() {
        for src in [
            "use ..",
            "use ...",
            "use .../a",
            "use ..a",
            "use ../..",
            "use ../.",
            "use ./../a",
            "use /../a",
            "use a/../b",
            "use ./a/../b",
            "use /a/../b",
            "use ../a/../b",
            "use ../a/..",
            "use ../a/./b",
            "let x = ../a::b",
            "let x: ../a::B = 1",
            "fn f() {\nuse ../a\n}",
        ] {
            assert!(super::parse(src).is_err(), "{src:?} should not parse");
        }
        for src in [
            "use . ./a",
            "use .. /a",
            "use ../ a",
            "use ../ ../a",
            "use ../.. /a",
        ] {
            let errs = super::parse(src).expect_err(src);
            assert!(
                errs.iter().any(|err| err
                    .to_string()
                    .contains("paths can't contain spaces or comments")),
                "{src:?}: {errs:?}"
            );
        }
    }

    #[test]
    fn parses_module_imports() {
        let directory = |path| stmt_node(StmtKind::Use(Import::Module(path)));
        let segments = |segments: &[&str]| segments.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            parse("use ./users/\nuse /users/admins/\nuse ./\nuse /\nuse stdlib/"),
            Ok(vec![
                directory(ImportPath::Relative(segments(&["users"]))),
                directory(ImportPath::Absolute(segments(&["users", "admins"]))),
                directory(ImportPath::Relative(vec![])),
                directory(ImportPath::Absolute(vec![])),
                directory(ImportPath::Package(segments(&["stdlib"]))),
            ])
        );
    }

    #[test]
    fn rejects_spaces_and_comments_in_paths() {
        for src in [
            "use . /a",
            "use ./ a",
            "use / a",
            "use /a /b",
            "use /a/ b",
            "use a /b",
            "use a/ b",
            "use ./a/ b",
            "use ./a /b",
            "use ./a\t/b",
            "use ./users /",
            "use a /",
            "use . /",
            "use ./users/ as",
            "use /a/ b as c",
            "use /a/*c*//b",
            "use /a/*c*//",
        ] {
            let errs = super::parse(src).expect_err(src);
            assert!(
                errs.iter().any(|err| err
                    .to_string()
                    .contains("paths can't contain spaces or comments")),
                "{src:?}: {errs:?}"
            );
        }
    }

    #[test]
    fn modules_can_not_have_aliases() {
        for (src, module) in [
            ("use ./users/ as x", "./users/"),
            ("use /users/admins/ as x", "/users/admins/"),
            ("use ./ as x", "./"),
            ("use ../ as x", "../"),
            ("use ../../users/ as x", "../../users/"),
            ("use / as x", "/"),
            ("use stdlib/ as x", "stdlib/"),
            ("use ./users/ as x // Note.", "./users/"),
            ("use ./users/ as x\nlet y = 1", "./users/"),
        ] {
            let errs: Vec<_> = super::parse(src)
                .expect_err(src)
                .iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(
                errs,
                [format!("`{module}` is a module, so it can't have an alias")],
                "{src:?}"
            );
        }
        // A file called `as` can still have one.
        assert!(super::parse("use ./users/as as x").is_ok());
        assert!(super::parse("use ./users/as").is_ok());
    }

    #[test]
    fn spaces_are_allowed_around_paths() {
        let relative = |segments: &[&str]| {
            ImportPath::Relative(segments.iter().map(|s| s.to_string()).collect())
        };
        let src = "  use   ./a/b   as   c  \n\tuse\t./d/\t";
        assert_eq!(
            parse(src),
            Ok(vec![
                stmt_node(StmtKind::Use(Import::File {
                    path: relative(&["a", "b"]),
                    alias: Some("c".to_string()),
                })),
                stmt_node(StmtKind::Use(Import::Module(relative(&["d"])))),
            ])
        );
        // The real spans are what show whether a path contains spaces.
        assert!(super::parse(src).is_ok());
    }

    #[test]
    fn division_can_have_spaces() {
        assert_eq!(parse_expr("a / b"), *bin(var("a"), BinOp::Div, var("b")));
    }

    #[test]
    fn comments_can_follow_any_part_of_a_use() {
        let file = |alias: Option<&str>| {
            stmt_node(StmtKind::Use(Import::File {
                path: ImportPath::Relative(vec!["a".to_string()]),
                alias: alias.map(str::to_string),
            }))
        };
        let users = stmt_node(StmtKind::Use(Import::Module(ImportPath::Relative(vec![
            "users".to_string(),
        ]))));
        for (src, expected) in [
            ("use ./a/* note */", file(None)),
            ("use /* note */ ./a", file(None)),
            ("use ./a as b/* note */", file(Some("b"))),
            ("use ./a /* note */ as b", file(Some("b"))),
            ("use ./users/ /* note */", users.clone()),
            ("use ./users/ // note", users.clone()),
        ] {
            assert_eq!(parse(src), Ok(vec![expected]), "{src:?}");
            // The real spans are what show whether a path contains comments.
            assert!(super::parse(src).is_ok(), "{src:?}");
        }
    }

    #[test]
    fn displays_module_imports_as_written() {
        for src in [
            "./users/",
            "/users/admins/",
            "./",
            "/",
            "stdlib/",
            "../",
            "../../",
            "../users/",
        ] {
            let program = parse(&format!("use {src}")).unwrap();
            let StmtKind::Use(import) = &program[0].kind else {
                panic!("expected a `use`");
            };
            assert_eq!(import.to_string(), src);
        }
    }

    #[test]
    fn rejects_malformed_module_imports() {
        for src in [
            "use *",
            "use ./users*",
            "use ./*c*/a",
            // `/*` always starts a comment, which is never closed here.
            "use ./users/*",
            "use ./*",
            "use /*",
            "use ./users/ /admins",
            "use ./users/ as x",
            "use ./ as x",
            "use / as x",
            "use / /",
            "use ./ ./",
            "use users/ /",
            "use ./users/\n/admins",
            "let x = users::*",
            "fn f() {\nuse ./\n}",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
    }
}
