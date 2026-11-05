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

//! Functions' signatures and bodies, and functions used as values.

use super::*;

impl<'a> Checker<'a> {
    /// Declares a function, returning its index into `functions`.
    pub(super) fn declare_fn(
        &mut self,
        name: &'a str,
        span: Span,
        params: &'a [Param],
        ret: &TypeName,
    ) -> usize {
        let index = self.signature(params, ret, self.full_name(name));
        self.declare(name, span, Binding::Function(index));
        index
    }
    /// Declares a method or static function of `self_ty`, which is `None` if
    /// it's unknown, returning its index into `functions`.
    pub(super) fn declare_method(
        &mut self,
        self_ty: Option<&Type>,
        method: (&'a String, &'a [Param], &TypeName),
        span: Span,
        is_static: bool,
        full_name: String,
    ) -> usize {
        let (name, params, ret) = method;
        let index = self.signature(params, ret, full_name);
        if let Some(self_ty) = self_ty {
            // Methods must be unique across every file, since a call can't
            // say which file's method it means.
            let existing = if is_static {
                match (
                    self.option(self_ty, name),
                    self.static_function(self_ty, name),
                ) {
                    (Some(_), _) => Some(MemberKind::Option),
                    (None, Some(_)) => Some(MemberKind::StaticFunction),
                    (None, None) => None,
                }
            } else {
                match self.member(self_ty, name) {
                    Some(Member::Field(..)) => Some(MemberKind::Field),
                    Some(Member::Method(_)) => Some(MemberKind::Method),
                    None => None,
                }
            };
            if let Some(existing) = existing {
                self.error(
                    span,
                    CheckError::MemberRedeclared {
                        ty: self_ty.clone(),
                        member: name.clone(),
                        existing,
                    },
                );
            } else {
                let info = &mut self.types[self_ty.id];
                let table = if is_static {
                    &mut info.statics
                } else {
                    &mut info.methods
                };
                table.insert(name, index);
            }
        }
        index
    }
    /// Resolves the parameter and return types of the function called
    /// `full_name`, returning its index into `functions`.
    pub(super) fn signature(
        &mut self,
        params: &'a [Param],
        ret: &TypeName,
        full_name: String,
    ) -> usize {
        let params = params
            .iter()
            .map(|param| ParamSignature {
                label: param.label.as_deref(),
                name: &param.name,
                span: param.span,
                ty: self.resolve_type(&param.ty),
            })
            .collect();
        let returns = self.resolve_type(ret);

        let template = self.generic_context();
        self.functions.push(Signature {
            name: full_name,
            params,
            returns,
            template,
        });
        self.lowered_functions.push(None);
        self.functions.len() - 1
    }
    /// Checks the body of the function at `index` in `functions`.
    /// `aliases` are the aliases of the `impl` a method or static function is
    /// in, as an index into `impl_aliases`. Other functions keep the aliases
    /// of the code they're nested in.
    pub(super) fn fn_body(
        &mut self,
        index: usize,
        name: &'a str,
        span: Span,
        body: &'a [Stmt],
        kind: FnKind,
        aliases: Option<usize>,
    ) {
        let Signature {
            name: full_name,
            params,
            returns,
            ..
        } = self.functions[index].clone();

        // The body starts a new function scope holding the parameters. It can
        // see the functions and types declared outside it, but not values.
        let outer_scope_start =
            std::mem::replace(&mut self.cx.fn_scope_start, self.cx.scopes.len());
        let outer_fn = self.cx.current_fn.replace(FnContext {
            name,
            full_name: full_name.clone(),
            returns: returns.clone(),
            is_static: matches!(kind, FnKind::Static(_)),
            locals: vec![],
        });
        self.cx.scopes.push(HashMap::new());
        let outer_impl_type = match &kind {
            FnKind::Method(ty) | FnKind::Static(ty) => self.cx.impl_type.replace(ty.clone()),
            FnKind::Function => self.cx.impl_type.clone(),
        };
        let outer_impl_aliases = match &kind {
            FnKind::Method(_) | FnKind::Static(_) => {
                std::mem::replace(&mut self.cx.impl_aliases, aliases)
            }
            FnKind::Function => self.cx.impl_aliases,
        };

        // `self` is a keyword, so it can't clash with a parameter. Like any
        // other value, functions nested in the method can't use it.
        if let FnKind::Method(ty) = kind {
            self.declare_local("self", span, Mutability::Let, ty);
        }
        for param in params {
            self.declare_local(param.name, param.span, Mutability::Let, param.ty);
        }
        let param_count = self.cx.current_fn.as_ref().expect("set above").locals.len();
        let (mut lowered, returns_always) = self.stmts(body);

        self.cx.scopes.pop();
        let locals = std::mem::replace(&mut self.cx.current_fn, outer_fn)
            .expect("set above")
            .locals;
        self.cx.fn_scope_start = outer_scope_start;
        self.cx.impl_type = outer_impl_type;
        self.cx.impl_aliases = outer_impl_aliases;

        // A `Void` function can finish without an explicit `return`.
        if !returns_always {
            match &returns {
                Some(expected) if *expected != self.std_type("Void") => {
                    self.error(
                        span,
                        CheckError::MissingReturn {
                            function: name.to_string(),
                            expected: expected.clone(),
                        },
                    );
                }
                _ => lowered.push(ir::Stmt::Return(self.void())),
            }
        }

        self.lowered_functions[index] = locals
            .into_iter()
            .map(|ty| ty.map(|ty| ty.id))
            .collect::<Option<_>>()
            .zip(returns)
            .map(|(locals, returns)| ir::Function {
                name: full_name,
                locals,
                params: param_count,
                returns: returns.id,
                body: ir::Body::Stmts(lowered),
            });
    }
    /// Records the primitive function called `name` at `index` in
    /// `functions`, such as `exit`, or `and` from an `impl` of the type
    /// called `owner`, such as `Bool`. It must be declared in the standard
    /// library.
    pub(super) fn define_primitive_fn(
        &mut self,
        index: usize,
        owner: Option<&str>,
        name: &str,
        span: Span,
        kind: FnKind,
    ) {
        let shown = match owner {
            Some(owner) => format!("{owner}.{name}"),
            None => name.to_string(),
        };
        if self.files[self.cx.file].package != Some(STDLIB_PACKAGE) {
            self.error(span, CheckError::PrimitiveOutsideStdlib(shown));
            return;
        }
        // Operators work on integers, which may be signed, `Bool`s and
        // `ChoiceTag`s, and are named after the functions they call.
        let signed = match &kind {
            FnKind::Method(Some(ty)) => self.int_type(ty).is_some_and(|(_, signed)| signed),
            _ => false,
        };
        let is_binary = |op: &BinOp| op.trait_function().1 == name;
        let is_unary = |op: &UnOp| op.trait_function().1 == name;
        let primitive = match (owner, name) {
            (None, "exit") => ir::PrimitiveFn::Exit,
            (Some(_), _) if let Some(op) = BinOp::ALL.into_iter().find(is_binary) => {
                ir::PrimitiveFn::Binary { op, signed }
            }
            (Some(_), _) if let Some(op) = UnOp::ALL.into_iter().find(is_unary) => {
                ir::PrimitiveFn::Unary(op)
            }
            (Some("Fn"), "call") => ir::PrimitiveFn::CallFn,
            (Some("ChoiceTag"), "new") => ir::PrimitiveFn::ChoiceTag,
            (Some("ChoiceOption"), "tag") => ir::PrimitiveFn::OptionTag,
            (Some("ChoiceOption"), "call") => ir::PrimitiveFn::CallOption,
            _ => panic!("the compiler doesn't know how to implement `{shown}`"),
        };
        let Signature {
            name: full_name,
            params,
            returns,
            ..
        } = self.functions[index].clone();
        // Like any method, `self` is its first local.
        let receiver = match kind {
            FnKind::Method(ty) => Some(ty),
            FnKind::Function | FnKind::Static(_) => None,
        };
        let locals: Option<Vec<_>> = receiver
            .into_iter()
            .chain(params.into_iter().map(|param| param.ty))
            .map(|ty| Some(ty?.id))
            .collect();
        self.lowered_functions[index] = locals.zip(returns).map(|(locals, returns)| ir::Function {
            name: full_name,
            params: locals.len(),
            locals,
            returns: returns.id,
            body: ir::Body::Primitive(primitive),
        });
    }
    /// The function at `index` in `functions`, referred to where it's
    /// declared, so its generic type's parameters stand for themselves.
    pub(super) fn own_fn(&self, index: usize) -> FnRef {
        let args = match self.functions[index].template {
            Some(template) => self.templates[template].stand_ins.clone(),
            None => vec![],
        };
        FnRef { index, args }
    }
    /// The signature of `function`, with what its generic type's parameters
    /// stand for in place of them, for the code at `span`.
    pub(super) fn signature_of(&mut self, function: &FnRef, span: Span) -> Signature<'a> {
        let signature = self.functions[function.index].clone();
        let Some(template) = signature.template else {
            return signature;
        };
        let params = self.params(template, &function.args);
        let mut substitute =
            |ty: Option<Type>| ty.and_then(|ty| self.substitute(&ty, &params, span));
        Signature {
            params: signature
                .params
                .into_iter()
                .map(|param| ParamSignature {
                    ty: substitute(param.ty),
                    ..param
                })
                .collect(),
            returns: substitute(signature.returns),
            ..signature
        }
    }
    /// The index in `callees` of `function`, which the code at `span` calls
    /// or uses as a value.
    pub(super) fn callee(&mut self, function: FnRef, span: Span) -> usize {
        if let Some(&index) = self.callee_ids.get(&function) {
            return index;
        }
        self.callees.push((function.clone(), self.cx.file, span));
        self.callee_ids.insert(function, self.callees.len() - 1);
        self.callees.len() - 1
    }
    /// The generic type whose `impl` the code being checked is in, as an
    /// index into `templates`, if it's in one, including inside one of its
    /// functions.
    pub(super) fn generic_context(&self) -> Option<usize> {
        let Some(Some(ty)) = &self.cx.impl_type else {
            return None;
        };
        let (template, _) = self.types[ty.id].instance.as_ref()?;
        (self.templates[*template].generic.as_ref() == Some(ty)).then_some(*template)
    }
    /// The type `Fn<Input, Output>` of `function`, or `None` if it's unknown.
    /// Its `Input` is a tuple of its parameters, which have only their labels,
    /// since their names are private to the function. It's used by the code
    /// at `span`.
    pub(super) fn function_type(&mut self, function: &FnRef, span: Span) -> Option<Type> {
        let Signature {
            params, returns, ..
        } = self.signature_of(function, span);
        let elements = params.iter().map(|param| {
            Some(TupleElementType {
                label: param.label.map(str::to_string),
                name: param.label.map(str::to_string),
                ty: param.ty.clone()?,
            })
        });
        let input = elements.collect::<Option<_>>()?;
        let input = self.tuple_type(input);
        let template = self.std_template("Fn");
        self.instance(template, vec![input, returns?], span)
    }
    /// `function` as a value, which compiles to a pointer to a function taking
    /// its `Input` as a single parameter, so that every `Fn` can be called in
    /// the same way. It's used by the code at `span`.
    pub(super) fn function_value(&mut self, function: FnRef, span: Span) -> Typed {
        let Some(ty) = self.function_type(&function, span) else {
            return Typed::UNKNOWN;
        };
        let index = match self.function_values.get(&function.index) {
            Some(&index) => index,
            None => self.single_input_function(function.index, span),
        };
        let value = FnRef {
            index,
            args: function.args,
        };
        Typed {
            ty: Some(ty),
            expr: ir::Expr::Function(self.callee(value, span)),
        }
    }
    /// Makes a function that calls the function at `index` in `functions` with
    /// the elements of its `Input` as a single parameter, returning its index
    /// in `functions`. It's for the same generic type as the function, if
    /// any, so it's only made once. It's used by the code at `span`.
    pub(super) fn single_input_function(&mut self, index: usize, span: Span) -> usize {
        let function = self.own_fn(index);
        let ty = self
            .function_type(&function, span)
            .expect("the function's type is known");
        let (_, fn_args) = self.types[ty.id].instance.as_ref().expect("made from `Fn`");
        let input = fn_args[0].id;
        let Signature {
            name,
            params,
            returns,
            template,
        } = self.functions[index].clone();
        let returns = returns.expect("the function's type is known");
        let args = (0..params.len())
            .map(|index| ir::Expr::Field {
                receiver: Box::new(ir::Expr::Local(0)),
                index,
            })
            .collect();
        let call = self.callee(function, span);
        let value = self.functions.len();
        let value_name = format!("{name}.value");
        self.functions.push(Signature {
            name: value_name.clone(),
            params: vec![],
            returns: Some(returns.clone()),
            template,
        });
        self.lowered_functions.push(Some(ir::Function {
            name: value_name,
            locals: vec![input],
            params: 1,
            returns: returns.id,
            body: ir::Body::Stmts(vec![ir::Stmt::Return(ir::Expr::Call {
                function: call,
                args,
            })]),
        }));
        self.function_values.insert(index, value);
        value
    }
}
