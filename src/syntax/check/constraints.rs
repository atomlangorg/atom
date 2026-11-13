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

//! The constraints of an `impl`'s `where`, as in `impl Optional with Equal
//! where Self.T == some Equal<Rhs = Self.T>`.
//!
//! An `impl` with constraints is only for the types made from its generic
//! type whose arguments meet them. Its functions are checked once, like any
//! other generic type's, where each parameter it constrains implements its
//! trait, with a function standing for each of the trait's. A call to one of
//! those calls the function of whatever the parameter stands for, once each
//! function is made for its arguments.

use super::*;

impl<'a> Checker<'a> {
    /// Works out the `constraints` of the `impl` being declared, returning
    /// their index in `constraints` if there are any. Each must be on one of
    /// the parameters of the `impl`'s generic type.
    pub(super) fn define_constraints(&mut self, decls: &'a [ast::Constraint]) -> Option<usize> {
        if decls.is_empty() {
            return None;
        }
        let template = self.generic_context();
        let mut constraints = vec![];
        for decl in decls {
            // Everything is resolved, so each part's errors are found.
            let param = self.resolve_type(&decl.ty);
            let trait_index = self.resolve_trait(&decl.trait_name);
            let param = match (param, template) {
                (Some(param), Some(template))
                    if self.templates[template].stand_ins.contains(&param) =>
                {
                    Some(param)
                }
                (Some(param), _) => {
                    self.error(decl.span, CheckError::ConstrainedNotParam(param));
                    None
                }
                (None, _) => None,
            };
            let mut given = HashMap::new();
            for alias in &decl.aliases {
                let ty = alias.ty.as_ref().and_then(|ty| self.resolve_type(ty));
                if let Some(trait_index) = trait_index
                    && !self.traits[trait_index]
                        .aliases
                        .iter()
                        .any(|declared| declared.name == alias.name)
                {
                    let error = CheckError::NotInTrait {
                        trait_name: decl.trait_name.to_string(),
                        member: alias.name.clone(),
                    };
                    self.error(alias.span, error);
                } else if given.insert(alias.name.as_str(), ty).is_some() {
                    self.error(alias.span, CheckError::Redeclared(alias.name.clone()));
                }
            }
            let (Some(param), Some(trait_index)) = (param, trait_index) else {
                continue;
            };
            let file = self.cx.file;
            let aliases = self.traits[trait_index]
                .aliases
                .iter()
                .map(|declared| {
                    let name = declared.name.as_str();
                    let alias = match given.remove(name) {
                        Some(ty) => ConstraintAlias::Given(ty),
                        None => {
                            let shown = format!("{param}.{name}");
                            let id = self.add_type(shown.clone(), file, TypeKind::Group(vec![]));
                            ConstraintAlias::Any(Type {
                                id,
                                name: shown,
                                parts: None,
                            })
                        }
                    };
                    (name, alias)
                })
                .collect();
            constraints.push(ConstraintInfo {
                decl,
                param,
                trait_index,
                aliases,
                functions: HashMap::new(),
            });
        }
        // Without a generic type, every constraint has been reported.
        let template = template?;
        let count = constraints.len();
        self.constraints.push(Constraints {
            template,
            constraints,
        });
        let index = self.constraints.len() - 1;
        for constraint in 0..count {
            self.declare_constraint_fns(index, constraint);
        }
        Some(index)
    }
    /// Declares a function for each of the methods of the trait of the
    /// constraint at `constraint` in the constraints at `index` in
    /// `constraints`, as the type it constrains has it. Its static functions
    /// can't be called, since the type can't be named as a value.
    fn declare_constraint_fns(&mut self, index: usize, constraint: usize) {
        let template = self.constraints[index].template;
        let ConstraintInfo {
            decl,
            ref param,
            trait_index,
            ref aliases,
            ..
        } = self.constraints[index].constraints[constraint];
        let param = param.clone();
        let trait_ = self.traits[trait_index].clone();
        // What the trait's `Self` and aliases stand for. An alias whose type
        // is unknown has been reported, so the types using it are unknown.
        let mut params = HashMap::from([(trait_.stand_ins[0].id, param.clone())]);
        let mut unknown = vec![];
        for ((_, alias), stand_in) in aliases.iter().zip(&trait_.stand_ins[1..]) {
            match alias {
                ConstraintAlias::Given(Some(ty)) | ConstraintAlias::Any(ty) => {
                    params.insert(stand_in.id, ty.clone());
                }
                ConstraintAlias::Given(None) => unknown.push(stand_in.clone()),
            }
        }
        let methods = trait_.functions.iter().zip(&trait_.signatures);
        for (declared, (param_types, returns)) in methods.filter(|(f, _)| !f.is_static) {
            let mut resolve = |ty: &Option<Type>| {
                let ty = ty.as_ref()?;
                if unknown.iter().any(|alias| self.mentions(ty, alias)) {
                    return None;
                }
                self.substitute(ty, &params, decl.span)
            };
            let params = declared
                .params
                .iter()
                .zip(param_types)
                .map(|(element, ty)| ParamSignature::declared(element, resolve(ty)))
                .collect();
            let returns = resolve(returns);
            self.functions.push(Signature {
                name: format!("{param}.{}", declared.name),
                params,
                returns,
                template: Some(template),
                constraints: Some(index),
            });
            self.lowered_functions.push(None);
            let function = self.functions.len() - 1;
            self.fn_stand_ins.insert(
                function,
                FnStandIn {
                    param: param.clone(),
                    name: &declared.name,
                },
            );
            self.constraints[index].constraints[constraint]
                .functions
                .insert(&declared.name, function);
        }
    }
    /// The constraint of the current `impl` that `ty` implements the trait
    /// at `trait_index` in `traits`, if it has one.
    pub(super) fn constraint_on(
        &self,
        ty: &Type,
        trait_index: usize,
    ) -> Option<&ConstraintInfo<'a>> {
        let index = self.cx.constraints?;
        self.constraints[index]
            .constraints
            .iter()
            .find(|constraint| constraint.param == *ty && constraint.trait_index == trait_index)
    }
    /// The method called `name` that `ty` has from the traits the current
    /// `impl` constrains it to.
    pub(super) fn constrained_method(&self, ty: &Type, name: &str) -> Option<FnRef> {
        let index = self.cx.constraints?;
        let Constraints {
            template,
            constraints,
        } = &self.constraints[index];
        let index = constraints
            .iter()
            .filter(|constraint| constraint.param == *ty)
            .find_map(|constraint| constraint.functions.get(name).copied())?;
        let args = self.templates[*template].stand_ins.clone();
        Some(FnRef { index, args })
    }
    /// Whether `function`'s arguments meet the constraints of the `impl` that
    /// declares it.
    pub(super) fn meets(&self, function: &FnRef) -> bool {
        let constraints = self.functions[function.index].constraints;
        self.unmet_constraint(constraints, &function.args).is_none()
    }
    /// The first of the constraints at `constraints` in `constraints`, if
    /// there are any, that `args`, the arguments of their generic type,
    /// don't meet.
    pub(super) fn unmet_constraint(
        &self,
        constraints: Option<usize>,
        args: &[Type],
    ) -> Option<&ConstraintInfo<'a>> {
        let Constraints {
            template,
            constraints,
        } = &self.constraints[constraints?];
        let params = self.params(*template, args);
        constraints.iter().find(|constraint| {
            let arg = &params[&constraint.param.id];
            let given_match = constraint.aliases.iter().all(|(name, alias)| {
                let ConstraintAlias::Given(Some(given)) = alias else {
                    return true;
                };
                // An unknown type has already been reported.
                self.alias_of(arg, constraint.trait_index, name).is_none_or(
                    |(found, found_params)| self.same_type(given, &params, &found, &found_params),
                )
            });
            !(self.implements(arg, constraint.trait_index) && given_match)
        })
    }
    /// The error for `ty` not having its `member`, which is `declared`,
    /// because it doesn't meet the constraints of its `impl`, if that's why.
    pub(super) fn unmet(&self, ty: &Type, member: &str, declared: &FnRef) -> Option<CheckError> {
        let constraints = self.functions[declared.index].constraints;
        let constraint = self.unmet_constraint(constraints, &declared.args)?;
        let template = self.constraints[constraints?].template;
        let arg = self.params(template, &declared.args)[&constraint.param.id].clone();
        Some(CheckError::UnmetConstraint {
            ty: ty.clone(),
            member: member.to_string(),
            constraint: constraint.decl.to_string(),
            param: constraint.param.name.clone(),
            arg,
        })
    }
    /// What `ty`'s implementation of the trait at `trait_index` in `traits`
    /// gives its alias called `alias`, along with what the stand-ins it uses
    /// stand for in `ty`, if it implements it.
    pub(super) fn alias_of(
        &self,
        ty: &Type,
        trait_index: usize,
        alias: &str,
    ) -> Option<(Type, HashMap<usize, Type>)> {
        if let Some(constraint) = self.constraint_on(ty, trait_index) {
            let (_, found) = constraint.aliases.iter().find(|(name, _)| *name == alias)?;
            let found = match found {
                ConstraintAlias::Given(ty) => ty.clone()?,
                ConstraintAlias::Any(ty) => ty.clone(),
            };
            return Some((found, HashMap::new()));
        }
        let (owner, args) = self.owner(ty);
        let implementation = self.implementations.get(&(owner, trait_index))?;
        let ImplAliasType::Known(Some(found)) =
            self.impl_aliases[implementation.aliases?].get(alias)?
        else {
            return None;
        };
        // A generic type's `impl` gives it with the generic type's
        // parameters.
        let params = match self.found_args(ty) {
            Some((template, _)) if owner != ty.id => self.params(template, &args),
            _ => HashMap::new(),
        };
        Some((found.clone(), params))
    }
    /// Whether `a`, with what `a_params` says stand-ins stand for in place of
    /// them, is the same type as `b`, with `b_params` in place of its.
    pub(super) fn same_type(
        &self,
        a: &Type,
        a_params: &HashMap<usize, Type>,
        b: &Type,
        b_params: &HashMap<usize, Type>,
    ) -> bool {
        let none = HashMap::new();
        if let Some(a) = a_params.get(&a.id) {
            return self.same_type(a, &none, b, b_params);
        }
        if let Some(b) = b_params.get(&b.id) {
            return self.same_type(a, a_params, b, &none);
        }
        if a_params.is_empty() && b_params.is_empty() {
            return a == b;
        }
        match (self.found_args(a), self.found_args(b)) {
            (Some((a_template, a_args)), Some((b_template, b_args))) => {
                a_template == b_template
                    && a_args
                        .iter()
                        .zip(&b_args)
                        .all(|(a, b)| self.same_type(a, a_params, b, b_params))
            }
            (None, None) => match (self.elements(a.id), self.elements(b.id)) {
                (Some(a_elements), Some(b_elements)) => {
                    a_elements.len() == b_elements.len()
                        && a_elements
                            .iter()
                            .zip(b_elements)
                            .all(|(a, b)| a.label == b.label)
                        && self
                            .element_types(a)
                            .iter()
                            .zip(&self.element_types(b))
                            .all(|(a, b)| self.same_type(a, a_params, b, b_params))
                }
                _ => a == b,
            },
            _ => false,
        }
    }
    /// Adds what the stand-ins for the aliases that the constraints at
    /// `constraints` in `constraints` don't give stand for to `params`, which
    /// says what their parameters stand for, for making a function at
    /// `span`. Returns `None` if that makes a generic type nested too deeply,
    /// which has been reported.
    pub(super) fn add_alias_params(
        &mut self,
        constraints: usize,
        params: &mut HashMap<usize, Type>,
        span: Span,
    ) -> Option<()> {
        let mut found = vec![];
        for constraint in &self.constraints[constraints].constraints {
            let Some(arg) = params.get(&constraint.param.id) else {
                continue;
            };
            for (name, alias) in &constraint.aliases {
                if let ConstraintAlias::Any(stand_in) = alias {
                    let (ty, ty_params) = self
                        .alias_of(arg, constraint.trait_index, name)
                        .expect(NO_ERRORS);
                    found.push((stand_in.id, ty, ty_params));
                }
            }
        }
        for (id, ty, ty_params) in found {
            let ty = self.substitute(&ty, &ty_params, span)?;
            params.insert(id, ty);
        }
        Some(())
    }
    /// The function that `function` calls, which is itself unless it stands
    /// for a function of a constraint's trait, when it's the function of the
    /// type its parameter stands for in `function`'s arguments.
    pub(super) fn constrained_callee(&self, function: FnRef) -> FnRef {
        let Some(FnStandIn { param, name }) = self.fn_stand_ins.get(&function.index) else {
            return function;
        };
        let template = self.functions[function.index]
            .template
            .expect("only generic types have constraints");
        let ty = &self.params(template, &function.args)[&param.id];
        // The constraint was met where the function was used.
        self.declared_method(ty, name).expect(NO_ERRORS)
    }
}
