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

//! Checking statements inside functions.

use super::*;

impl<'a> Checker<'a> {
    /// The type of the top-level value at `index` in `globals`, checking its
    /// declaration if that hasn't been done yet.
    pub(super) fn global_type(&mut self, index: usize) -> Option<Type> {
        let global = &self.globals[index];
        match &global.state {
            LazyType::Checked(ty) => return ty.clone(),
            LazyType::Checking => {
                let error = CheckError::CyclicValue(global.name.to_string());
                self.error_in(global.file, global.span, error);
                return None;
            }
            LazyType::Unchecked => {}
        }

        // Checked at the top level of its own file, even when another file
        // needs it first.
        let (file, name, type_name, value) = (global.file, global.name, global.ty, global.value);
        self.globals[index].state = LazyType::Checking;
        let Typed { ty, expr } = self.at_top_level(file, None, |checker| {
            checker.decl_type(name, type_name, value)
        });

        self.globals[index].state = LazyType::Checked(ty.clone());
        self.globals[index].lowered = Some(expr);
        self.global_order.push(index);
        ty
    }
    /// Checks a `let` or `var` declaration's value, returning the type it's
    /// declared with, along with what the value compiles to.
    pub(super) fn decl_type(
        &mut self,
        name: &str,
        type_name: Option<&TypeName>,
        value: &Expr,
    ) -> Typed {
        // Checked before it's declared, so that `let x = x` refers to an
        // earlier `x`. The value takes its type from the explicit type, if
        // there is one.
        let expected = type_name.and_then(|type_name| self.resolve_type(type_name));
        let Typed { ty: found, expr } = self.expr_as(value, expected.as_ref());

        // An explicit type takes precedence over the value's type, so later
        // uses are checked against what the programmer wrote.
        let ty = match type_name {
            Some(_) => {
                self.check_assignable(name, value.span, expected.clone(), found);
                expected
            }
            None => found,
        };
        Typed { ty, expr }
    }
    /// Checks `stmts` in a new scope, so their declarations aren't visible
    /// afterwards, returning what they compile to, and whether they always
    /// return.
    pub(super) fn block(&mut self, stmts: &'a [Stmt]) -> (Vec<ir::Stmt>, bool) {
        self.cx.scopes.push(HashMap::new());
        let checked = self.stmts(stmts);
        self.cx.scopes.pop();
        checked
    }
    /// Checks `stmts` in the current scope, returning what they compile to,
    /// and whether they always return, which they do if any of them does.
    pub(super) fn stmts(&mut self, stmts: &'a [Stmt]) -> (Vec<ir::Stmt>, bool) {
        let mut lowered = vec![];
        let mut returns = false;
        for stmt in stmts {
            let (checked, always_returns) = self.stmt(stmt);
            lowered.extend(checked);
            returns |= always_returns;
        }
        (lowered, returns)
    }
    /// Checks a statement inside a function, returning what it compiles to,
    /// or `None` if it declares a function or group, which compile
    /// separately, or if it has an error. Also returns whether it returns on
    /// every path, which it does if it evaluates a `Never`, since that never
    /// finishes. An `if` does if both of its blocks do, so one without an
    /// `else` never does, and a `match` does if every arm does, which is
    /// enough since every value is matched.
    pub(super) fn stmt(&mut self, stmt: &'a Stmt) -> (Option<ir::Stmt>, bool) {
        match &stmt.kind {
            StmtKind::Use(_)
            | StmtKind::Impl { .. }
            | StmtKind::Trait { .. }
            | StmtKind::Primitive { .. } => {
                unreachable!(
                    "the parser only allows `use`, `impl`, `trait` and `primitive` at the top level"
                )
            }
            StmtKind::Decl {
                mutability,
                name,
                ty: type_name,
                value,
            } => {
                let (Typed { ty, expr }, never) =
                    self.evaluating(|checker| checker.decl_type(name, type_name.as_ref(), value));
                let local = self.declare_local(name, stmt.span, *mutability, ty);
                (Some(ir::Stmt::Assign { local, value: expr }), never)
            }
            StmtKind::Assign { name, value } => {
                let target = Name {
                    span: stmt.span,
                    ..Name::new(name)
                };
                let error = match self.lookup(&target) {
                    Ok(Binding::Value {
                        mutability: Mutability::Var,
                        ty: expected,
                        local,
                    }) => {
                        // The value takes its type from the variable.
                        let (found, never) =
                            self.evaluating(|checker| checker.expr_as(value, expected.as_ref()));
                        self.check_assignable(name, value.span, expected, found.ty);
                        let value = found.expr;
                        return (Some(ir::Stmt::Assign { local, value }), never);
                    }
                    Ok(Binding::Value {
                        mutability: Mutability::Let,
                        ..
                    }) => Some(CheckError::AssignToConstant(name.clone())),
                    Ok(Binding::Function(_)) => Some(CheckError::AssignToFunction(name.clone())),
                    Ok(Binding::Type(_) | Binding::Alias(_) | Binding::Generic(_)) => {
                        Some(CheckError::TypeAsValue(name.clone()))
                    }
                    Ok(Binding::Trait(_)) => Some(CheckError::TraitAsValue(name.clone())),
                    // `lookup` never finds top-level values from inside a
                    // function, which is the only place assignments can be.
                    Ok(Binding::Global(_)) => {
                        self.lookup_error(&target, LookupError::Captured);
                        None
                    }
                    Err(error) => {
                        self.lookup_error(&target, error);
                        None
                    }
                };
                if let Some(error) = error {
                    self.error(stmt.span, error);
                }
                // The value is still checked, so its errors are found.
                let (_, never) = self.evaluating(|checker| checker.expr(value));
                (None, never)
            }
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond_span = cond.span;
                let (cond, never) = self.evaluating(|checker| checker.expr(cond));
                if let Some(found) = &cond.ty
                    && *found != self.std_type("Bool")
                {
                    self.error(cond_span, CheckError::ConditionNotBool(found.clone()));
                }

                let (then_block, then_returns) = self.block(then_block);
                let (else_block, else_returns) = match else_block {
                    Some(else_block) => self.block(else_block),
                    None => (vec![], false),
                };
                let lowered = ir::Stmt::If {
                    cond: cond.expr,
                    then_block,
                    else_block,
                };
                (Some(lowered), never || then_returns && else_returns)
            }
            // Inside functions, functions and groups are only visible after
            // they're declared.
            StmtKind::Fn(function) => {
                // Declared before the body is checked, so it can call itself.
                let index = self.declare_fn(
                    &function.name,
                    function.span,
                    &function.params,
                    &function.ret,
                );
                // Only the top level can have a `primitive fn`, so it has a
                // body.
                let body = function.body.as_deref().unwrap_or_default();
                self.fn_body(
                    index,
                    &function.name,
                    function.span,
                    body,
                    FnKind::Function,
                    None,
                );
                (None, false)
            }
            StmtKind::Group { name, params, .. } | StmtKind::Choice { name, params, .. }
                if !params.is_empty() =>
            {
                self.error(stmt.span, CheckError::GenericInFunction(name.clone()));
                (None, false)
            }
            StmtKind::Group { name, .. } | StmtKind::Choice { name, .. } => {
                let id = self.declare_type(name, stmt);
                self.define_type(id, stmt);
                self.check_not_recursive(id, name, stmt.span);
                (None, false)
            }
            // Like other names inside functions, an alias is only visible
            // after it's declared, so it's worked out straight away.
            StmtKind::Alias(ast::Alias {
                name, ty: Some(ty), ..
            }) => {
                let target = self.resolve_type(ty);
                self.declare_alias(name, stmt.span, ty, LazyType::Checked(target));
                (None, false)
            }
            // Only a `trait`'s aliases have no type.
            StmtKind::Alias(_) => (None, false),
            StmtKind::Match { value, arms } => self.match_(stmt, value, arms),
            StmtKind::Destructure { pattern, value } => self.destructure(pattern, value),
            StmtKind::Return(value) => {
                let FnContext { name, returns, .. } = self
                    .cx
                    .current_fn
                    .clone()
                    .expect("statements are only checked inside functions");
                let found = value
                    .as_ref()
                    .map(|value| self.expr_as(value, returns.as_ref()));

                // A bare `return` returns `Void`.
                let function = name.to_string();
                match (returns, found.as_ref().map(|found| &found.ty)) {
                    (Some(expected), None) if expected != self.std_type("Void") => {
                        self.error(
                            stmt.span,
                            CheckError::MissingReturnValue { function, expected },
                        );
                    }
                    (Some(expected), Some(Some(found))) if expected != *found => {
                        self.error(
                            value.as_ref().map_or(stmt.span, |value| value.span),
                            CheckError::ReturnTypeMismatch {
                                function,
                                expected,
                                found: found.clone(),
                            },
                        );
                    }
                    _ => {}
                }
                let lowered = match found {
                    Some(found) => found.expr,
                    None => self.void(),
                };
                (Some(ir::Stmt::Return(lowered)), true)
            }
            StmtKind::Expr(expr) => {
                // A call's return value can be ignored.
                let (found, never) = self.evaluating(|checker| checker.expr(expr));
                (Some(ir::Stmt::Expr(found.expr)), never)
            }
        }
    }
    /// Checks expressions with `check`, also returning whether they evaluate
    /// a `Never`.
    pub(super) fn evaluating<T>(&mut self, check: impl FnOnce(&mut Self) -> T) -> (T, bool) {
        self.evaluated_never = false;
        let checked = check(self);
        (checked, self.evaluated_never)
    }
    /// Checks a `match` on `value`, whose arms' patterns must together match
    /// every value it could be, with each matching something the ones before
    /// it don't, returning what it compiles to.
    /// Also returns whether it always returns, as `stmt` does.
    pub(super) fn match_(
        &mut self,
        stmt: &'a Stmt,
        value: &Expr,
        arms: &'a [MatchArm],
    ) -> (Option<ir::Stmt>, bool) {
        let (Typed { ty, expr }, never) = self.evaluating(|checker| checker.expr(value));
        let mut lowered = vec![];
        let mut spaces = vec![];
        let mut returns = true;
        for arm in arms {
            // A pattern's bindings are only visible in its arm's block. Arms
            // are still checked when the type is unknown, so the errors in
            // their blocks are found.
            self.cx.scopes.push(HashMap::new());
            let (pattern, space) = self.pattern(&arm.pattern, ty.as_ref());
            let (body, arm_returns) = self.stmts(&arm.body);
            self.cx.scopes.pop();
            spaces.push(space);
            returns &= arm_returns;
            if let Some(pattern) = pattern {
                lowered.push(ir::MatchArm { pattern, body });
            }
        }
        let returns = never || returns;

        // Patterns with errors have already been reported, and can't be told
        // apart from patterns that match anything.
        let Some(ty) = ty else {
            return (None, returns);
        };
        if lowered.len() < arms.len() {
            return (None, returns);
        }
        self.check_exhaustive(&ty, stmt.span, arms, spaces);
        let lowered = ir::Stmt::Match {
            value: expr,
            ty: ty.id,
            arms: lowered,
        };
        (Some(lowered), returns)
    }
    /// Checks `pattern = value`, which declares the values that `pattern`
    /// binds in the current scope. With nothing to fall back on, `pattern`
    /// must match every value it could be given. It compiles to a `match`
    /// with one arm, whose block is empty.
    pub(super) fn destructure(
        &mut self,
        pattern: &'a Pattern,
        value: &Expr,
    ) -> (Option<ir::Stmt>, bool) {
        let (Typed { ty, expr }, never) = self.evaluating(|checker| checker.expr(value));
        (self.destructured(pattern, ty, expr), never)
    }
    /// Checks `pattern = value`, where `value` has the type `ty` and compiles
    /// to `expr`, returning what it compiles to.
    pub(super) fn destructured(
        &mut self,
        pattern: &'a Pattern,
        ty: Option<Type>,
        expr: ir::Expr,
    ) -> Option<ir::Stmt> {
        let (lowered, space) = self.pattern(pattern, ty.as_ref());
        let (ty, lowered) = (ty?, lowered?);
        let tys = std::slice::from_ref(&ty);
        if let Some(values) = self
            .unmatched(&[vec![space]], &[Space::Any], tys, 1)
            .first()
        {
            self.error(
                pattern.span,
                CheckError::RefutablePattern {
                    pattern: pattern.to_string(),
                    missing: self.show_space(&values[0], &ty),
                },
            );
            return None;
        }
        Some(ir::Stmt::Match {
            value: expr,
            ty: ty.id,
            arms: vec![ir::MatchArm {
                pattern: lowered,
                body: vec![],
            }],
        })
    }
    /// Reports an error if a value of type `found`, at `span`, can't be given to
    /// `name`, whose type is `expected`. Unknown types are assumed to be fine.
    pub(super) fn check_assignable(
        &mut self,
        name: &str,
        span: Span,
        expected: Option<Type>,
        found: Option<Type>,
    ) {
        if let (Some(expected), Some(found)) = (expected, found)
            && expected != found
        {
            self.error(
                span,
                CheckError::AssignTypeMismatch {
                    name: name.to_string(),
                    expected,
                    found,
                },
            );
        }
    }
}
