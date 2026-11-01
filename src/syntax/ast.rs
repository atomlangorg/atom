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

/// The path to a module, as in `use /dir/other`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModulePath {
    /// `/dir/other`: from the root of the project.
    Absolute(Vec<String>),
    /// `./dir/other`: from the directory of the current file, within its
    /// package if it's in one.
    Relative(Vec<String>),
    /// `stdlib/core`: from the root of a package, whose name comes first.
    /// Always has at least two segments.
    Package(Vec<String>),
    /// `other`: by name alone. Only used as a qualifier, where it matches any
    /// imported module with that name.
    Named(String),
}

impl fmt::Display for ModulePath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Absolute(segments) => write!(f, "/{}", segments.join("/")),
            Self::Relative(segments) => write!(f, "./{}", segments.join("/")),
            Self::Package(segments) => write!(f, "{}", segments.join("/")),
            Self::Named(name) => write!(f, "{name}"),
        }
    }
}

/// What a `use` imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Import {
    /// `use /dir/other`: one module.
    Module(ModulePath),
    /// `use /dir/*`: every module in a directory, including those in its
    /// subdirectories, given the directory's path.
    /// The path can be empty, as in `use ./*`, or just a package's name, as in
    /// `use stdlib/*`.
    Directory(ModulePath),
}

impl fmt::Display for Import {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Module(path) => write!(f, "{path}"),
            // Written like a module path whose last segment is `*`.
            Self::Directory(path) => {
                let with_star = |segments: &Vec<String>| {
                    let mut segments = segments.clone();
                    segments.push("*".to_string());
                    segments
                };
                let path = match path {
                    ModulePath::Absolute(segments) => ModulePath::Absolute(with_star(segments)),
                    ModulePath::Relative(segments) => ModulePath::Relative(with_star(segments)),
                    ModulePath::Package(segments) => ModulePath::Package(with_star(segments)),
                    ModulePath::Named(name) => ModulePath::Package(vec![name.clone(), "*".into()]),
                };
                write!(f, "{path}")
            }
        }
    }
}

/// A name, optionally qualified by the module it's declared in, as in `f`,
/// `other::f` or `/dir/other::f`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub module: Option<ModulePath>,
    pub name: String,
}

impl Name {
    /// A name without a module.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            module: None,
            name: name.into(),
        }
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.module {
            Some(module) => write!(f, "{module}::{}", self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

/// A type as it's written, such as `Int`, `other::User`, or the tuple
/// `(x: Int, y: Int)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    Named(Name),
    Tuple(Vec<TupleElement>),
}

impl TypeName {
    /// The type called `name`, without a module.
    pub fn new(name: impl Into<String>) -> Self {
        Self::Named(Name::new(name))
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
        }
    }
}

/// An element of a tuple type, such as `at x: Int`, `x: Int` or `_: Int`.
///
/// Like a function parameter, its label is used to build the tuple, as in
/// `(at: 1)`, and its name is used to read it, as in `point.x`. `x: Int` has
/// `x` as both. Without a label, it's built without one, as in `(1)`, and
/// without a name, it can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleElement {
    pub label: Option<String>,
    pub name: Option<String>,
    pub ty: TypeName,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    /// `==`, which compares two `Int`s or two `Bool`s.
    Eq,
    /// `!=`, the opposite of `==`.
    Ne,
    /// `<`, which orders two integers.
    Lt,
    /// `<=`, which orders two integers.
    Le,
    /// `>`, which orders two integers.
    Gt,
    /// `>=`, which orders two integers.
    Ge,
    /// `.&`, which ands each bit of two integers.
    BitAnd,
    /// `.|`, which ors each bit of two integers.
    BitOr,
    /// `.^`, which xors each bit of two integers.
    BitXor,
    /// `.<`, which shifts the bits of an unsigned integer left, filling with
    /// zeros.
    ShiftLeft,
    /// `.>`, which shifts the bits of an unsigned integer right, filling with
    /// zeros.
    ShiftRight,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(&self) -> &'static str {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// `.!`, which flips each bit of an integer.
    BitNot,
}

impl UnOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
            Self::BitNot => ".!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
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
    /// `Type { field: value, ... }`: constructs a group, giving every field
    /// a value in the order they're declared.
    Construct {
        ty: Name,
        fields: Vec<FieldValue>,
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
}

