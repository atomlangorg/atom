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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
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
    Var(String),
    Unary(UnOp, Box<Expr>),
    Binary(Box<Expr>, BinOp, Box<Expr>),
    /// `callee(label: value, ...)`
    Call {
        callee: String,
        args: Vec<Arg>,
    },
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
    /// `let name = value` or `var name = value`
    ///
    /// Either can have an explicit type, as in `let name: Int = value`.
    Decl {
        mutability: Mutability,
        name: String,
        /// The name of the explicit type, if there is one.
        ty: Option<String>,
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
    /// `fn name(label param: Type, ...) -> Type { ... }`
    Fn {
        name: String,
        params: Vec<Param>,
        /// The name of the return type. `fn f()` is the same as
        /// `fn f() -> Void`, so this is `Void` when no type is written.
        ret: String,
        body: Block,
    },
    /// `return` or `return value`
    Return(Option<Expr>),
    /// An expression on its own line. Only function calls are allowed.
    Expr(Expr),
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
    pub ty: String,
}

/// A sequence of statements with its own scope, written `{ ... }`.
pub type Block = Vec<Stmt>;

pub type Program = Vec<Stmt>;
