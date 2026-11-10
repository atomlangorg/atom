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

/// Where something is written in its file, as a range of bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// The span from the start of this one to the end of `other`.
    pub fn to(self, other: Self) -> Self {
        Self {
            start: self.start,
            end: other.end,
        }
    }
}

/// The path to a file or a module, as in `use /dir/other` or `use /dir/`. Paths
/// only appear in `use`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPath {
    /// `/dir/other`: from the root of the project, or of its package if the
    /// current file is in one.
    Absolute(Vec<String>),
    /// `./dir/other`: from the directory of the current file, within its
    /// package if it's in one.
    Relative(Vec<String>),
    /// `../dir/other`: from this many directories above the directory of the
    /// current file, which can't go above the root of the project or its
    /// package. `..` can only be at the start of a path, so this is the only way
    /// a path can go up.
    Parent(usize, Vec<String>),
    /// `stdlib/core`: from the root of a package, whose name comes first. Only
    /// a directory can have just the one segment, as in `stdlib/`.
    Package(Vec<String>),
    /// `other`: by name alone, which never leads to a file, but is parsed so that
    /// the checker can explain why.
    Named(String),
}

impl fmt::Display for ImportPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Absolute(segments) => write!(f, "/{}", segments.join("/")),
            Self::Relative(segments) => write!(f, "./{}", segments.join("/")),
            Self::Parent(up, segments) => write!(f, "{}{}", "../".repeat(*up), segments.join("/")),
            Self::Package(segments) => write!(f, "{}", segments.join("/")),
            Self::Named(name) => write!(f, "{name}"),
        }
    }
}

/// What a `use` imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Import {
    /// `use /dir/other` or `use /dir/other as renamed`: one file, whose names
    /// are qualified by its alias if it has one, and otherwise by its own name.
    File {
        path: ImportPath,
        alias: Option<String>,
    },
    /// `use /dir/`: a module, which is every file in a directory, including
    /// those in its subdirectories, given the directory's path.
    /// The path can be empty, as in `use ./`, or just a package's name, as in
    /// `use stdlib/`.
    Module(ImportPath),
}

impl fmt::Display for Import {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::File { path, alias: None } => write!(f, "{path}"),
            Self::File {
                path,
                alias: Some(alias),
            } => write!(f, "{path} as {alias}"),
            // An empty path already ends in a `/`.
            Self::Module(
                path @ (ImportPath::Absolute(segments)
                | ImportPath::Relative(segments)
                | ImportPath::Parent(_, segments)),
            ) if segments.is_empty() => {
                write!(f, "{path}")
            }
            Self::Module(path) => write!(f, "{path}/"),
        }
    }
}

/// A name, optionally qualified by the name of the file it's declared in, as
/// in `f` or `other::f`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub file: Option<String>,
    pub name: String,
    pub span: Span,
}

impl Name {
    /// A name without a file.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            file: None,
            name: name.into(),
            span: Span::default(),
        }
    }

    /// Whether this is the keyword `Self`.
    pub fn is_self(&self) -> bool {
        self.file.is_none() && self.name == "Self"
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.file {
            Some(file) => write!(f, "{file}::{}", self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

/// A type as it's written, such as `Int`, `other::User`, the tuple
/// `(x: Int, y: Int)`, or the generic type `Optional<Int>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    Named(Name),
    Tuple(Vec<TupleElement>),
    /// A generic type, given the types its generic parameters stand for.
    Generic {
        name: Name,
        args: Vec<TypeName>,
    },
    /// `Self.T`: what the generic parameter `T` of the `impl`'s type stands
    /// for.
    SelfParam(String, Span),
}

impl TypeName {
    /// The type called `name`, without a file.
    pub fn new(name: impl Into<String>) -> Self {
        Self::Named(Name::new(name))
    }

    /// Whether this is written `Void`, which is what's meant where no type is
    /// written.
    pub fn is_void(&self) -> bool {
        matches!(self, Self::Named(name) if name.file.is_none() && name.name == "Void")
    }
}

impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Named(name) => write!(f, "{name}"),
            Self::Tuple(elements) => {
                let elements: Vec<_> = elements.iter().map(ToString::to_string).collect();
                write!(f, "({})", elements.join(", "))
            }
            Self::Generic { name, args } => {
                let args: Vec<_> = args.iter().map(ToString::to_string).collect();
                write!(f, "{name}<{}>", args.join(", "))
            }
            Self::SelfParam(param, _) => write!(f, "Self.{param}"),
        }
    }
}

