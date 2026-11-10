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
    /// Calls `fn main() -> Int` from `/main`, or `None` if the project doesn't
    /// declare one.
    pub main: Option<Expr>,
}

#[derive(Debug)]
pub enum TypeDef {
    /// A primitive type, such as `Int`, which the compiler represents itself.
    Primitive(Primitive),
    /// A group made of its fields' types, as indexes into `Program::types`,
    /// in the order they're declared.
    Group { name: String, fields: Vec<usize> },
    /// A choice, with the types of the values its options hold, as indexes
    /// into `Program::types`, in the order they're declared.
    Choice { name: String, options: Vec<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    /// An integer type, such as `IntU8`, which is `bits` wide.
    Int {
        bits: u32,
        signed: bool,
    },
    Bool,
    /// `Fn`, which is a pointer to a function whose parameters are the
    /// elements of its `Input`.
    Fn,
    /// `ChoiceOption` and `ChoiceTag`, which are the index of an option of a
    /// choice, in the order they're declared.
    Tag,
}

#[derive(Debug, Clone)]
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
    pub body: Body,
}

#[derive(Debug, Clone)]
pub enum Body {
    /// Every path through the statements ends with a `return`, or a call to
    /// a function that never returns.
    Stmts(Vec<Stmt>),
    /// A function that the compiler implements itself, such as `exit`.
    Primitive(PrimitiveFn),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveFn {
    /// Stops the program with exit code 1.
    Exit,
    /// The function that a binary operator calls on two integers, `Bool`s or
    /// `ChoiceTag`s of the same type, such as `add` for `+`. Shifts are only
    /// on unsigned integers, by an integer of any type.
    Binary {
        op: BinOp,
        /// Whether the operands are signed integers, which decides when
        /// arithmetic overflows, how it divides, and how integers are
        /// ordered.
        signed: bool,
    },
    /// The function that an operator before a value calls on an integer or a
    /// `Bool`, such as `negate` for `-`, which is only on signed integers.
    Unary(UnOp),
    /// `Fn.call`, which calls the function that the `Fn` points to with the
    /// input.
    CallFn,
    /// `ChoiceTag.new`, which is the option that a choice is.
    ChoiceTag,
    /// `ChoiceOption.tag`, which is the option it already is.
    OptionTag,
    /// `ChoiceOption.call`, which creates the choice as that option, holding
    /// the input.
    CallOption,
}

#[derive(Debug)]
pub struct Global {
    /// Its full name, such as `/main::x`.
    pub name: String,
    /// Its type, as an index into `Program::types`.
    pub ty: usize,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// Stores a value in a local, which is how both declarations and
    /// assignments compile.
    Assign {
        local: usize,
        value: Expr,
    },
    /// `cond` is a `Bool`.
    If {
        cond: Expr,
        then_block: Vec<Stmt>,
        else_block: Vec<Stmt>,
    },
    /// Runs the block of the first arm whose pattern matches `value`, whose
    /// type is at `ty` in `Program::types`. Some arm always matches.
    Match {
        value: Expr,
        ty: usize,
        arms: Vec<MatchArm>,
    },
    Return(Expr),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Vec<Stmt>,
}

/// What a value is matched against, which gives the locals it binds their
/// values when it matches.
#[derive(Debug, Clone)]
pub enum Pattern {
    Any,
    /// Matches anything, storing it in the local at this index.
    Binding(usize),
    /// An integer's bits, like `Expr::Int`.
    Int(u64),
    Bool(bool),
    /// A tuple, matching each element in order.
    Tuple(Vec<Pattern>),
    /// The option of a choice at `index`, in the order they're declared, and
    /// its value.
    Option {
        index: usize,
        value: Box<Pattern>,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    /// An integer of the integer type at an index into `Program::types`.
    /// `value` is its bits, so a negative value is in two's complement, and
    /// it's never wider than the type.
    Int {
        value: u64,
        ty: usize,
    },
    Bool(bool),
    /// An index into the current function's `locals`.
    Local(usize),
    /// An index into `Program::globals`.
    Global(usize),
    /// Calls the function at an index into `Program::functions`. A method's
    /// receiver is its first argument.
    Call {
        function: usize,
        args: Vec<Expr>,
    },
    /// A pointer to the function at an index into `Program::functions`, which
    /// is a value of the primitive type `Fn`.
    Function(usize),
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
    /// The option of a choice at this index, in the order they're declared,
    /// which is a value of the primitive type `ChoiceOption`.
    Tag(usize),
    /// Reads the field at `index` in the order its group declares them.
    Field {
        receiver: Box<Expr>,
        index: usize,
    },
    /// Where the checker found a mistake. A program with mistakes is never
    /// compiled, so the back end never sees one.
    Error,
}
