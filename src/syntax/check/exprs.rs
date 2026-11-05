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

//! Checking expressions, other than calls.

use super::*;

/// `value`, the checked value for the option at `option` of the choice with
/// id `choice`, as the choice.
pub(super) fn chosen(choice: usize, option: usize, value: Typed) -> Typed {
    Typed {
        ty: value.ty,
        expr: ir::Expr::Choose {
            choice,
            option,
            value: Box::new(value.expr),
        },
    }
}

/// What `.member` is used on.
pub(super) enum Receiver {
    /// A value of the type, for its fields and methods, along with what it
    /// compiles to.
    Value(Type, ir::Expr),
    /// The type itself, for its static functions.
    Type(Type),
}

/// A field or method of a type, which are found with `.name` on a value.
/// Whether it's called is decided by the brackets after it, not by which kind
/// it is.
pub(super) enum Member {
    /// The field's index in the order its group declares them, and its type,
    /// which is `None` if it's unknown.
    Field(usize, Option<Type>),
    Method(FnRef),
}

/// The result of checking an expression.
pub(super) struct Typed {
    /// `None` if the type couldn't be determined because of an error that has
    /// already been reported.
    pub(super) ty: Option<Type>,
    /// What the expression compiles to, which is only compiled if the program
    /// has no errors.
    pub(super) expr: ir::Expr,
}

impl Typed {
    /// An expression whose error has already been reported.
    pub(super) const UNKNOWN: Self = Self {
        ty: None,
        expr: ir::Expr::Error,
    };

    /// An expression of type `ty` with an error, which has been reported.
    pub(super) fn error(ty: Type) -> Self {
        Self {
            ty: Some(ty),
            expr: ir::Expr::Error,
        }
    }
}

/// Calls the function at `function` in `Checker::callees` with `args`, which
/// is an error if the function couldn't be found.
pub(super) fn call(function: Option<usize>, args: Vec<ir::Expr>) -> ir::Expr {
    match function {
        Some(function) => ir::Expr::Call { function, args },
        None => ir::Expr::Error,
    }
}

/// Splits checked expressions into their types and what they compile to.
pub(super) fn split(typed: Vec<Typed>) -> (Vec<Option<Type>>, Vec<ir::Expr>) {
    typed.into_iter().map(|t| (t.ty, t.expr)).unzip()
}

/// Whether `expr` has a type of its own, rather than being made only of
/// integer literals, which take their type from where they're used.
pub(super) fn has_own_type(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Int(_) => false,
        ExprKind::Unary(UnOp::Neg | UnOp::BitNot, operand) | ExprKind::Paren(operand) => {
            has_own_type(operand)
        }
        ExprKind::Binary(
            lhs,
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor,
            rhs,
        ) => has_own_type(lhs) || has_own_type(rhs),
        // A shift has the type of what's shifted, whatever it's shifted by.
        ExprKind::Binary(lhs, BinOp::ShiftLeft | BinOp::ShiftRight, _) => has_own_type(lhs),
        _ => true,
    }
}