/// An element of a tuple type, a function's parameters, or an option's
/// value, such as `at x: Int`, `x: Int`, `at _: Int`, `_ x: Int` or `_: Int`.
///
/// Its label and type are its public contract: the label is used to build
/// the tuple or call the function, as in `(at: 1)`, and `_` means it's built
/// without one, as in `(1)`. Its name is what the value is read by, as in
/// `point.x`, or what a parameter is called in its function. `x: Int` has
/// `x` as both, and without a name, it can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleElement {
    pub label: Option<String>,
    pub name: Option<String>,
    pub ty: TypeName,
    pub span: Span,
}

impl fmt::Display for TupleElement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let ty = &self.ty;
        match (&self.label, &self.name) {
            (Some(label), Some(name)) if label == name => write!(f, "{name}: {ty}"),
            (Some(label), Some(name)) => write!(f, "{label} {name}: {ty}"),
            (None, Some(name)) => write!(f, "_ {name}: {ty}"),
            (Some(label), None) => write!(f, "{label} _: {ty}"),
            (None, None) => write!(f, "_: {ty}"),
        }
    }
}

/// A binary operator, which calls a function of a trait from the standard
/// library, as `trait_function` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    And,
    Or,
}

impl BinOp {
    pub const ALL: [Self; 17] = [
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Eq,
        Self::Ne,
        Self::Lt,
        Self::Le,
        Self::Gt,
        Self::Ge,
        Self::BitAnd,
        Self::BitOr,
        Self::BitXor,
        Self::ShiftLeft,
        Self::ShiftRight,
        Self::And,
        Self::Or,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::BitAnd => ".&",
            Self::BitOr => ".|",
            Self::BitXor => ".^",
            Self::ShiftLeft => ".<",
            Self::ShiftRight => ".>",
            Self::And => "&",
            Self::Or => "|",
        }
    }

    /// The trait from the standard library that the operator calls a
    /// function of, and that function's name, such as `Add` and `add` for
    /// `+`.
    pub fn trait_function(self) -> (&'static str, &'static str) {
        match self {
            Self::Add => ("Add", "add"),
            Self::Sub => ("Subtract", "subtract"),
            Self::Mul => ("Multiply", "multiply"),
            Self::Div => ("Divide", "divide"),
            Self::Eq => ("Equal", "isEqual"),
            Self::Ne => ("Equal", "isNotEqual"),
            Self::Lt => ("Compare", "isLess"),
            Self::Le => ("Compare", "isLessOrEqual"),
            Self::Gt => ("Compare", "isGreater"),
            Self::Ge => ("Compare", "isGreaterOrEqual"),
            Self::BitAnd => ("BitAnd", "bitAnd"),
            Self::BitOr => ("BitOr", "bitOr"),
            Self::BitXor => ("BitXor", "bitXor"),
            Self::ShiftLeft => ("ShiftLeft", "shiftLeft"),
            Self::ShiftRight => ("ShiftRight", "shiftRight"),
            Self::And => ("And", "and"),
            Self::Or => ("Or", "or"),
        }
    }

    /// Whether it compares its operands, producing a `Bool` rather than
    /// something like them. Comparisons can't be chained.
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }
}

/// An operator before a value, which calls a function of a trait from the
/// standard library, as `trait_function` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

impl UnOp {
    pub const ALL: [Self; 3] = [Self::Neg, Self::Not, Self::BitNot];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
            Self::BitNot => ".!",
        }
    }

    /// The trait from the standard library that the operator calls a
    /// function of, and that function's name, such as `Negate` and `negate`
    /// for `-`.
    pub fn trait_function(self) -> (&'static str, &'static str) {
        match self {
            Self::Neg => ("Negate", "negate"),
            Self::Not => ("Not", "not"),
            Self::BitNot => ("BitNot", "bitNot"),
        }
    }
}

