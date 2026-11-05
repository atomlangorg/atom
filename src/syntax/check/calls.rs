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

//! Checking calls of functions, methods, options and values.

use super::*;

impl<'a> Checker<'a> {
    /// Checks calling `value`, of type `ty`, called `name`, with `args`, at
    /// `span`, which calls its `call` from the standard library's `Call`
    /// trait, as in `adder(a: 3)` for `adder.call(input: (a: 3))`. The
    /// arguments are always built into a tuple, which must be the `Input`.
    pub(super) fn call_value(
        &mut self,
        ty: Option<Type>,
        value: ir::Expr,
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        if let Some(option_ty) = ty.as_ref()
            && let Some((base, value_ty)) = self.option_args(option_ty)
        {
            return self.call_option(option_ty, value, (base, value_ty), name, args, span);
        }
        let function = ty.as_ref().and_then(|ty| {
            let not_callable = CheckError::NotCallable {
                name: name.to_string(),
                ty: ty.clone(),
            };
            self.trait_method("Call", "call", ty, span, not_callable)
        });
        let signature = function
            .as_ref()
            .map(|function| self.signature_of(function, span));
        // The input is the only parameter of `call`, which the arguments are
        // checked as, like any other function's.
        let (param, returns) = match signature {
            Some(signature) => (
                signature.params.first().and_then(|param| param.ty.clone()),
                signature.returns,
            ),
            None => (None, None),
        };
        let Typed {
            ty: found,
            expr: input,
        } = self.tuple(args, param.as_ref());
        let function = function.map(|function| self.callee(function, span));
        if let (Some(expected), Some(found)) = (&param, &found)
            && expected != found
        {
            self.error(
                span,
                CheckError::WrongCallInput {
                    name: name.to_string(),
                    expected: expected.clone(),
                    found: found.clone(),
                },
            );
        }
        Typed {
            ty: returns,
            expr: call(function, vec![value, input]),
        }
    }
    /// Checks calling `value`, whose type `ty` is a `ChoiceOption` with
    /// `base` and `value_ty`, called `name`, with `args`, at `span`.
    pub(super) fn call_option(
        &mut self,
        ty: &Type,
        value: ir::Expr,
        (base, value_ty): (Type, Type),
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let not_callable = CheckError::NotCallable {
            name: name.to_string(),
            ty: ty.clone(),
        };
        let function = self
            .trait_method("Call", "call", ty, span, not_callable)
            .map(|function| self.callee(function, span));
        let Typed { ty: returns, expr } =
            self.option_value(name.to_string(), base, Some(value_ty), args, span);
        Typed {
            ty: returns,
            expr: call(function, vec![value, expr]),
        }
    }
    /// Checks `args` as the value for an option of the choice `base`, called
    /// `name`, which holds a `value_ty`. Creating an option is like calling a
    /// function with one unlabelled parameter, so a tuple is always one
    /// argument, except that a `Void` can be left out. Returns the choice's
    /// type, along with what the value compiles to.
    pub(super) fn option_value(
        &mut self,
        name: String,
        base: Type,
        value_ty: Option<Type>,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        if args.is_empty() && value_ty == Some(self.std_type("Void")) {
            return Typed {
                ty: Some(base),
                expr: self.void(),
            };
        }
        let (found, lowered) = self.args(args, std::slice::from_ref(&value_ty));
        self.checked_option_value(name, base, value_ty, args, found, lowered, span)
    }
    /// Checks `args`, which have the types `found` and compile to `lowered`,
    /// as the value for an option, as `option_value` does.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn checked_option_value(
        &mut self,
        name: String,
        base: Type,
        value_ty: Option<Type>,
        args: &[Arg],
        found: Vec<Option<Type>>,
        lowered: Vec<ir::Expr>,
        span: Span,
    ) -> Typed {
        let signature = Signature {
            name: name.clone(),
            params: vec![ParamSignature {
                label: None,
                name: "value",
                span,
                ty: value_ty,
            }],
            returns: Some(base),
            template: None,
        };
        Typed {
            ty: self.check_args(name, args, found, signature, span),
            expr: <[_; 1]>::try_from(lowered).map_or(ir::Expr::Error, |[value]| value),
        }
    }
    /// The types of the parameters of the static function or option called
    /// `method` of the generic type at `template` in `templates`, if it has
    /// one, which can use its parameters' stand-ins.
    pub(super) fn declared_params(
        &self,
        template: usize,
        method: &str,
    ) -> Option<Vec<Option<Type>>> {
        let generic = self.templates[template].generic.as_ref()?;
        if let Some((_, value_ty)) = self.option(generic, method) {
            return Some(vec![value_ty]);
        }
        let function = self.static_function(generic, method)?;
        let params = &self.functions[function.index].params;
        Some(params.iter().map(|param| param.ty.clone()).collect())
    }
    /// Checks a call to the static function or option called `method` of the
    /// generic type called `name`, at `template` in `templates`, working out
    /// its arguments from the types of `args`, which are matched against
    /// `declared`. Each of `args` is checked once, in order, so once every
    /// argument of the type is known, the rest take their types from its
    /// parameters, as in any other call.
    pub(super) fn inferred_call(
        &mut self,
        name: &Name,
        template: usize,
        method: &str,
        declared: &[Option<Type>],
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let stand_ins = self.templates[template].stand_ins.clone();
        let mut inferred = vec![None; stand_ins.len()];
        // The type, once every one of its arguments is known.
        let mut made = None;
        let mut found = vec![];
        let mut lowered = vec![];
        for (index, arg) in args.iter().enumerate() {
            if made.is_none()
                && let Some(args) = inferred.iter().cloned().collect()
            {
                made = Some(self.named_instance(template, args, name));
            }
            let expected = match made.clone().flatten() {
                Some(ty) => self.param_type(&ty, method, index, span),
                None => None,
            };
            let Typed { ty, expr } = self.expr_as(&arg.value, expected.as_ref());
            if let (Some(Some(param)), Some(ty)) = (declared.get(index), &ty) {
                self.infer(param, ty, &stand_ins, &mut inferred);
            }
            found.push(ty);
            lowered.push(expr);
        }

        let made = match (made, inferred.into_iter().collect()) {
            (Some(made), _) => made,
            (None, Some(args)) => self.named_instance(template, args, name),
            (None, None) => {
                // An unknown argument has been reported, and may be why.
                if found.iter().all(Option::is_some) {
                    self.error(name.span, CheckError::CannotInferGeneric(name.to_string()));
                }
                return Typed::UNKNOWN;
            }
        };
        let Some(ty) = made else {
            return Typed::UNKNOWN;
        };
        let full_name = format!("{ty}.{method}");
        if let Some((option, value_ty)) = self.option(&ty, method) {
            let choice = ty.id;
            let value =
                self.checked_option_value(full_name, ty, value_ty, args, found, lowered, span);
            return chosen(choice, option, value);
        }
        let Some(function) = self.static_function(&ty, method) else {
            return Typed::UNKNOWN;
        };
        let signature = self.signature_of(&function, span);
        let function = self.callee(function, span);
        Typed {
            ty: self.check_args(full_name, args, found, signature, span),
            expr: ir::Expr::Call {
                function,
                args: lowered,
            },
        }
    }
    /// The type of the parameter at `index` of the static function or option
    /// called `method` of `ty`, for the code at `span`.
    pub(super) fn param_type(
        &mut self,
        ty: &Type,
        method: &str,
        index: usize,
        span: Span,
    ) -> Option<Type> {
        if let Some((_, value_ty)) = self.option(ty, method) {
            return value_ty.filter(|_| index == 0);
        }
        let function = self.static_function(ty, method)?;
        let signature = self.signature_of(&function, span);
        signature.params.get(index)?.ty.clone()
    }
    /// Checks a call to `function`, called `name`, with `args`, which are
    /// checked as its parameters.
    pub(super) fn call_function(
        &mut self,
        function: FnRef,
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let signature = self.signature_of(&function, span);
        let params: Vec<_> = signature
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect();
        let (found, lowered) = self.args(args, &params);
        let function = self.callee(function, span);
        Typed {
            ty: self.check_args(name.to_string(), args, found, signature, span),
            expr: ir::Expr::Call {
                function,
                args: lowered,
            },
        }
    }
    /// Checks a call to a method on a value, a static function on a type, or
    /// the creation of an option of a choice, returning its return type, or
    /// `None` if it can't be determined, along with what it compiles to.
    pub(super) fn method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Arg],
        span: Span,
        expected: Option<&Type>,
    ) -> Typed {
        // A generic type's name can leave out its arguments if they're not
        // known from `expected`, when they can be worked out from the call's.
        if let ExprKind::Var(name) = &receiver.kind
            && let Ok(Binding::Generic(template)) = self.lookup(name)
            && !expected.is_some_and(|expected| self.is_made_from(expected, template))
            && let Some(declared) = self.declared_params(template, method)
        {
            return self.inferred_call(name, template, method, &declared, args, span);
        }
        let receiver = self.receiver(receiver, method, expected);
        // A field followed by arguments is called, like any other value.
        let receiver = match receiver {
            Some(Receiver::Value(ty, lowered)) => match self.member(&ty, method) {
                Some(Member::Field(index, field_ty)) => {
                    let value = ir::Expr::Field {
                        receiver: Box::new(lowered),
                        index,
                    };
                    return self.call_value(field_ty, value, method, args, span);
                }
                _ => Some(Receiver::Value(ty, lowered)),
            },
            // A static function is called like any other function.
            Some(Receiver::Type(ty)) if let Some(found) = self.static_function(&ty, method) => {
                return self.call_function(found, &format!("{ty}.{method}"), args, span);
            }
            // An option is called directly, since it's known which one it is.
            Some(Receiver::Type(ty)) if let Some((option, value_ty)) = self.option(&ty, method) => {
                let choice = ty.id;
                let name = format!("{ty}.{method}");
                let value = self.option_value(name, ty, value_ty, args, span);
                return chosen(choice, option, value);
            }
            receiver => receiver,
        };
        let (ty, found_method, lowered_receiver) = match receiver {
            Some(Receiver::Value(ty, lowered)) => match self.member(&ty, method) {
                Some(Member::Method(found)) => (ty, found, lowered),
                _ => {
                    let error = self.no_member(ty, method);
                    self.error(span, error);
                    // The arguments are still checked, so their errors are
                    // found.
                    self.args(args, &[]);
                    return Typed::UNKNOWN;
                }
            },
            Some(Receiver::Type(ty)) => {
                let error = self.no_static_function(ty, method);
                self.error(span, error);
                self.args(args, &[]);
                return Typed::UNKNOWN;
            }
            None => {
                self.args(args, &[]);
                return Typed::UNKNOWN;
            }
        };
        let signature = self.signature_of(&found_method, span);
        let params: Vec<_> = signature
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect();
        let (found, lowered_args) = self.args(args, &params);
        let returns = self.check_args(format!("{ty}.{method}"), args, found, signature, span);
        let found_method = self.callee(found_method, span);

        // A method's receiver is its first argument.
        let args = iter::once(lowered_receiver).chain(lowered_args).collect();
        Typed {
            ty: returns,
            expr: ir::Expr::Call {
                function: found_method,
                args,
            },
        }
    }
    /// Checks a call, returning what's called's return type, or `None` if it
    /// can't be determined, along with what it compiles to. A function is
    /// called directly, and other values through the standard library's
    /// `Call` trait.
    pub(super) fn call(&mut self, callee: &Name, args: &[Arg], span: Span) -> Typed {
        let name = callee.to_string();
        match self.lookup(callee) {
            Ok(Binding::Function(index)) => {
                let function = self.own_fn(index);
                self.call_function(function, &name, args, span)
            }
            Ok(Binding::Value { .. } | Binding::Global(_)) => {
                let Typed { ty, expr } = self.expr(&Expr {
                    kind: ExprKind::Var(callee.clone()),
                    span: callee.span,
                });
                self.call_value(ty, expr, &name, args, span)
            }
            // The arguments are still checked, so their errors are found.
            Ok(Binding::Type(_) | Binding::Alias(_) | Binding::Trait(_) | Binding::Generic(_)) => {
                self.args(args, &[]);
                self.error(callee.span, CheckError::NotAFunction(name));
                Typed::UNKNOWN
            }
            Err(error) => {
                self.args(args, &[]);
                self.lookup_error(callee, error);
                Typed::UNKNOWN
            }
        }
    }
    /// Checks a call's arguments, where each is used as the parameter at the
    /// same position in `params`, returning their types, and what they
    /// compile to if none of them have errors.
    pub(super) fn args(
        &mut self,
        args: &[Arg],
        params: &[Option<Type>],
    ) -> (Vec<Option<Type>>, Vec<ir::Expr>) {
        let typed = args.iter().enumerate().map(|(index, arg)| {
            let expected = params.get(index).and_then(Option::as_ref);
            self.expr_as(&arg.value, expected)
        });
        split(typed.collect())
    }
    /// Checks a call's arguments, whose types are `found`, against the
    /// signature of `function`, returning its return type.
    pub(super) fn check_args(
        &mut self,
        function: String,
        args: &[Arg],
        found: Vec<Option<Type>>,
        signature: Signature,
        span: Span,
    ) -> Option<Type> {
        if args.len() != signature.params.len() {
            self.error(
                span,
                CheckError::WrongArgumentCount {
                    function,
                    expected: signature.params.len(),
                    found: args.len(),
                },
            );
            return signature.returns;
        }

        for ((arg, found), param) in args.iter().zip(found).zip(&signature.params) {
            if arg.label.as_deref() != param.label {
                self.error(
                    arg.span,
                    CheckError::WrongLabel {
                        function: function.clone(),
                        expected: param.label.map(str::to_string),
                        found: arg.label.clone(),
                    },
                );
            } else if let (Some(expected), Some(found)) = (&param.ty, found)
                && *expected != found
            {
                self.error(
                    arg.value.span,
                    CheckError::ArgumentTypeMismatch {
                        function: function.clone(),
                        param: param.label.unwrap_or(param.name).to_string(),
                        expected: expected.clone(),
                        found,
                    },
                );
            }
        }

        signature.returns
    }
}