/// An argument in a function call, such as `to: 3` or `3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub label: Option<String>,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    /// `let`: a constant that cannot be assigned to again.
    Let,
    /// `var`: a variable that can be reassigned.
    Var,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// `use /dir/other` or `use /dir/*`: makes the names declared at the top
    /// level of other modules visible. Only allowed at the top level.
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
    Assign {
        name: String,
        value: Expr,
    },
    /// `if cond { ... } else { ... }`
    ///
    /// `else if` is represented as an `else` block containing a single `If`.
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
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
        aliases: Vec<ImplAlias>,
        functions: Vec<ImplFunction>,
    },
    /// `trait Name { alias ... fn ... }`: functions that a type can
    /// implement, declared without bodies, and aliases, such as `alias Rhs`,
    /// whose types each implementation gives. Only allowed at the top level.
    Trait {
        name: String,
        aliases: Vec<String>,
        functions: Vec<TraitFunction>,
    },
    /// `group Name { let field: Type ... }`
    Group {
        name: String,
        fields: Vec<Field>,
    },
    /// `alias Name = Type`: another name for a type, which can be used in
    /// place of it anywhere.
    Alias {
        name: String,
        ty: TypeName,
    },
    /// `primitive Name`: a type that the compiler represents itself, such as
    /// `Int`. Only allowed at the top level of the standard library.
    Primitive {
        name: String,
    },
    /// `primitive fn name(params) -> Type`: a function that the compiler
    /// implements itself, such as `exit`. Only allowed at the top level of
    /// the standard library.
    PrimitiveFn {
        name: String,
        params: Vec<Param>,
        ret: TypeName,
    },
    /// `choice Name { option: Type ... }`: a value that's exactly one of its
    /// options, each holding a value of its own type.
    Choice {
        name: String,
        options: Vec<ChoiceOption>,
    },
    /// `match value { pattern => { ... } ... }`: runs the block of the first
    /// pattern that `value` matches. Together, the patterns must match every
    /// possible value, and each must match something the ones before it
    /// don't.
    Match {
        value: Expr,
        arms: Vec<MatchArm>,
    },
    /// `return` or `return value`
    Return(Option<Expr>),
    /// An expression on its own line. Only function calls are allowed.
    Expr(Expr),
}

/// `fn name(label param: Type, ...) -> Type { ... }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    /// The return type. `fn f()` is the same as `fn f() -> Void`,
    /// so this is `Void` when no type is written.
    pub ret: TypeName,
    pub body: Block,
}

/// A function declared by a `trait`, such as `fn and(_ rhs: Self) -> Bool`,
/// where `Self` is whichever type implements it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitFunction {
    /// Whether it's a `static fn`, like in an `impl`.
    pub is_static: bool,
    pub name: String,
    pub params: Vec<Param>,
    /// `Void` when no type is written, like in a `fn`.
    pub ret: TypeName,
    /// Its default implementation, which an `impl` uses unless it implements
    /// the function itself, or `None` if every `impl` must implement it.
    pub body: Option<Block>,
}

/// `alias Name = Type` in an `impl`, giving the type of its trait's alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplAlias {
    pub name: String,
    pub ty: TypeName,
}

/// A function in an `impl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplFunction {
    /// Whether it's a `static fn`, which is called on the type itself, as in
    /// `Int.zero()`, rather than a method, which is called on a value.
    pub is_static: bool,
    /// Whether it's a `primitive fn`, which the compiler implements itself,
    /// so its `body` is empty. Only allowed in the standard library.
    pub is_primitive: bool,
    pub function: Function,
}

/// A function parameter, such as `to value: Int`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// The label callers must use, or `None` if declared with `_`.
    ///
    /// When only one name is given, as in `value: Int`, it is both the label
    /// and the parameter name.
    pub label: Option<String>,
    pub name: String,
    /// The parameter's type.
    pub ty: TypeName,
}

/// A group field, such as `let age: Int`. Fields can't have values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub mutability: Mutability,
    pub name: String,
    /// The field's type.
    pub ty: TypeName,
}

/// An option of a choice, such as `circle: Int` or `empty`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    pub name: String,
    /// The type of the value it holds. `empty` is the same as
    /// `empty: Void`, so this is `Void` when no type is written.
    pub ty: TypeName,
}

/// A pattern in a `match`, and the block run when it's the first to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
}

/// What a value is matched against in a `match`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
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
    /// `Type.option` or `Type.option(pattern)`: matches the option of a
    /// choice, and if given, its value against `pattern`.
    Option {
        ty: Name,
        option: String,
        value: Option<Box<Pattern>>,
    },
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Any => write!(f, "_"),
            Self::Binding(name) => write!(f, "let {name}"),
            Self::Int { value, negative } => {
                write!(f, "{}{value}", if *negative { "-" } else { "" })
            }
            Self::Bool(value) => write!(f, "{value}"),
            Self::Tuple(elements) => {
                let elements: Vec<_> = elements
                    .iter()
                    .map(|element| match &element.label {
                        Some(label) => format!("{label}: {}", element.pattern),
                        None => element.pattern.to_string(),
                    })
                    .collect();
                write!(f, "({})", elements.join(", "))
            }
            Self::Option { ty, option, value } => match value {
                Some(value) => write!(f, "{ty}.{option}({value})"),
                None => write!(f, "{ty}.{option}"),
            },
        }
    }
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