/// An expression, along with where it's written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    /// An integer literal, whose type comes from where it's used.
    Int(u64),
    Bool(bool),
    Var(Name),
    Unary(UnOp, Box<Expr>),
    Binary(Box<Expr>, BinOp, Box<Expr>),
    /// `callee(label: value, ...)`
    Call {
        callee: Name,
        args: Vec<Arg>,
    },
    /// `Self { field: value, ... }`: constructs the group that `Self` is,
    /// giving every field a value in the order they're declared.
    Construct(Vec<FieldValue>),
    /// `Type<Arg, ...>` before a `.`, as in `Optional<Int>.none`: a generic
    /// type given its arguments, whose options and static functions can be
    /// used.
    GenericType {
        name: Name,
        args: Vec<TypeName>,
    },
    /// `receiver.method(label: value, ...)`: calls a function from an `impl`
    /// of the receiver's type.
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Arg>,
    },
    /// `receiver.field`: reads a field of a group.
    Field {
        receiver: Box<Expr>,
        field: String,
    },
    /// `self`: the value a method was called on.
    SelfValue,
    /// `(value)`: a value in brackets, which builds a tuple with one element
    /// without a label when one is expected, and is the value itself
    /// otherwise.
    Paren(Box<Expr>),
    /// `(label: value, ...)` or `()`: builds a tuple, labelling each element
    /// that has a label in its type, like the arguments of a call.
    Tuple(Vec<Arg>),
}

/// A field's value when constructing a group, such as `age: 42`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldValue {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

/// An argument in a function call, such as `to: 3` or `3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub label: Option<String>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    /// `let`: a constant that cannot be assigned to again.
    Let,
    /// `var`: a variable that can be reassigned.
    Var,
}

/// A statement, along with where it's written. For one holding a block or a
/// list of declarations, its span stops before them, at the part that says
/// what it is, such as `fn f()`, `if x` or `group Point`, so that's what errors
/// point at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmtKind {
    /// `use /dir/other` or `use /dir/`: makes the names declared at the top
    /// level of other files visible. Only allowed at the top level.
    Use(Import),
    /// `let name = value` or `var name = value`
    ///
    /// Either can have an explicit type, as in `let name: Int = value`.
    Decl {
        mutability: Mutability,
        name: String,
        /// The explicit type, if there is one.
        ty: Option<TypeName>,
        value: Expr,
    },
    /// `name = value`: reassigns an existing `var`.
    Assign { name: String, value: Expr },
    /// `if cond { ... } else { ... }`
    ///
    /// `else if` is represented as an `else` block containing a single `If`.
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// A function, which can be a `primitive fn` at the top level of the
    /// standard library, such as `exit`.
    Fn(Function),
    /// `impl Type { fn ... }`: adds methods and static functions to a type.
    /// Only allowed at the top level.
    ///
    /// `impl Type with Trait { alias ... fn ... }` implements a trait's
    /// aliases and functions, and only those.
    Impl {
        ty: Name,
        with: Option<Name>,
        /// `alias Name = Type`, giving the type of one of the trait's aliases.
        aliases: Vec<Alias>,
        functions: Vec<Function>,
    },
    /// `trait Name { alias ... fn ... }`: functions that a type can
    /// implement, declared without bodies, and aliases, such as `alias Rhs`,
    /// whose types each implementation gives. Only allowed at the top level.
    Trait {
        name: String,
        aliases: Vec<Alias>,
        functions: Vec<Function>,
    },
    /// `group Name { let field: Type ... }`, or `group Name<T, ...> { ... }`
    /// for a generic group, whose fields can use its generic parameters as
    /// types.
    Group {
        name: String,
        /// The names of its generic parameters, if it's generic.
        params: Vec<String>,
        fields: Vec<Field>,
    },
    /// `alias Name = Type`: another name for a type, which can be used in
    /// place of it anywhere.
    Alias(Alias),
    /// `primitive Name`: a type that the compiler represents itself, such as
    /// `Int`, which can be generic, as in `primitive Fn<Input, Output>`. Only
    /// allowed at the top level of the standard library.
    Primitive {
        name: String,
        /// The names of its generic parameters, if it's generic.
        params: Vec<String>,
    },
    /// `choice Name { option: Type ... }`: a value that's exactly one of its
    /// options, each holding a value of its own type. Like a group, it can be
    /// generic, as in `choice Optional<T> { ... }`.
    Choice {
        name: String,
        /// The names of its generic parameters, if it's generic.
        params: Vec<String>,
        options: Vec<ChoiceOption>,
    },
    /// `match value { pattern => { ... } ... }`: runs the block of the first
    /// pattern that `value` matches. Together, the patterns must match every
    /// possible value, and each must match something the ones before it
    /// don't.
    Match { value: Expr, arms: Vec<MatchArm> },
    /// `(let a, let b) = value`: matches `value` against a tuple pattern,
    /// declaring the values it binds as `let` constants. The pattern must
    /// match every value it could be given.
    Destructure { pattern: Pattern, value: Expr },
    /// `return` or `return value`
    Return(Option<Expr>),
    /// An expression on its own line. Only function calls are allowed.
    Expr(Expr),
}

