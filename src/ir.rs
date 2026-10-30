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

//! The checked program, which the front end produces and the back end
//! compiles. Every name has been resolved to what it refers to, so nothing
//! needs looking up again.
//!
//! Operators only work on builtin values, so `Int` and `Bool` are taken apart
//! and put back together around them, as their `builtin` field.

use crate::syntax::ast::{BinOp, UnOp};

#[derive(Debug)]
pub struct Program {
    /// Every type, indexed by its `id`.
    pub types: Vec<TypeDef>,
    /// Every function, including methods, static functions and functions
    /// nested in other functions.
    pub functions: Vec<Function>,
    /// Every top-level value.
    pub globals: Vec<Global>,
    /// Indexes into `globals`, ordered so that each value comes after the
    /// values its initialiser uses.
    pub global_order: Vec<usize>,
    /// Calls `fn main() -> Int` from `/main`, giving its result as a builtin
    /// integer, or `None` if the project doesn't declare one.
    pub main: Option<Expr>,
}

#[derive(Debug)]
pub enum TypeDef {
    /// A type from `stdlib/builtin`, which the compiler represents itself.
    Builtin(Builtin),
    /// A group made of its fields' types, as indexes into `Program::types`,
    /// in the order they're declared.
    Group { name: String, fields: Vec<usize> },
    /// A choice, with the types of the values its options hold, as indexes
    /// into `Program::types`, in the order they're declared.
    Choice { name: String, options: Vec<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// A 64-bit signed integer.
    Int,
    Bool,
}

#[derive(Debug)]
pub struct Function {
    /// Its full name, such as `/main::f`, `/main::f::nested`, or
    /// `/main::Int.doubled` for a method from an `impl Int` in `/main`.
    pub name: String,
    /// The types of its local values, as indexes into `Program::types`. A
    /// method's `self` comes first, then the parameters in order.
    pub locals: Vec<usize>,
    /// How many of the first `locals` are given by the caller.
    pub params: usize,
    /// The return type, as an index into `Program::types`.
    pub returns: usize,
    /// Every path through the body ends with a `return`.
    pub body: Vec<Stmt>,
}

#[derive(Debug)]
pub struct Global {
    /// Its full name, such as `/main::x`.
    pub name: String,
    /// Its type, as an index into `Program::types`.
    pub ty: usize,
    pub value: Expr,
}

#[derive(Debug)]
pub enum Stmt {
    /// Stores a value in a local, which is how both declarations and
    /// assignments compile.
    Assign {
        local: usize,
        value: Expr,
    },
    /// `cond` is a builtin boolean.
    If {
        cond: Expr,
        then_block: Vec<Stmt>,
        else_block: Vec<Stmt>,
    },
    /// Runs the block for the option of `value`, a choice, as an index into
    /// `Program::types`. Every option of the choice is matched exactly once.
    Match {
        value: Expr,
        choice: usize,
        options: Vec<MatchOption>,
    },
    Return(Expr),
    Expr(Expr),
}

#[derive(Debug)]
pub struct MatchOption {
    /// The option's index, in the order they're declared.
    pub index: usize,
    /// The local the option's value is stored in, if it's given a name.
    pub binding: Option<usize>,
    pub body: Vec<Stmt>,
}

#[derive(Debug)]
pub enum Expr {
    /// A builtin integer.
    Int(i64),
    /// A builtin boolean.
    Bool(bool),
    /// An index into the current function's `locals`.
    Local(usize),
    /// An index into `Program::globals`.
    Global(usize),
    /// An operator on builtin values, producing another.
    Unary(UnOp, Box<Expr>),
    Binary(Box<Expr>, BinOp, Box<Expr>),
    /// Calls the function at an index into `Program::functions`. A method's
    /// receiver is its first argument.
    Call {
        function: usize,
        args: Vec<Expr>,
    },
    /// Creates a group, as an index into `Program::types`, from its fields'
    /// values in the order they're declared.
    Construct {
        group: usize,
        fields: Vec<Expr>,
    },
    /// Creates a choice, as an index into `Program::types`, as the option at
    /// `option` in the order they're declared, holding `value`.
    Choose {
        choice: usize,
        option: usize,
        value: Box<Expr>,
    },
    /// Reads the field at `index` in the order its group declares them.
    Field {
        receiver: Box<Expr>,
        index: usize,
    },
}