impl<'a> Checker<'a> {
    /// Checks `expr`, where no particular type is expected.
    pub(super) fn expr(&mut self, expr: &Expr) -> Typed {
        self.expr_as(expr, None)
    }
    /// Checks `expr`, returning its type, or `None` if it can't be determined
    /// because of an error that has already been reported, along with what it
    /// compiles to. `expected` is the type it's used as, if that's known,
    /// which integer literals take as their type. It doesn't have to match:
    /// whatever uses `expr` checks that.
    pub(super) fn expr_as(&mut self, expr: &Expr, expected: Option<&Type>) -> Typed {
        let typed = self.expr_kind_as(expr, expected);
        if typed.ty == Some(self.std_type("Never")) {
            self.evaluated_never = true;
        }
        typed
    }
    /// Checks `expr` like `expr_as`, by what kind of expression it is.
    pub(super) fn expr_kind_as(&mut self, expr: &Expr, expected: Option<&Type>) -> Typed {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Int(value) => self.int_literal(i128::from(*value), span, expected),
            // A negated literal is checked as one value, so the smallest
            // signed integers can be written.
            ExprKind::Unary(UnOp::Neg, operand) if let ExprKind::Int(value) = &operand.kind => {
                self.int_literal(-i128::from(*value), span, expected)
            }
            ExprKind::Bool(value) => Typed {
                ty: Some(self.std_type("Bool")),
                expr: ir::Expr::Bool(*value),
            },
            ExprKind::Var(name) => match self.lookup(name) {
                Ok(Binding::Value { ty, local, .. }) => Typed {
                    ty,
                    expr: ir::Expr::Local(local),
                },
                Ok(Binding::Global(index)) => Typed {
                    ty: self.global_type(index),
                    expr: ir::Expr::Global(index),
                },
                Ok(Binding::Function(index)) => {
                    let function = self.own_fn(index);
                    self.function_value(function, span)
                }
                Ok(Binding::Type(_) | Binding::Alias(_) | Binding::Generic(_)) => {
                    self.error(span, CheckError::TypeAsValue(name.to_string()));
                    Typed::UNKNOWN
                }
                Ok(Binding::Trait(_)) => {
                    self.error(span, CheckError::TraitAsValue(name.to_string()));
                    Typed::UNKNOWN
                }
                Err(error) => {
                    self.lookup_error(name, error);
                    Typed::UNKNOWN
                }
            },
            ExprKind::Unary(op, operand) => self.unary(*op, operand, expected),
            ExprKind::Binary(lhs, op, rhs) => self.binary(lhs, *op, rhs, expected),
            ExprKind::Call { callee, args } => self.call(callee, args, span),
            ExprKind::Construct(fields) => self.construct(span, fields),
            ExprKind::Paren(value) => match expected.and_then(|ty| self.elements(ty.id)) {
                // A tuple with one unlabelled element is built from a value in
                // brackets, as in `(1)`.
                Some([element]) if element.label.is_none() => {
                    let arg = Arg {
                        label: None,
                        value: (**value).clone(),
                        span,
                    };
                    self.tuple(&[arg], expected)
                }
                _ => self.expr_as(value, expected),
            },
            ExprKind::Tuple(values) => self.tuple(values, expected),
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => self.method_call(receiver, method, args, span, expected),
            ExprKind::Field { receiver, field } => self.field(receiver, field, span, expected),
            ExprKind::GenericType { name, .. } => {
                self.error(span, CheckError::TypeAsValue(name.to_string()));
                Typed::UNKNOWN
            }
            ExprKind::SelfValue => {
                let name = Name {
                    span,
                    ..Name::new("self")
                };
                match self.lookup(&name) {
                    Ok(Binding::Value { ty, local, .. }) => Typed {
                        ty,
                        expr: ir::Expr::Local(local),
                    },
                    Ok(_) => unreachable!("only methods declare `self`, as a value"),
                    Err(LookupError::Undeclared) => {
                        let in_static = self.cx.current_fn.as_ref().is_some_and(|f| f.is_static);
                        self.error(
                            span,
                            if in_static {
                                CheckError::SelfInStatic
                            } else {
                                CheckError::SelfOutsideImpl
                            },
                        );
                        Typed::UNKNOWN
                    }
                    Err(error) => {
                        self.lookup_error(&name, error);
                        Typed::UNKNOWN
                    }
                }
            }
        }
    }
    /// Checks an integer literal of `value`, which may be negated, used as
    /// `expected`. It must fit in its type.
    pub(super) fn int_literal(
        &mut self,
        value: i128,
        span: Span,
        expected: Option<&Type>,
    ) -> Typed {
        let ty = self.int_type_for(expected);
        let (bits, signed) = self.int_type(&ty).expect("literals have integer types");
        let (min, max) = int_range(bits, signed);
        if !(min..=max).contains(&value) {
            self.error(
                span,
                CheckError::IntOutOfRange {
                    value,
                    ty: ty.clone(),
                    min,
                    max,
                },
            );
            return Typed::error(ty);
        }

        // Stored as its bits, so a negative value is in two's complement.
        let bits = (value as u128 & (u128::MAX >> (128 - bits))) as u64;
        Typed {
            expr: ir::Expr::Int {
                value: bits,
                ty: ty.id,
            },
            ty: Some(ty),
        }
    }
    /// Checks `op operand`, which calls a function of a trait from the
    /// standard library, as `UnOp::trait_function` says, such as
    /// `operand.negate()` from `Negate` for `-operand`. The operand's type
    /// must implement the trait, and the result is the function's. The
    /// operand is used as `expected`, which is what the operator is used as.
    pub(super) fn unary(&mut self, op: UnOp, operand: &Expr, expected: Option<&Type>) -> Typed {
        let (trait_name, method) = op.trait_function();
        let span = operand.span;
        let Typed { ty, expr } = self.expr_as(operand, expected);
        let function = ty
            .as_ref()
            .and_then(|ty| self.operator_method(op.symbol(), trait_name, method, ty, span));
        // The result's type comes from the function, so it's unknown without
        // it.
        let returns = function
            .as_ref()
            .and_then(|function| self.signature_of(function, span).returns);
        let function = function.map(|function| self.callee(function, span));
        Typed {
            ty: returns.map(|returns| self.named_like(returns, ty.as_ref())),
            expr: call(function, vec![expr]),
        }
    }
    /// Finds `method` of `ty`, which the operator `op` calls, as an index
    /// into `functions`, as `trait_method` does, for the code at `span`.
    pub(super) fn operator_method(
        &mut self,
        op: &'static str,
        trait_name: &'static str,
        method: &str,
        ty: &Type,
        span: Span,
    ) -> Option<FnRef> {
        let not_implemented = CheckError::NotImplemented {
            op,
            trait_name,
            found: ty.clone(),
        };
        self.trait_method(trait_name, method, ty, span, not_implemented)
    }
    /// Finds `method` of `ty`, as an index into `functions`, which the
    /// compiler calls for some syntax, such as an operator. `ty` must
    /// implement the standard library's trait called `trait_name`, or
    /// `not_implemented` is reported at `span`.
    pub(super) fn trait_method(
        &mut self,
        trait_name: &str,
        method: &str,
        ty: &Type,
        span: Span,
        not_implemented: CheckError,
    ) -> Option<FnRef> {
        let trait_index = match self.files[self.core].scope.get(trait_name) {
            Some(Binding::Trait(index)) => *index,
            _ => panic!("the standard library should declare `{trait_name}`"),
        };
        let found = match self.member(ty, method) {
            Some(Member::Method(found)) if self.implements(ty, trait_index) => found,
            _ => {
                self.error(span, not_implemented);
                return None;
            }
        };
        Some(found)
    }
    /// Checks `lhs op rhs`, which calls a function of a trait from the
    /// standard library, as `BinOp::trait_function` says, such as
    /// `lhs.add(rhs)` from `Add` for `lhs + rhs`. The type of `lhs` must
    /// implement the trait, `rhs` must be its implementation's `Rhs`, and the
    /// result is the function's. Unless it's a comparison, the operands are
    /// used as `expected`, which is what the operator is used as.
    pub(super) fn binary(
        &mut self,
        lhs: &Expr,
        op: BinOp,
        rhs: &Expr,
        expected: Option<&Type>,
    ) -> Typed {
        let (trait_name, method) = op.trait_function();
        let (lhs_span, rhs_span) = (lhs.span, rhs.span);
        let expected = expected.filter(|_| !op.is_comparison());
        // An operand made only of integer literals has no type of its own, so
        // in `0 == byte`, the `0` takes its type from `byte`. What a shift is
        // shifted by doesn't have the type of what's shifted.
        let is_shift = matches!(op, BinOp::ShiftLeft | BinOp::ShiftRight);
        let (lhs, rhs_first) = if !is_shift && !has_own_type(lhs) && has_own_type(rhs) {
            let rhs = self.expr_as(rhs, expected);
            (self.expr_as(lhs, rhs.ty.as_ref().or(expected)), Some(rhs))
        } else {
            (self.expr_as(lhs, expected), None)
        };
        let op = op.symbol();
        let signature = lhs
            .ty
            .as_ref()
            .and_then(|ty| self.operator_method(op, trait_name, method, ty, lhs_span))
            .map(|function| {
                let signature = self.signature_of(&function, lhs_span);
                (self.callee(function, lhs_span), signature)
            });

        // The right operand is the method's argument, so it's used as its
        // parameter's type, like any other argument.
        let param = signature
            .as_ref()
            .and_then(|(_, signature)| signature.params.first()?.ty.clone());
        let rhs = match rhs_first {
            Some(rhs) => rhs,
            None => self.expr_as(rhs, param.as_ref().or(expected)),
        };
        let mut function = signature.as_ref().map(|&(function, _)| function);
        if let Some((param, found)) = param.zip(rhs.ty)
            && param != found
        {
            let expected = self.named_like(param, lhs.ty.as_ref());
            self.error(
                rhs_span,
                CheckError::InvalidOperand {
                    op,
                    expected,
                    found,
                },
            );
            function = None;
        }

        let returns = signature.and_then(|(_, signature)| signature.returns);
        Typed {
            ty: returns.map(|returns| self.named_like(returns, lhs.ty.as_ref())),
            expr: call(function, vec![lhs.expr, rhs.expr]),
        }
    }
    /// `ty`, named as `operand` is if it's the same type, such as `Int`
    /// rather than the `IntS64` that an operator's `impl` is for.
    pub(super) fn named_like(&self, ty: Type, operand: Option<&Type>) -> Type {
        operand
            .filter(|operand| **operand == ty)
            .cloned()
            .unwrap_or(ty)
    }
    /// Checks a tuple built from `values`, labelled like the elements of its
    /// type. If it has the labels of `expected`, its values take their types
    /// from its elements, and it's that type if they're the same. Otherwise,
    /// its elements are named after their labels, so `(x: 1)` is `(x: Int)`,
    /// and the elements without labels can't be read.
    pub(super) fn tuple(&mut self, values: &[Arg], expected: Option<&Type>) -> Typed {
        let expected_elements = expected
            .and_then(|ty| self.elements(ty.id))
            .filter(|elements| {
                elements.len() == values.len()
                    && elements
                        .iter()
                        .zip(values)
                        .all(|(element, value)| element.label == value.label)
            })
            .map(<[_]>::to_vec);
        let typed: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let element = expected_elements
                    .as_ref()
                    .map(|elements| &elements[index].ty);
                self.expr_as(&value.value, element)
            })
            .collect();
        let (found, lowered) = split(typed);

        // An element whose type is unknown has already been reported.
        let is_expected = expected_elements.is_some_and(|elements| {
            elements
                .iter()
                .zip(&found)
                .all(|(element, found)| found.as_ref().is_none_or(|found| *found == element.ty))
        });
        let ty = if is_expected {
            expected.cloned()
        } else {
            let elements = values.iter().zip(found).map(|(value, ty)| {
                Some(TupleElementType {
                    label: value.label.clone(),
                    name: value.label.clone(),
                    ty: ty?,
                })
            });
            let elements = elements.collect::<Option<_>>();
            elements.map(|elements| self.tuple_type(elements))
        };
        let expr = match &ty {
            Some(ty) => ir::Expr::Construct {
                group: ty.id,
                fields: lowered,
            },
            None => ir::Expr::Error,
        };
        Typed { ty, expr }
    }
    /// Checks the receiver of a field access or method call, which is followed
    /// by `.member`. A type's name is the type itself, for its static
    /// functions. A file's name gets its own error, since `::` was probably
    /// meant.
    ///
    /// A generic type's name can leave out its arguments when the value is
    /// used as `expected`, if that's made from it, as in `Optional.none`.
    pub(super) fn receiver(
        &mut self,
        receiver: &Expr,
        member: &str,
        expected: Option<&Type>,
    ) -> Option<Receiver> {
        if let ExprKind::GenericType { name, args } = &receiver.kind {
            let ty = self.generic_type(name, args)?;
            return Some(Receiver::Type(ty));
        }
        if let ExprKind::Var(name) = &receiver.kind {
            match self.lookup(name) {
                Ok(Binding::Type(ty)) => return Some(Receiver::Type(ty)),
                Ok(Binding::Generic(_)) => {
                    let type_name = TypeName::Named(name.clone());
                    let ty = self.type_in_context(&type_name, &[expected])?;
                    return Some(Receiver::Type(ty));
                }
                Err(LookupError::Undeclared)
                    if name.file.is_none()
                        && self.files[self.cx.file]
                            .imports
                            .iter()
                            .any(|import| import.name == name.name) =>
                {
                    self.error(
                        receiver.span,
                        CheckError::FileNotValue {
                            file: name.name.clone(),
                            member: member.to_string(),
                        },
                    );
                    return None;
                }
                _ => {}
            }
        }
        let Typed { ty, expr } = self.expr(receiver);
        Some(Receiver::Value(ty?, expr))
    }
    /// Finds the field or method of `ty` called `name`. A tuple's elements
    /// are its fields, found by their names.
    pub(super) fn member(&self, ty: &Type, name: &str) -> Option<Member> {
        if let Some(elements) = self.elements(ty.id)
            && let Some(index) = elements
                .iter()
                .position(|element| element.name.as_deref() == Some(name))
        {
            return Some(Member::Field(index, Some(elements[index].ty.clone())));
        }
        let fields = self.fields(ty.id);
        if let Some(index) = fields.iter().position(|field| field.name == name) {
            return Some(Member::Field(index, fields[index].ty.clone()));
        }
        let (owner, args) = self.owner(ty);
        let index = *self.types[owner].methods.get(name)?;
        Some(Member::Method(FnRef { index, args }))
    }
    /// The `id` of the type whose `impl`s declare the functions of `ty`,
    /// along with what its generic type's parameters stand for in `ty`. A
    /// type made from a generic type shares the functions of the one made
    /// with its own parameters.
    pub(super) fn owner(&self, ty: &Type) -> (usize, Vec<Type>) {
        if let Some((template, args)) = &self.types[ty.id].instance
            && let Some(generic) = &self.templates[*template].generic
        {
            return (generic.id, args.clone());
        }
        (ty.id, vec![])
    }
    /// Whether `ty` implements the trait at `trait_index` in `traits`.
    pub(super) fn implements(&self, ty: &Type, trait_index: usize) -> bool {
        let (owner, _) = self.owner(ty);
        self.implementations.contains(&(owner, trait_index))
    }
    /// Finds the static function of `ty` called `name`.
    pub(super) fn static_function(&self, ty: &Type, name: &str) -> Option<FnRef> {
        let (owner, args) = self.owner(ty);
        let index = *self.types[owner].statics.get(name)?;
        Some(FnRef { index, args })
    }
    /// Finds the option of `ty` called `name`, if `ty` is a choice, returning
    /// its index in the order they're declared, and the type of the value it
    /// holds, which is `None` if it's unknown.
    pub(super) fn option(&self, ty: &Type, name: &str) -> Option<(usize, Option<Type>)> {
        let options = self.options(ty.id)?;
        let index = options.iter().position(|option| option.name == name)?;
        Some((index, options[index].ty.clone()))
    }
    /// The `Base` and `Value` of `ty`, if it's made from `ChoiceOption`.
    pub(super) fn option_args(&self, ty: &Type) -> Option<(Type, Type)> {
        let (template, args) = self.types[ty.id].instance.as_ref()?;
        match &args[..] {
            [base, value] if *template == self.std_template("ChoiceOption") => {
                Some((base.clone(), value.clone()))
            }
            _ => None,
        }
    }
    /// The error for `name` not being a field or method of `ty`, which points
    /// out an option or static function with that name.
    pub(super) fn no_member(&self, ty: Type, name: &str) -> CheckError {
        let member = name.to_string();
        let kind = if self.option(&ty, name).is_some() {
            MemberKind::Option
        } else if self.static_function(&ty, name).is_some() {
            MemberKind::StaticFunction
        } else {
            return CheckError::NoMember { ty, member };
        };
        CheckError::NeedsType { ty, member, kind }
    }
    /// The error for `name` not being an option or static function of `ty`,
    /// which points out a field or method with that name.
    pub(super) fn no_static_function(&self, ty: Type, name: &str) -> CheckError {
        let kind = match self.member(&ty, name) {
            Some(Member::Field(..)) => MemberKind::Field,
            Some(Member::Method(_)) => MemberKind::Method,
            None if self.options(ty.id).is_some() => {
                return CheckError::NoOption {
                    ty,
                    option: name.to_string(),
                };
            }
            None => {
                return CheckError::NoStaticFunction {
                    ty,
                    function: name.to_string(),
                };
            }
        };
        CheckError::NeedsValue {
            ty,
            member: name.to_string(),
            kind,
        }
    }
    /// Checks a field access, returning the field's type, or `None` if it
    /// can't be determined, along with what it compiles to.
    pub(super) fn field(
        &mut self,
        receiver: &Expr,
        field: &str,
        span: Span,
        expected: Option<&Type>,
    ) -> Typed {
        // A generic type's name can leave out its arguments when one of its
        // options is used as `expected`, as in `Optional.none`.
        let base = expected.and_then(|expected| self.option_args(expected));
        let expected = base.as_ref().map(|(base, _)| base).or(expected);
        let Some(receiver) = self.receiver(receiver, field, expected) else {
            return Typed::UNKNOWN;
        };
        let error = match receiver {
            Receiver::Value(ty, receiver) => match self.member(&ty, field) {
                Some(Member::Field(index, field_ty)) => {
                    return Typed {
                        ty: field_ty,
                        expr: ir::Expr::Field {
                            receiver: Box::new(receiver),
                            index,
                        },
                    };
                }
                Some(Member::Method(_)) => CheckError::MemberNotValue {
                    ty,
                    member: field.to_string(),
                    kind: MemberKind::Method,
                },
                None => self.no_member(ty, field),
            },
            // Naming an option doesn't create the choice, but gives its
            // `ChoiceOption`.
            Receiver::Type(ty) if let Some((option, value_ty)) = self.option(&ty, field) => {
                let template = self.std_template("ChoiceOption");
                return Typed {
                    ty: value_ty.and_then(|value| self.instance(template, vec![ty, value], span)),
                    expr: ir::Expr::Tag(option),
                };
            }
            Receiver::Type(ty) => match self.static_function(&ty, field) {
                Some(found) => {
                    return self.function_value(found, span);
                }
                None => self.no_static_function(ty, field),
            },
        };
        self.error(span, error);
        Typed::UNKNOWN
    }
    /// Checks `Self { ... }`, which constructs the type of the `impl` it's
    /// in, returning that type, or `None` if it can't be determined, along
    /// with what it compiles to.
    pub(super) fn construct(&mut self, span: Span, values: &[FieldValue]) -> Typed {
        let ty = match &self.cx.impl_type {
            Some(Some(ty)) => Some(ty.clone()),
            // The `impl`'s type is unknown, which has been reported.
            Some(None) => None,
            None => {
                self.error(span, CheckError::SelfTypeOutsideImpl);
                None
            }
        };
        // The values are still checked if the type is unknown, so their
        // errors are found. Otherwise, they take their types from its fields.
        let fields = match &ty {
            Some(ty) => self.fields(ty.id).to_vec(),
            None => vec![],
        };
        let typed = values.iter().enumerate().map(|(index, value)| {
            let expected = fields
                .get(index)
                .filter(|field| field.name == value.name)
                .and_then(|field| field.ty.as_ref());
            self.expr_as(&value.value, expected)
        });
        let (found, lowered) = split(typed.collect());
        let Some(ty) = ty else {
            return Typed::UNKNOWN;
        };
        if self.options(ty.id).is_some() {
            self.error(span, CheckError::ConstructedChoice(ty.clone()));
            return Typed::error(ty);
        }
        if self.primitive(ty.id).is_some() {
            self.error(span, CheckError::ConstructedPrimitive(ty.clone()));
            return Typed::error(ty);
        }
        // Only the type's own file can construct it. Elsewhere, values come
        // from its functions, such as `Void.new()`.
        let declared_in = self.types[ty.id].file;
        if declared_in != self.cx.file {
            self.error(
                span,
                CheckError::ConstructedOutsideFile {
                    ty: ty.clone(),
                    file: self.files[declared_in].display.clone(),
                },
            );
            return Typed::error(ty);
        }
        let fields = self.fields(ty.id).to_vec();
        let name = ty.to_string();

        // Every field must be given, in the order the group declares them.
        if values.len() != fields.len() {
            self.error(
                span,
                CheckError::WrongFieldCount {
                    ty: name,
                    expected: fields.len(),
                    found: values.len(),
                },
            );
            return Typed::error(ty);
        }

        for ((value, found), field) in values.iter().zip(found).zip(&fields) {
            if value.name != field.name {
                self.error(
                    value.span,
                    CheckError::WrongField {
                        ty: name.clone(),
                        expected: field.name.to_string(),
                        found: value.name.clone(),
                    },
                );
            } else if let (Some(expected), Some(found)) = (&field.ty, found)
                && *expected != found
            {
                self.error(
                    value.value.span,
                    CheckError::FieldTypeMismatch {
                        ty: name.clone(),
                        field: field.name.to_string(),
                        expected: expected.clone(),
                        found,
                    },
                );
            }
        }

        Typed {
            expr: ir::Expr::Construct {
                group: ty.id,
                fields: lowered,
            },
            ty: Some(ty),
        }
    }
}