/// `fn name(label param: Type, ...) -> Type { ... }`. In an `impl` or a
/// `trait`, it can be a `static fn`, and in the standard library, it can be
/// a `primitive fn`, which has no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// Whether it's a `static fn`, which is called on the type itself, as in
    /// `Int.zero()`, rather than a method, which is called on a value.
    pub is_static: bool,
    pub name: String,
    pub params: Vec<TupleElement>,
    /// The return type. `fn f()` is the same as `fn f() -> Void`,
    /// so this is `Void` when no type is written.
    pub ret: TypeName,
    /// `None` for a `primitive fn`, which the compiler implements itself.
    /// In a `trait`, it's the default implementation, which an `impl` uses
    /// unless it implements the function itself, or `None` if every `impl`
    /// must implement it.
    pub body: Option<Block>,
    /// Its signature, without its body.
    pub span: Span,
}

/// `alias Name = Type`: another name for a type, or in a `trait`, `alias
/// Name`, whose type each `impl` gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    /// `None` only in a `trait`.
    pub ty: Option<TypeName>,
    pub span: Span,
}

/// A group field, such as `let age: Int`. Fields can't have values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub mutability: Mutability,
    pub name: String,
    /// The field's type.
    pub ty: TypeName,
    pub span: Span,
}

/// An option of a choice, such as `circle(radius: Int)` or `empty()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    pub name: String,
    /// The elements of the tuple it holds, as in `(radius: Int)`, which
    /// each have a label.
    pub values: Vec<TupleElement>,
    pub span: Span,
}

/// A pattern in a `match`, and the block run when it's the first to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
}

/// What a value is matched against in a `match`, along with where it's written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternKind {
    /// `_`: matches anything.
    Any,
    /// `let name`: matches anything, giving it a name in the block.
    Binding(String),
    /// An integer, such as `0` or `-1`, which matches only itself.
    Int { value: u64, negative: bool },
    /// `true` or `false`.
    Bool(bool),
    /// `(label: pattern, ...)`: matches a tuple whose elements each match,
    /// with the labels its type builds it with.
    Tuple(Vec<TuplePattern>),
    /// `Type.option(label: pattern, ...)`: matches the option of a choice
    /// whose value matches the tuple pattern in brackets, as in
    /// `Shape.circle(radius: _)`. The type is a name, which can be given
    /// generic arguments.
    Option {
        ty: TypeName,
        option: String,
        values: Vec<TuplePattern>,
    },
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.kind {
            PatternKind::Any => write!(f, "_"),
            PatternKind::Binding(name) => write!(f, "let {name}"),
            PatternKind::Int { value, negative } => {
                write!(f, "{}{value}", if *negative { "-" } else { "" })
            }
            PatternKind::Bool(value) => write!(f, "{value}"),
            PatternKind::Tuple(elements) => write!(f, "{}", show_tuple(elements)),
            PatternKind::Option { ty, option, values } => {
                write!(f, "{ty}.{option}{}", show_tuple(values))
            }
        }
    }
}

/// How the tuple pattern with `elements` is written, as in `(x: 0, _)`.
pub fn show_tuple(elements: &[TuplePattern]) -> String {
    let elements: Vec<_> = elements
        .iter()
        .map(|element| match &element.label {
            Some(label) => format!("{label}: {}", element.pattern),
            None => element.pattern.to_string(),
        })
        .collect();
    format!("({})", elements.join(", "))
}

/// An element of a tuple pattern, such as `x: let x` or `_`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuplePattern {
    pub label: Option<String>,
    pub pattern: Pattern,
}

/// A sequence of statements with its own scope, written `{ ... }`.
pub type Block = Vec<Stmt>;

/// The statements in a file. Only declarations are allowed at the top level.
pub type Program = Vec<Stmt>;
