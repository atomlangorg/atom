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

//! Checks that names are declared before use, that only `var`s are
//! reassigned, that functions are called and return correctly, and that every
//! expression is well typed.

use std::{collections::HashMap, fmt};

use super::ast::{Arg, BinOp, Expr, Mutability, Program, Stmt, UnOp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
    /// The return type of a function that doesn't return anything useful.
    /// Functions without an explicit return type return `Void`.
    Void,
}

impl Type {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "Int" => Some(Self::Int),
            "Bool" => Some(Self::Bool),
            "Void" => Some(Self::Void),
            _ => None,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Int => write!(f, "Int"),
            Self::Bool => write!(f, "Bool"),
            Self::Void => write!(f, "Void"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A name was used or assigned without being declared.
    Undeclared(String),
    /// A name was declared twice.
    Redeclared(String),
    /// A `let` constant was assigned to.
    AssignToConstant(String),
    /// An operator was applied to an operand of the wrong type.
    InvalidOperand {
        op: &'static str,
        expected: Type,
        found: Type,
    },
    /// A name was given a value of a different type to its declared type,
    /// either in an explicitly typed declaration or a later assignment.
    AssignTypeMismatch {
        name: String,
        expected: Type,
        found: Type,
    },
    /// An `if` condition was not a `Bool`.
    ConditionNotBool(Type),
    /// An explicit type named a type that doesn't exist.
    UnknownType(String),
    /// Something that isn't a function was called.
    NotAFunction(String),
    /// A function was used as a value, or assigned to.
    NotAValue(String),
    /// A function was called with the wrong number of arguments.
    WrongArgumentCount {
        function: String,
        expected: usize,
        found: usize,
    },
    /// An argument's label didn't match its parameter's label. `None` means
    /// no label.
    WrongLabel {
        function: String,
        expected: Option<String>,
        found: Option<String>,
    },
    /// An argument had the wrong type.
    ArgumentTypeMismatch {
        function: String,
        /// The argument's label, or the parameter name if it has none.
        param: String,
        expected: Type,
        found: Type,
    },
    /// `return` was used outside a function.
    ReturnOutsideFunction,
    /// `return` had no value in a function that doesn't return `Void`.
    MissingReturnValue { function: String, expected: Type },
    /// A returned value had the wrong type.
    ReturnTypeMismatch {
        function: String,
        expected: Type,
        found: Type,
    },
    /// A function that doesn't return `Void` can finish without returning.
    MissingReturn { function: String, expected: Type },
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Undeclared(name) => write!(f, "`{name}` is not declared"),
            Self::Redeclared(name) => write!(f, "`{name}` is already declared"),
            Self::AssignToConstant(name) => {
                write!(
                    f,
                    "cannot assign to `{name}` because it was declared with `let`"
                )
            }
            Self::InvalidOperand {
                op,
                expected,
                found,
            } => write!(f, "operator `{op}` expects `{expected}`, found `{found}`"),
            Self::AssignTypeMismatch {
                name,
                expected,
                found,
            } => write!(
                f,
                "cannot assign `{found}` to `{name}`, which has type `{expected}`"
            ),
            Self::ConditionNotBool(found) => {
                write!(f, "`if` condition must be `Bool`, found `{found}`")
            }
            Self::UnknownType(name) => write!(f, "unknown type `{name}`"),
            Self::NotAFunction(name) => write!(f, "`{name}` is not a function"),
            Self::NotAValue(name) => write!(f, "`{name}` is a function, not a value"),
            Self::WrongArgumentCount {
                function,
                expected,
                found,
            } => {
                let s = if *expected == 1 { "" } else { "s" };
                write!(
                    f,
                    "`{function}` takes {expected} argument{s}, found {found}"
                )
            }
            Self::WrongLabel {
                function,
                expected,
                found,
            } => match (expected, found) {
                (Some(expected), Some(found)) => write!(
                    f,
                    "argument to `{function}` should be labelled `{expected}`, found `{found}`"
                ),
                (Some(expected), None) => write!(
                    f,
                    "argument to `{function}` is missing the label `{expected}`"
                ),
                (None, Some(found)) => write!(
                    f,
                    "argument to `{function}` should not have a label, found `{found}`"
                ),
                (None, None) => write!(f, "argument to `{function}` has the wrong label"),
            },
            Self::ArgumentTypeMismatch {
                function,
                param,
                expected,
                found,
            } => write!(
                f,
                "argument `{param}` of `{function}` expects `{expected}`, found `{found}`"
            ),
            Self::ReturnOutsideFunction => {
                write!(f, "`return` can only be used inside a function")
            }
            Self::MissingReturnValue { function, expected } => {
                write!(f, "`{function}` must return a value of type `{expected}`")
            }
            Self::ReturnTypeMismatch {
                function,
                expected,
                found,
            } => write!(f, "`{function}` must return `{expected}`, found `{found}`"),
            Self::MissingReturn { function, expected } => write!(
                f,
                "`{function}` must return a value of type `{expected}` on every path"
            ),
        }
    }
}

