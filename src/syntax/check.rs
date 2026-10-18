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

//! Checks that names are declared before use and that only `var`s are
//! reassigned.

use std::{collections::HashMap, fmt};

use super::ast::{Expr, Mutability, Program, Stmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A name was used or assigned without being declared.
    Undeclared(String),
    /// A name was declared twice.
    Redeclared(String),
    /// A `let` constant was assigned to.
    AssignToConstant(String),
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

#[derive(Default)]
struct Checker<'a> {
    names: HashMap<&'a str, Mutability>,
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
                self.expr(value);
                if self.names.insert(name, *mutability).is_some() {
                    self.errors.push(CheckError::Redeclared(name.clone()));
                }
            }
            Stmt::Assign { name, value } => {
                self.expr(value);
                match self.names.get(name.as_str()) {
                    Some(Mutability::Var) => {}
                    Some(Mutability::Let) => {
                        self.errors.push(CheckError::AssignToConstant(name.clone()));
                    }
                    None => self.errors.push(CheckError::Undeclared(name.clone())),
                }
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Int(_) => {}
            Expr::Var(name) => {
                if !self.names.contains_key(name.as_str()) {
                    self.errors.push(CheckError::Undeclared(name.clone()));
                }
            }
            Expr::Unary(_, operand) => self.expr(operand),
            Expr::Binary(lhs, _, rhs) => {
                self.expr(lhs);
                self.expr(rhs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn check_src(src: &str) -> Result<(), Vec<CheckError>> {
        check(&parse(src).expect("source should parse"))
    }

    #[test]
    fn accepts_valid_program() {
        assert_eq!(check_src("let x = 1\nvar y = x\ny = y + x\ny = -y"), Ok(()));
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
