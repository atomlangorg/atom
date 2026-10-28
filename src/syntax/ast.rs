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
            Self::And => "&",
            Self::Or => "|",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Int(i64),
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
        /// The name of the explicit type, if there is one.
        ty: Option<Name>,
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
    Impl {
        ty: Name,
        functions: Vec<ImplFunction>,
    },
    /// `group Name { let field: Type ... }`
    Group {
        name: String,
        fields: Vec<Field>,
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
    /// The name of the return type. `fn f()` is the same as `fn f() -> Void`,
    /// so this is `Void` when no type is written.
    pub ret: Name,
    pub body: Block,
}

/// A function in an `impl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplFunction {
    /// Whether it's a `static fn`, which is called on the type itself, as in
    /// `Int.zero()`, rather than a method, which is called on a value.
    pub is_static: bool,
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
    /// The name of the parameter's type.
    pub ty: Name,
}

/// A group field, such as `let age: Int`. Fields can't have values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub mutability: Mutability,
    pub name: String,
    /// The name of the field's type.
    pub ty: Name,
}

/// A sequence of statements with its own scope, written `{ ... }`.
pub type Block = Vec<Stmt>;

/// The statements in a file. Only declarations are allowed at the top level.
pub type Program = Vec<Stmt>;