pub fn check(program: &Program) -> Result<(), Vec<CheckError>> {
    let mut checker = Checker::default();
    checker.block(program);

    if checker.errors.is_empty() {
        Ok(())
    } else {
        Err(checker.errors)
    }
}

#[derive(Debug, Clone)]
struct Signature<'a> {
    params: Vec<ParamSignature<'a>>,
    /// `None` if the return type is unknown.
    returns: Option<Type>,
}

#[derive(Debug, Clone)]
struct ParamSignature<'a> {
    label: Option<&'a str>,
    name: &'a str,
    /// `None` if the parameter's type is unknown.
    ty: Option<Type>,
}

#[derive(Debug, Clone, Copy)]
enum Binding {
    Value {
        mutability: Mutability,
        /// `None` if the type couldn't be determined because of an error, so
        /// that later uses don't report errors caused by the same mistake.
        ty: Option<Type>,
    },
    /// An index into `Checker::functions`.
    Function(usize),
}

/// The function whose body is being checked.
#[derive(Debug, Clone, Copy)]
struct FnContext<'a> {
    name: &'a str,
    /// `None` if the return type is unknown.
    returns: Option<Type>,
}

#[derive(Default)]
struct Checker<'a> {
    /// One scope per enclosing block, innermost last.
    scopes: Vec<HashMap<&'a str, Binding>>,
    /// The index in `scopes` of the current function's outermost scope, or 0
    /// outside of a function. Names can't be redeclared from here inwards.
    fn_scope_start: usize,
    current_fn: Option<FnContext<'a>>,
    functions: Vec<Signature<'a>>,
    errors: Vec<CheckError>,
}

