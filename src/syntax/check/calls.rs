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
    /// arguments always build its `Input`, which must be a tuple.
    pub(super) fn call_value(
        &mut self,
        ty: Option<Type>,
        value: ir::Expr,
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let call = ty.as_ref().and_then(|ty| {
            let not_callable = CheckError::NotCallable {
                name: name.to_string(),
                ty: ty.clone(),
            };
            self.trait_method("Call", "call", ty, span, not_callable)
        });
        // The input is the only parameter of `call`.
        let (input, returns) = match &call {
            Some(call) => {
                let signature = self.signature_of(call, span);
                let input = signature.params.first().and_then(|param| param.ty.clone());
                (input, signature.returns)
            }
            None => (None, None),
        };
        match input {
            Some(input) if self.elements(input.id).is_some() => {
                let callee = Callee::Value {
                    value,
                    call,
                    input,
                    returns,
                };
                self.call_callee(callee, name.to_string(), args, span)
            }
            // Its input is unknown, which has already been reported, or isn't
            // a tuple, so the arguments can't build it.
            input => {
                let found = self.tuple(args, None).ty;
                if let (Some(expected), Some(found)) = (input, found) {
                    let name = name.to_string();
                    let error = CheckError::WrongCallInput {
                        name,
                        expected,
                        found,
                    };
                    self.error(span, error);
                }
                Typed {
                    ty: returns,
                    expr: ir::Expr::Error,
                }
            }
        }
    }
    /// The signature of `callee`, for the code at `span`. Something whose
    /// arguments build a tuple, such as a value with `Call`, is like a
    /// function whose parameters are the tuple's elements.
    pub(super) fn callee_signature(&mut self, callee: &Callee, span: Span) -> Signature<'a> {
        match callee {
            Callee::Function(function) | Callee::Method { function, .. } => {
                self.signature_of(function, span)
            }
            Callee::Value { input, returns, .. } => Signature {
                name: input.name.clone(),
                params: self.tuple_params(input),
                returns: returns.clone(),
                template: None,
            },
        }
    }
    /// The parameters whose arguments build the tuple `ty`, which are read
    /// by its labels.
    pub(super) fn tuple_params(&self, ty: &Type) -> Vec<ParamSignature<'a>> {
        let elements = self.elements(ty.id).unwrap_or_default();
        elements
            .iter()
            .map(|element| ParamSignature {
                label: element.label.clone(),
                name: None,
                span: Span::default(),
                ty: Some(element.ty.clone()),
            })
            .collect()
    }
    /// The tuple that the arguments for `params` build, or `None` if any of
    /// their types are unknown.
    pub(super) fn params_tuple(&mut self, params: &[ParamSignature]) -> Option<Type> {
        let elements = params.iter().map(|param| {
            Some(TupleElementType {
                label: param.label.clone(),
                ty: param.ty.clone()?,
            })
        });
        let elements = elements.collect::<Option<_>>()?;
        Some(self.tuple_type(elements, None))
    }
    /// Checks a call to `callee`, called `name`, with `args`, which are
    /// checked as its parameters.
    pub(super) fn call_callee(
        &mut self,
        callee: Callee,
        name: String,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let signature = self.callee_signature(&callee, span);
        let params: Vec<_> = signature
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect();
        let checked = self.args(args, &params);
        self.checked_call(callee, name, signature, args, checked, span)
    }
    /// Checks a call to `callee`, as `call_callee` does, against its
    /// `signature`, where `args` have already been checked, with the types
    /// and what they compile to in `checked`.
    pub(super) fn checked_call(
        &mut self,
        callee: Callee,
        name: String,
        signature: Signature,
        args: &[Arg],
        (found, lowered): (Vec<Option<Type>>, Vec<ir::Expr>),
        span: Span,
    ) -> Typed {
        let arity = signature.params.len();
        let returns = self.check_args(name, args, found, signature, span);
        // A value's arguments build a tuple.
        let build = |tuple: Option<&Type>, fields: Vec<ir::Expr>| match tuple {
            Some(tuple) if fields.len() == arity => ir::Expr::Construct {
                group: tuple.id,
                fields,
            },
            _ => ir::Expr::Error,
        };
        let expr = match callee {
            Callee::Function(function) => ir::Expr::Call {
                function: self.callee(function, span),
                args: lowered,
            },
            Callee::Method { function, receiver } => ir::Expr::Call {
                function: self.callee(function, span),
                args: iter::once(receiver).chain(lowered).collect(),
            },
            Callee::Value {
                value,
                call: function,
                input,
                ..
            } => {
                let input = build(Some(&input), lowered);
                let function = function.map(|function| self.callee(function, span));
                call(function, vec![value, input])
            }
        };
        Typed { ty: returns, expr }
    }
    /// The generic type that `receiver` names without its arguments, as an
    /// index into `templates`, along with its name and what's called on it,
    /// if `method` is called on it or on one of its static members.
    pub(super) fn generic_call<'e>(
        &mut self,
        receiver: &'e Expr,
        method: &'e str,
    ) -> Option<(&'e Name, usize, GenericCall<'e>)> {
        let (owner, call) = match &receiver.kind {
            ExprKind::Field {
                receiver: owner,
                field,
            } => (
                &**owner,
                GenericCall::Member {
                    member: field,
                    method,
                },
            ),
            _ => (receiver, GenericCall::Static(method)),
        };
        let ExprKind::Var(name) = &owner.kind else {
            return None;
        };
        let Ok(Binding::Generic(template)) = self.lookup(name) else {
            return None;
        };
        Some((name, template, call))
    }
    /// The signature of what `call` calls on `ty`, if it has it, without
    /// compiling anything, for the code at `span`.
    pub(super) fn generic_signature(
        &mut self,
        ty: &Type,
        call: GenericCall,
        span: Span,
    ) -> Option<Signature<'a>> {
        let function = match call {
            GenericCall::Static(name) => self.static_function(ty, name)?,
            GenericCall::Member { member, method } => {
                let member = self.static_member(ty, member)?;
                let value = self.static_member_type(ty, &member, span)?;
                match self.member(&value, method)? {
                    Member::Method(function) => function,
                    Member::Field(..) => return None,
                }
            }
        };
        Some(self.signature_of(&function, span))
    }
    /// What `call` calls on `ty`, if it has it, along with its name, for the
    /// code at `span`.
    pub(super) fn generic_callee(
        &mut self,
        ty: &Type,
        call: GenericCall,
        span: Span,
    ) -> Option<(Callee, String)> {
        match call {
            GenericCall::Static(name) => {
                let function = self.static_function(ty, name)?;
                Some((Callee::Function(function), format!("{ty}.{name}")))
            }
            GenericCall::Member { member, method } => {
                let member = self.static_member(ty, member)?;
                let Typed { ty: value, expr } = self.static_member_value(ty, member, span);
                let value = value?;
                let Some(Member::Method(function)) = self.member(&value, method) else {
                    return None;
                };
                let callee = Callee::Method {
                    function,
                    receiver: expr,
                };
                Some((callee, format!("{value}.{method}")))
            }
        }
    }
    /// Checks a call to what `call` calls on the generic type called `name`,
    /// at `template` in `templates`, working out its arguments from what the
    /// call returns being `expected`, and from the types of `args`. Each of
    /// `args` is checked once, in order, so once every argument of the type
    /// is known, the rest take their types from its parameters, as in any
    /// other call. Returns `None` without checking anything if the generic
    /// type has nothing that `call` calls.
    pub(super) fn inferred_call(
        &mut self,
        name: &Name,
        template: usize,
        call: GenericCall,
        args: &[Arg],
        span: Span,
        expected: Option<&Type>,
    ) -> Option<Typed> {
        let generic = self.templates[template].generic.clone()?;
        let declared = self.generic_signature(&generic, call, span)?;
        let stand_ins = self.templates[template].stand_ins.clone();
        let mut inferred = vec![None; stand_ins.len()];
        if let (Some(returns), Some(expected)) = (&declared.returns, expected) {
            self.infer(returns, expected, &stand_ins, &mut inferred);
        }
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
                Some(ty) => self.param_type(&ty, call, index, span),
                None => None,
            };
            let Typed { ty, expr } = self.expr_as(&arg.value, expected.as_ref());
            let param = declared
                .params
                .get(index)
                .and_then(|param| param.ty.as_ref());
            if let (Some(param), Some(ty)) = (param, &ty) {
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
                return Some(Typed::UNKNOWN);
            }
        };
        let Some((callee, name)) = made.and_then(|ty| self.generic_callee(&ty, call, span)) else {
            return Some(Typed::UNKNOWN);
        };
        let signature = self.callee_signature(&callee, span);
        Some(self.checked_call(callee, name, signature, args, (found, lowered), span))
    }
    /// The type of the parameter at `index` of what `call` calls on `ty`,
    /// for the code at `span`.
    pub(super) fn param_type(
        &mut self,
        ty: &Type,
        call: GenericCall,
        index: usize,
        span: Span,
    ) -> Option<Type> {
        let signature = self.generic_signature(ty, call, span)?;
        signature.params.into_iter().nth(index)?.ty
    }
    /// Checks a call to a method on a value or a static function on a type,
    /// returning its return type, or `None` if it can't be determined, along
    /// with what it compiles to.
    pub(super) fn method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Arg],
        span: Span,
        expected: Option<&Type>,
    ) -> Typed {
        // A generic type's name can leave out its arguments when they can be
        // worked out from the call.
        if let Some((name, template, call)) = self.generic_call(receiver, method)
            && let Some(typed) = self.inferred_call(name, template, call, args, span, expected)
        {
            return typed;
        }
        let (callee, name) = match self.receiver(receiver, method, expected) {
            Some(Receiver::Value(ty, lowered)) => match self.member(&ty, method) {
                // A field followed by arguments is called, like any other
                // value.
                Some(Member::Field(index)) => {
                    let field_ty = self.field_type(&ty, index, span);
                    let value = ir::Expr::Field {
                        receiver: Box::new(lowered),
                        index,
                    };
                    return self.call_value(field_ty, value, method, args, span);
                }
                Some(Member::Method(function)) => {
                    let callee = Callee::Method {
                        function,
                        receiver: lowered,
                    };
                    (callee, format!("{ty}.{method}"))
                }
                None => {
                    let error = self.no_member(ty, method);
                    self.error(span, error);
                    // The arguments are still checked, so their errors are
                    // found.
                    self.args(args, &[]);
                    return Typed::UNKNOWN;
                }
            },
            Some(Receiver::Type(ty)) => match self.static_member(&ty, method) {
                // A static function is called like any other function, and an
                // option is a value, so it's called like any other value.
                Some(StaticMember::Function(function)) => {
                    (Callee::Function(function), format!("{ty}.{method}"))
                }
                Some(member @ StaticMember::Option(_)) => {
                    let name = format!("{ty}.{method}");
                    let Typed { ty, expr } = self.static_member_value(&ty, member, span);
                    return self.call_value(ty, expr, &name, args, span);
                }
                None => {
                    let error = self.no_static_function(ty, method);
                    self.error(span, error);
                    self.args(args, &[]);
                    return Typed::UNKNOWN;
                }
            },
            None => {
                self.args(args, &[]);
                return Typed::UNKNOWN;
            }
        };
        self.call_callee(callee, name, args, span)
    }
    /// Checks a call, returning what's called's return type, or `None` if it
    /// can't be determined, along with what it compiles to. A function is
    /// called directly, and other values through the standard library's
    /// `Call` trait.
    pub(super) fn call(&mut self, callee: &Name, args: &[Arg], span: Span) -> Typed {
        let name = callee.to_string();
        match self.lookup(callee) {
            Ok(Binding::Function(index)) => {
                let function = Callee::Function(self.own_fn(index));
                self.call_callee(function, name, args, span)
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

        let params = signature.params.iter().enumerate();
        for ((arg, found), (index, param)) in args.iter().zip(found).zip(params) {
            if arg.label != param.label {
                self.error(
                    arg.span,
                    CheckError::WrongLabel {
                        function: function.clone(),
                        expected: param.label.clone(),
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
                        param: param.label.clone().or(param.name.map(str::to_string)),
                        position: index + 1,
                        expected: expected.clone(),
                        found,
                    },
                );
            }
        }

        signature.returns
    }
}
