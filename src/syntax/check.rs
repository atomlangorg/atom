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
//! reassigned, and that every expression is well typed.

use std::{collections::HashMap, fmt};

use super::ast::{BinOp, Expr, Mutability, Program, Stmt, UnOp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Int => write!(f, "Int"),
            Self::Bool => write!(f, "Bool"),
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
    /// A `var` was assigned a value of a different type to its declaration.
    AssignTypeMismatch {
        name: String,
        expected: Type,
        found: Type,
    },
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
        }
    }
}

pub fn check(program: &Program) -> Result<(), Vec<CheckError>> {
    let mut checker = Checker::default();
    for stmt in program {
        checker.stmt(stmt);
    }

    if checker.errors.is_empty() {
        Ok(())
    } else {
        Err(checker.errors)
    }
}

#[derive(Debug, Clone, Copy)]
struct Binding {
    mutability: Mutability,
    /// `None` if the declared value had an error, so that later uses don't
    /// report errors caused by the same mistake.
    ty: Option<Type>,
}

#[derive(Default)]
struct Checker<'a> {
    names: HashMap<&'a str, Binding>,
    errors: Vec<CheckError>,
}

impl<'a> Checker<'a> {
    fn stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Decl {
                mutability,
                name,
                value,
            } => {
                // Checked first so that `let x = x` refers to an earlier `x`.
                let ty = self.expr(value);
                let binding = Binding {
                    mutability: *mutability,
                    ty,
                };
                if self.names.insert(name, binding).is_some() {
                    self.errors.push(CheckError::Redeclared(name.clone()));
                }
            }
            Stmt::Assign { name, value } => {
                let found = self.expr(value);
                match self.names.get(name.as_str()).copied() {
                    Some(Binding {
                        mutability: Mutability::Var,
                        ty: expected,
                    }) => {
                        if let (Some(expected), Some(found)) = (expected, found)
                            && expected != found
                        {
                            self.errors.push(CheckError::AssignTypeMismatch {
                                name: name.clone(),
                                expected,
                                found,
                            });
                        }
                    }
                    Some(Binding {
                        mutability: Mutability::Let,
                        ..
                    }) => {
                        self.errors.push(CheckError::AssignToConstant(name.clone()));
                    }
                    None => self.errors.push(CheckError::Undeclared(name.clone())),
                }
            }
        }
    }

    /// Returns the type of `expr`, or `None` if it can't be determined
    /// because of an error that has already been reported.
    fn expr(&mut self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Int(_) => Some(Type::Int),
            Expr::Bool(_) => Some(Type::Bool),
            Expr::Var(name) => match self.names.get(name.as_str()) {
                Some(binding) => binding.ty,
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
}