impl<'a> Checker<'a> {
    fn lookup(&self, name: &str) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
    }

    fn declare(&mut self, name: &'a str, binding: Binding) {
        // Names can't be redeclared within a function, even to shadow an
        // outer block. A function's own scope can reuse names from outside it.
        if self.scopes[self.fn_scope_start..]
            .iter()
            .any(|scope| scope.contains_key(name))
        {
            self.errors.push(CheckError::Redeclared(name.to_string()));
        }

        let scope = self.scopes.last_mut().expect("always in a scope");
        scope.insert(name, binding);
    }

    fn resolve_type(&mut self, name: &str) -> Option<Type> {
        let ty = Type::from_name(name);
        if ty.is_none() {
            self.errors.push(CheckError::UnknownType(name.to_string()));
        }
        ty
    }

    /// Checks `stmts` in a new scope, so their declarations aren't visible
    /// afterwards.
    fn block(&mut self, stmts: &'a [Stmt]) {
        self.scopes.push(HashMap::new());
        for stmt in stmts {
            self.stmt(stmt);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Decl {
                mutability,
                name,
                ty: type_name,
                value,
            } => {
                // Checked first so that `let x = x` refers to an earlier `x`.
                let found = self.expr(value);

                // An explicit type takes precedence over the value's type, so
                // later uses are checked against what the programmer wrote.
                let ty = match type_name {
                    Some(type_name) => {
                        let expected = self.resolve_type(type_name);
                        self.check_assignable(name, expected, found);
                        expected
                    }
                    None => found,
                };

                let binding = Binding::Value {
                    mutability: *mutability,
                    ty,
                };
                self.declare(name, binding);
            }
            Stmt::Assign { name, value } => {
                let found = self.expr(value);
                match self.lookup(name) {
                    Some(Binding::Value {
                        mutability: Mutability::Var,
                        ty: expected,
                    }) => self.check_assignable(name, expected, found),
                    Some(Binding::Value {
                        mutability: Mutability::Let,
                        ..
                    }) => {
                        self.errors.push(CheckError::AssignToConstant(name.clone()));
                    }
                    Some(Binding::Function(_)) => {
                        self.errors.push(CheckError::NotAValue(name.clone()));
                    }
                    None => self.errors.push(CheckError::Undeclared(name.clone())),
                }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
            } => {
                if let Some(found) = self.expr(cond)
                    && found != Type::Bool
                {
                    self.errors.push(CheckError::ConditionNotBool(found));
                }

                self.block(then_block);
                if let Some(else_block) = else_block {
                    self.block(else_block);
                }
            }
            Stmt::Fn {
                name,
                params,
                ret,
                body,
            } => {
                let params: Vec<_> = params
                    .iter()
                    .map(|param| ParamSignature {
                        label: param.label.as_deref(),
                        name: &param.name,
                        ty: self.resolve_type(&param.ty),
                    })
                    .collect();
                let returns = self.resolve_type(ret);

                // Declared before the body is checked, so it can call itself.
                self.functions.push(Signature {
                    params: params.clone(),
                    returns,
                });
                self.declare(name, Binding::Function(self.functions.len() - 1));

                // The body can see everything declared so far, but starts a
                // new function scope holding the parameters.
                let outer_scope_start =
                    std::mem::replace(&mut self.fn_scope_start, self.scopes.len());
                let outer_fn = self.current_fn.replace(FnContext { name, returns });
                self.scopes.push(HashMap::new());

                for param in params {
                    let binding = Binding::Value {
                        mutability: Mutability::Let,
                        ty: param.ty,
                    };
                    self.declare(param.name, binding);
                }
                for stmt in body {
                    self.stmt(stmt);
                }

                self.scopes.pop();
                self.current_fn = outer_fn;
                self.fn_scope_start = outer_scope_start;

                // A `Void` function can finish without an explicit `return`.
                if let Some(expected) = returns
                    && expected != Type::Void
                    && !always_returns(body)
                {
                    self.errors.push(CheckError::MissingReturn {
                        function: name.clone(),
                        expected,
                    });
                }
            }
            Stmt::Return(value) => {
                let found = value.as_ref().map(|value| self.expr(value));
                let Some(FnContext { name, returns }) = self.current_fn else {
                    self.errors.push(CheckError::ReturnOutsideFunction);
                    return;
                };

                // A bare `return` returns `Void`.
                let function = name.to_string();
                match (returns, found) {
                    (Some(expected), None) if expected != Type::Void => {
                        self.errors
                            .push(CheckError::MissingReturnValue { function, expected });
                    }
                    (Some(expected), Some(Some(found))) if expected != found => {
                        self.errors.push(CheckError::ReturnTypeMismatch {
                            function,
                            expected,
                            found,
                        });
                    }
                    _ => {}
                }
            }
            Stmt::Expr(expr) => match expr {
                // A call's return value can be ignored.
                Expr::Call { callee, args } => {
                    self.call(callee, args);
                }
                _ => {
                    self.expr(expr);
                }
            },
        }
    }

    /// Returns the type of `expr`, or `None` if it can't be determined
    /// because of an error that has already been reported.
    fn expr(&mut self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Int(_) => Some(Type::Int),
            Expr::Bool(_) => Some(Type::Bool),
            Expr::Var(name) => match self.lookup(name) {
                Some(Binding::Value { ty, .. }) => ty,
                Some(Binding::Function(_)) => {
                    self.errors.push(CheckError::NotAValue(name.clone()));
                    None
                }
                None => {
                    self.errors.push(CheckError::Undeclared(name.clone()));
                    None
                }
            },
            // Operators always produce their result type, even if an operand
            // is invalid, so one bad operand doesn't cause further errors.
            Expr::Unary(op, operand) => {
                let ty = unary_type(op);
                self.operand(op.symbol(), operand, ty);
                Some(ty)
            }
            Expr::Binary(lhs, op, rhs) => {
                let ty = binary_type(op);
                self.operand(op.symbol(), lhs, ty);
                self.operand(op.symbol(), rhs, ty);
                Some(ty)
            }
            Expr::Call { callee, args } => self.call(callee, args),
        }
    }

    /// Checks a call, returning the function's return type, or `None` if it
    /// can't be determined.
    fn call(&mut self, callee: &str, args: &[Arg]) -> Option<Type> {
        // Arguments are checked first, so their errors come before the call's.
        let found: Vec<_> = args.iter().map(|arg| self.expr(&arg.value)).collect();

        let index = match self.lookup(callee) {
            Some(Binding::Function(index)) => index,
            Some(Binding::Value { .. }) => {
                self.errors
                    .push(CheckError::NotAFunction(callee.to_string()));
                return None;
            }
            None => {
                self.errors.push(CheckError::Undeclared(callee.to_string()));
                return None;
            }
        };
        let signature = self.functions[index].clone();

        if args.len() != signature.params.len() {
            self.errors.push(CheckError::WrongArgumentCount {
                function: callee.to_string(),
                expected: signature.params.len(),
                found: args.len(),
            });
            return signature.returns;
        }

        for ((arg, found), param) in args.iter().zip(found).zip(&signature.params) {
            if arg.label.as_deref() != param.label {
                self.errors.push(CheckError::WrongLabel {
                    function: callee.to_string(),
                    expected: param.label.map(str::to_string),
                    found: arg.label.clone(),
                });
            } else if let (Some(expected), Some(found)) = (param.ty, found)
                && expected != found
            {
                self.errors.push(CheckError::ArgumentTypeMismatch {
                    function: callee.to_string(),
                    param: param.label.unwrap_or(param.name).to_string(),
                    expected,
                    found,
                });
            }
        }

        signature.returns
    }

    /// Reports an error if a value of type `found` can't be given to `name`,
    /// whose type is `expected`. Unknown types are assumed to be fine.
    fn check_assignable(&mut self, name: &str, expected: Option<Type>, found: Option<Type>) {
        if let (Some(expected), Some(found)) = (expected, found)
            && expected != found
        {
            self.errors.push(CheckError::AssignTypeMismatch {
                name: name.to_string(),
                expected,
                found,
            });
        }
    }

    fn operand(&mut self, op: &'static str, operand: &Expr, expected: Type) {
        if let Some(found) = self.expr(operand)
            && found != expected
        {
            self.errors.push(CheckError::InvalidOperand {
                op,
                expected,
                found,
            });
        }
    }
}

/// Whether `stmts` returns on every path. Both branches of an `if` must return
/// for it to count, so an `if` without an `else` never does.
fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(_) => true,
        Stmt::If {
            then_block,
            else_block: Some(else_block),
            ..
        } => always_returns(then_block) && always_returns(else_block),
        _ => false,
    })
}

/// The type of a unary operator's operand, which is also its result type.
fn unary_type(op: &UnOp) -> Type {
    match op {
        UnOp::Neg => Type::Int,
        UnOp::Not => Type::Bool,
    }
}

/// The type of a binary operator's operands, which is also its result type.
fn binary_type(op: &BinOp) -> Type {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => Type::Int,
        BinOp::And | BinOp::Or => Type::Bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn check_src(src: &str) -> Result<(), Vec<CheckError>> {
        check(&parse(src).expect("source should parse"))
    }

    fn invalid_operand(op: &'static str) -> CheckError {
        CheckError::InvalidOperand {
            op,
            expected: Type::Int,
            found: Type::Bool,
        }
    }

    #[test]
    fn accepts_valid_program() {
        assert_eq!(check_src("let x = 1\nvar y = x\ny = y + x\ny = -y"), Ok(()));
    }

    #[test]
    fn accepts_booleans() {
        assert_eq!(check_src("let t = true\nvar b = t\nb = false"), Ok(()));
    }

    #[test]
    fn accepts_boolean_operators() {
        assert_eq!(
            check_src("let t = true\nvar b = !t & (false | t)\nb = !!b | b"),
            Ok(())
        );
    }

    #[test]
    fn rejects_boolean_operators_on_integers() {
        let invalid = |op| CheckError::InvalidOperand {
            op,
            expected: Type::Bool,
            found: Type::Int,
        };

        assert_eq!(check_src("let x = !1"), Err(vec![invalid("!")]));
        assert_eq!(check_src("let x = true & 1"), Err(vec![invalid("&")]));
        assert_eq!(
            check_src("let x = 1 | 2"),
            Err(vec![invalid("|"), invalid("|")])
        );
    }

    #[test]
    fn boolean_operators_produce_booleans() {
        assert_eq!(
            check_src("let x = (true | false) + 1"),
            Err(vec![invalid_operand("+")])
        );
        assert_eq!(
            check_src("var n = 0\nn = !false"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "n".to_string(),
                expected: Type::Int,
                found: Type::Bool,
            }])
        );
    }

    #[test]
    fn rejects_mixing_negations() {
        // `-` needs an `Int` and `!` needs a `Bool`, so `-!x` and `!-x` can
        // never be valid.
        assert_eq!(check_src("let x = -!true"), Err(vec![invalid_operand("-")]));
    }

    #[test]
    fn rejects_assignment_without_declaration() {
        assert_eq!(
            check_src("x = 1"),
            Err(vec![CheckError::Undeclared("x".to_string())])
        );
    }

    #[test]
    fn rejects_assignment_to_constant() {
        assert_eq!(
            check_src("let x = 1\nx = 2"),
            Err(vec![CheckError::AssignToConstant("x".to_string())])
        );
    }

    #[test]
    fn rejects_use_of_undeclared_name() {
        assert_eq!(
            check_src("let x = y + 1"),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
    }

    #[test]
    fn rejects_self_reference_in_declaration() {
        assert_eq!(
            check_src("var x = x"),
            Err(vec![CheckError::Undeclared("x".to_string())])
        );
    }

    #[test]
    fn rejects_redeclaration() {
        for src in [
            "let x = 1\nlet x = 2",
            "var x = 1\nlet x = 2",
            "let x = 1\nvar x = 2",
        ] {
            assert_eq!(
                check_src(src),
                Err(vec![CheckError::Redeclared("x".to_string())]),
                "{src:?}"
            );
        }
    }

    #[test]
    fn rejects_arithmetic_on_booleans() {
        assert_eq!(
            check_src("let x = true + 1"),
            Err(vec![invalid_operand("+")])
        );
        assert_eq!(
            check_src("let x = 1 - false"),
            Err(vec![invalid_operand("-")])
        );
        assert_eq!(check_src("let x = -true"), Err(vec![invalid_operand("-")]));
        assert_eq!(
            check_src("let x = 4 / (true)"),
            Err(vec![invalid_operand("/")])
        );
        assert_eq!(
            check_src("let b = true\nlet x = b * b"),
            Err(vec![invalid_operand("*"), invalid_operand("*")])
        );
    }

    #[test]
    fn rejects_assignment_of_different_type() {
        assert_eq!(
            check_src("var x = 1\nx = true"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "x".to_string(),
                expected: Type::Int,
                found: Type::Bool,
            }])
        );
        assert_eq!(
            check_src("var b = false\nb = 1 + 2"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "b".to_string(),
                expected: Type::Bool,
                found: Type::Int,
            }])
        );
    }

    #[test]
    fn does_not_cascade_errors() {
        // `x` has no known type, so later uses of it report nothing further.
        assert_eq!(
            check_src("var x = y\nlet z = x + 1\nx = true"),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
        assert_eq!(
            check_src("let x = -true\nlet y = x + 1"),
            Err(vec![invalid_operand("-")])
        );
    }

    #[test]
    fn type_errors_use_type_names() {
        let messages = |src| {
            check_src(src)
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            messages("let x = true + 1"),
            ["operator `+` expects `Int`, found `Bool`"]
        );
        assert_eq!(
            messages("var b = false\nb = 1"),
            ["cannot assign `Int` to `b`, which has type `Bool`"]
        );
        assert_eq!(
            messages("if 1 {}"),
            ["`if` condition must be `Bool`, found `Int`"]
        );
        assert_eq!(messages("let x: int = 1"), ["unknown type `int`"]);
    }

    #[test]
    fn accepts_matching_explicit_types() {
        assert_eq!(
            check_src(
                "
                let x: Int = 3
                var y: Bool = true
                var z: Int = x * 2
                y = !y
                z = 4
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_value_of_wrong_explicit_type() {
        assert_eq!(
            check_src("let x: Int = true"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "x".to_string(),
                expected: Type::Int,
                found: Type::Bool,
            }])
        );
        assert_eq!(
            check_src("var y: Bool = 1 + 2"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "y".to_string(),
                expected: Type::Bool,
                found: Type::Int,
            }])
        );
    }

    #[test]
    fn rejects_unknown_types() {
        for name in ["Foo", "int", "bool", "x"] {
            assert_eq!(
                check_src(&format!("let x: {name} = 1")),
                Err(vec![CheckError::UnknownType(name.to_string())]),
                "{name:?}"
            );
        }
    }

    #[test]
    fn explicit_type_is_used_after_a_mismatch() {
        // `x` is an `Int` as written, so only the declaration is an error.
        assert_eq!(
            check_src("var x: Int = true\nx = 1\nlet y = x + 1"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "x".to_string(),
                expected: Type::Int,
                found: Type::Bool,
            }])
        );
    }

    #[test]
    fn unknown_type_does_not_cascade() {
        assert_eq!(
            check_src("var x: Foo = 1\nx = true\nlet y = x + 1"),
            Err(vec![CheckError::UnknownType("Foo".to_string())])
        );
    }

    #[test]
    fn type_names_are_not_reserved() {
        assert_eq!(check_src("let Int: Int = 1\nlet Bool: Bool = true"), Ok(()));
    }

    #[test]
    fn accepts_if_else() {
        assert_eq!(
            check_src(
                "
                let a = true
                var x = 0
                if a {
                    x = 1
                } else if !a {
                    x = 2
                } else {
                    x = 3
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_non_boolean_condition() {
        assert_eq!(
            check_src("if 1 + 2 {}"),
            Err(vec![CheckError::ConditionNotBool(Type::Int)])
        );
        assert_eq!(
            check_src("if true {} else if 0 {}"),
            Err(vec![CheckError::ConditionNotBool(Type::Int)])
        );
        // An undeclared condition is only reported once.
        assert_eq!(
            check_src("if c {}"),
            Err(vec![CheckError::Undeclared("c".to_string())])
        );
    }

    #[test]
    fn checks_statements_inside_blocks() {
        assert_eq!(
            check_src(
                "
                let x = 1
                if true {
                    x = 2
                } else {
                    let y = z
                }
                "
            ),
            Err(vec![
                CheckError::AssignToConstant("x".to_string()),
                CheckError::Undeclared("z".to_string()),
            ])
        );
    }

    #[test]
    fn blocks_can_use_and_assign_outer_names() {
        assert_eq!(
            check_src(
                "
                let a = 1
                var b = 2
                if true {
                    if true {
                        b = a + b
                    }
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn block_declarations_are_not_visible_outside() {
        assert_eq!(
            check_src(
                "
                if true {
                    let y = 1
                }
                let z = y
                "
            ),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
        assert_eq!(
            check_src(
                "
                if true {
                    let y = 1
                } else {
                    y = 2
                }
                "
            ),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
    }

    #[test]
    fn separate_blocks_can_reuse_names() {
        assert_eq!(
            check_src(
                "
                if true {
                    let y = 1
                } else {
                    let y = false
                }
                let y = 2
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_shadowing_outer_names() {
        assert_eq!(
            check_src(
                "
                let x = 1
                if true {
                    let x = 2
                }
                "
            ),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
    }

    #[test]
    fn reports_every_error() {
        assert_eq!(
            check_src("let x = 1\nx = 2\ny = z"),
            Err(vec![
                CheckError::AssignToConstant("x".to_string()),
                CheckError::Undeclared("z".to_string()),
                CheckError::Undeclared("y".to_string()),
            ])
        );
    }

    fn messages(src: &str) -> Vec<String> {
        match check_src(src) {
            Ok(()) => vec![],
            Err(errs) => errs.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn accepts_function_definition_and_calls() {
        assert_eq!(
            check_src(
                "
                fn addFive(to value: Int) -> Int {
                    return value + 5
                }
                let x: Int = addFive(to: 3)
                let y = addFive(to: addFive(to: x))
                addFive(to: 1) // The result can be ignored.
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_all_label_forms() {
        assert_eq!(
            check_src(
                "
                fn f(a: Int, to b: Bool, _ c: Int) -> Bool {
                    return b & !b | b
                }
                let r: Bool = f(a: 1, to: true, 2)
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_function_without_return_type() {
        assert_eq!(
            check_src(
                "
                var count = 1
                fn reset() {
                    count = 0
                    return
                }
                reset()
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn functions_capture_earlier_names() {
        assert_eq!(
            check_src(
                "
                let step = 2
                var total = 0
                fn add(_ n: Int) {
                    total = total + n * step
                }
                add(3)
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn functions_cannot_capture_later_names() {
        assert_eq!(
            check_src(
                "
                fn f() -> Int {
                    return later
                }
                let later = 1
                "
            ),
            Err(vec![CheckError::Undeclared("later".to_string())])
        );
    }

    #[test]
    fn captured_constants_stay_constant() {
        assert_eq!(
            check_src(
                "
                let x = 1
                fn f() {
                    x = 2
                }
                "
            ),
            Err(vec![CheckError::AssignToConstant("x".to_string())])
        );
    }

    #[test]
    fn functions_can_recurse() {
        assert_eq!(
            check_src(
                "
                fn sum(upTo n: Int, acc: Int) -> Int {
                    return sum(upTo: n - 1, acc: acc + n)
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn functions_cannot_be_called_before_declaration() {
        assert_eq!(
            check_src(
                "
                let x = f()
                fn f() -> Int {
                    return 1
                }
                "
            ),
            Err(vec![CheckError::Undeclared("f".to_string())])
        );
    }

    #[test]
    fn parameters_are_constants() {
        assert_eq!(
            check_src(
                "
                fn f(x: Int) {
                    x = 2
                }
                "
            ),
            Err(vec![CheckError::AssignToConstant("x".to_string())])
        );
    }

    #[test]
    fn function_scope_is_not_visible_outside() {
        assert_eq!(
            check_src(
                "
                fn f(x: Int) {
                    let y = x
                }
                let a = x
                let b = y
                "
            ),
            Err(vec![
                CheckError::Undeclared("x".to_string()),
                CheckError::Undeclared("y".to_string()),
            ])
        );
    }

    #[test]
    fn functions_can_reuse_outer_names() {
        assert_eq!(
            check_src(
                "
                let x = true
                fn f(x: Int) -> Int {
                    let y = x + 1
                    return y
                }
                let y = false
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_redeclaration_within_function() {
        assert_eq!(
            check_src("fn f(x: Int, x: Int) {}"),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
        assert_eq!(
            check_src(
                "
                fn f(x: Int) {
                    let x = 1
                }
                "
            ),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
        // Blocks inside a function still can't shadow.
        assert_eq!(
            check_src(
                "
                fn f(x: Int) {
                    if true {
                        let x = 1
                    }
                }
                "
            ),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
        // Functions share names with values.
        assert_eq!(
            check_src("let f = 1\nfn f() {}"),
            Err(vec![CheckError::Redeclared("f".to_string())])
        );
    }

    #[test]
    fn rejects_wrong_argument_count() {
        assert_eq!(
            messages("fn f(a: Int) {}\nf()\nf(a: 1, a: 2)"),
            [
                "`f` takes 1 argument, found 0",
                "`f` takes 1 argument, found 2",
            ]
        );
        assert_eq!(
            messages("fn g() {}\ng(1)"),
            ["`g` takes 0 arguments, found 1"]
        );
    }

    #[test]
    fn rejects_wrong_labels() {
        assert_eq!(
            messages(
                "
                fn f(to value: Int, _ other: Int) {}
                f(from: 1, 2)
                f(1, 2)
                f(to: 1, other: 2)
                f(value: 1, 2)
                "
            ),
            [
                "argument to `f` should be labelled `to`, found `from`",
                "argument to `f` is missing the label `to`",
                "argument to `f` should not have a label, found `other`",
                "argument to `f` should be labelled `to`, found `value`",
            ]
        );
    }

    #[test]
    fn labels_must_be_in_order() {
        assert_eq!(
            messages("fn f(a: Int, b: Int) {}\nf(b: 1, a: 2)"),
            [
                "argument to `f` should be labelled `a`, found `b`",
                "argument to `f` should be labelled `b`, found `a`",
            ]
        );
    }

    #[test]
    fn rejects_wrong_argument_types() {
        assert_eq!(
            messages("fn f(to value: Int, _ flag: Bool) {}\nf(to: true, 1)"),
            [
                "argument `to` of `f` expects `Int`, found `Bool`",
                "argument `flag` of `f` expects `Bool`, found `Int`",
            ]
        );
    }

    #[test]
    fn checks_arguments_even_when_call_is_wrong() {
        assert_eq!(
            check_src("fn f() {}\nf(x)\nlet y = 1\ny(z)"),
            Err(vec![
                CheckError::Undeclared("x".to_string()),
                CheckError::WrongArgumentCount {
                    function: "f".to_string(),
                    expected: 0,
                    found: 1,
                },
                CheckError::Undeclared("z".to_string()),
                CheckError::NotAFunction("y".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_using_functions_as_values() {
        assert_eq!(
            messages("fn f() {}\nlet x = f\nf = 2"),
            [
                "`f` is a function, not a value",
                "`f` is a function, not a value",
            ]
        );
    }

    #[test]
    fn rejects_calling_undeclared_or_non_functions() {
        assert_eq!(
            messages("let x = 1\nx()\ng()"),
            ["`x` is not a function", "`g` is not declared"]
        );
    }

    #[test]
    fn functions_without_return_type_return_void() {
        assert_eq!(
            messages(
                "
                fn f() {}
                let x = f()
                let y: Void = x
                let z = 1 + f()
                let w: Int = f()
                if f() {}
                "
            ),
            [
                "operator `+` expects `Int`, found `Void`",
                "cannot assign `Void` to `w`, which has type `Int`",
                "`if` condition must be `Bool`, found `Void`",
            ]
        );
    }

    #[test]
    fn return_type_is_used_by_callers() {
        assert_eq!(
            messages(
                "
                fn f() -> Bool {
                    return true
                }
                let x = f() + 1
                let y: Int = f()
                "
            ),
            [
                "operator `+` expects `Int`, found `Bool`",
                "cannot assign `Bool` to `y`, which has type `Int`",
            ]
        );
    }

    #[test]
    fn rejects_wrong_return_values() {
        assert_eq!(
            messages(
                "
                fn a() -> Int {
                    return true
                }
                fn b() -> Int {
                    return
                }
                fn c() {
                    return 1
                }
                "
            ),
            [
                "`a` must return `Int`, found `Bool`",
                "`b` must return a value of type `Int`",
                "`c` must return `Void`, found `Int`",
            ]
        );
    }

    #[test]
    fn explicit_void_is_the_same_as_no_return_type() {
        assert_eq!(
            check_src(
                "
                fn a() -> Void {}
                fn b() {}
                fn c(flag: Bool) -> Void {
                    if flag {
                        return
                    }
                }
                let x: Void = a()
                let y: Void = b()
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn void_functions_can_return_void_values() {
        assert_eq!(
            check_src(
                "
                fn log() {}
                fn wrapper() -> Void {
                    return log()
                }
                fn other() {
                    return wrapper()
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn void_can_be_used_like_other_types() {
        assert_eq!(
            check_src(
                "
                fn take(_ v: Void) -> Void {
                    return v
                }
                fn nothing() {}
                take(nothing())
                "
            ),
            Ok(())
        );
        assert_eq!(
            messages("fn take(_ v: Void) {}\ntake(1)"),
            ["argument `v` of `take` expects `Void`, found `Int`"]
        );
    }

    #[test]
    fn non_void_function_cannot_return_void() {
        assert_eq!(
            messages(
                "
                fn log() {}
                fn f() -> Int {
                    return log()
                }
                "
            ),
            ["`f` must return `Int`, found `Void`"]
        );
    }

    #[test]
    fn rejects_return_outside_function() {
        assert_eq!(
            messages(
                "
                return
                if true {
                    return 1
                }
                "
            ),
            [
                "`return` can only be used inside a function",
                "`return` can only be used inside a function",
            ]
        );
    }

    #[test]
    fn requires_return_on_every_path() {
        let missing = |function: &str| CheckError::MissingReturn {
            function: function.to_string(),
            expected: Type::Int,
        };

        assert_eq!(check_src("fn f() -> Int {}"), Err(vec![missing("f")]));
        assert_eq!(
            check_src(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        return 1
                    }
                }
                "
            ),
            Err(vec![missing("f")])
        );
        assert_eq!(
            check_src(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        return 1
                    } else if !c {
                        return 2
                    }
                }
                "
            ),
            Err(vec![missing("f")])
        );
    }

    #[test]
    fn accepts_return_on_every_path() {
        assert_eq!(
            check_src(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        return 1
                    } else if !c {
                        return 2
                    } else {
                        return 3
                    }
                }
                fn g(c: Bool) -> Int {
                    if c {
                        return 1
                    }
                    return 2
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn nested_functions_have_their_own_return_type() {
        assert_eq!(
            check_src(
                "
                fn outer(x: Int) -> Bool {
                    fn inner() -> Int {
                        return x
                    }
                    // `inner`'s `return` is checked against `Int`, and this
                    // one against `Bool`.
                    return inner()
                }
                "
            ),
            Err(vec![CheckError::ReturnTypeMismatch {
                function: "outer".to_string(),
                expected: Type::Bool,
                found: Type::Int,
            }])
        );
    }
}
