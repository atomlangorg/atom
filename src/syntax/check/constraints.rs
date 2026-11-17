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
//! A generic type can have constraints too, as in `group Fn<Input, Output>
//! where Input == some Tuple`, which every type made from it must meet, so
//! its parameters implement their traits in its fields and all of its
//! `impl`s.
//!
//! An `impl` with constraints is only for the types made from its generic
//! type whose arguments meet them. Its functions are checked once, like any
//! other generic type's, where each parameter it constrains implements its
//! trait, with a function standing for each of the trait's. A call to one of
//! those calls the function of whatever the parameter stands for, once each
//! function is made for its arguments.

use super::*;

impl<'a> Checker<'a> {
    /// Works out the constraints of the generic type at `template` in
    /// `templates`, once it and every trait has been defined.
    pub(super) fn define_type_constraints(&mut self, template: usize) {
        let Template {
            file,
            decls,
            ref stand_ins,
            ..
        } = self.templates[template];
        let type_params = Some((template, stand_ins.clone()));
        let constraints = self.at_top_level(file, type_params, |checker| {
            checker.define_constraints(decls, Some(template))
        });
        self.templates[template].constraints = constraints;
    }
    /// Checks that the arguments of `unchecked` meet its generic type's
    /// constraints, reporting the first they don't. Returns whether they do.
    pub(super) fn check_instance(&mut self, unchecked: UncheckedInstance) -> bool {
        let UncheckedInstance {
            file,
            name,
            template,
            args,
            constraints,
        } = unchecked;
        let outer = std::mem::replace(&mut self.cx.constraints, constraints);
        let unmet = self
            .unmet_constraint(self.templates[template].constraints, &args)
            .map(|(decl, param)| {
                let arg = self.params(template, &args)[&param.id].clone();
                (decl.to_string(), param.name.clone(), arg)
            });
        self.cx.constraints = outer;
        let Some((constraint, param, arg)) = unmet else {
            return true;
        };
        let error = CheckError::UnmetTypeConstraint {
            ty: self.instance_name(template, &args),
            constraint,
            param,
            arg,
        };
        self.error_in(file, name.span, error);
        false
    }
    /// Works out `decls`, returning their index in `constraints` if there
    /// are any. They're the constraints of the generic type at `of_type` in
    /// `templates` if it's given, which can only be that its parameters
    /// implement traits, and otherwise of the `impl` being declared. Each
    /// must be on one of the parameters of the generic type.
    pub(super) fn define_constraints(
        &mut self,
        decls: &'a [ast::Constraint],
        of_type: Option<usize>,
    ) -> Option<usize> {
        if decls.is_empty() {
            return None;
        }
        let template = of_type.or_else(|| self.generic_context());
        let mut constraints = vec![];
        let mut equal = vec![];
        for decl in decls {
            // Everything is resolved, so each part's errors are found.
            let param = self.resolve_type(&decl.ty);
            let (trait_name, aliases) = match &decl.bound {
                ast::Bound::Trait { name, aliases } => (name, aliases),
                ast::Bound::Type(ty) => {
                    let ty = self.resolve_type(ty);
                    if of_type.is_some() {
                        let error = CheckError::TypeConstraintNotTrait(decl.to_string());
                        self.error(decl.span, error);
                    } else if let Some(param) =
                        self.constrained_param(param, (template, of_type), decl)
                        && let Some(ty) = ty
                    {
                        equal.push(EqualConstraint { decl, param, ty });
                    }
                    continue;
                }
            };
            let trait_index = self.resolve_trait(trait_name);
            let param = self.constrained_param(param, (template, of_type), decl);
            let mut given = HashMap::new();
            for alias in aliases {
                let ty = alias.ty.as_ref().and_then(|ty| self.resolve_type(ty));
                if let Some(trait_index) = trait_index
                    && !self.traits[trait_index]
                        .aliases
                        .iter()
                        .any(|declared| declared.name == alias.name)
                {
                    let error = CheckError::NotInTrait {
                        trait_name: trait_name.to_string(),
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
            let aliases = self.constraint_aliases(&param, trait_index, given);
            constraints.push(ConstraintInfo {
                decl,
                param,
                trait_index,
                aliases,
                functions: HashMap::new(),
                implied: false,
            });
        }
        // Without a generic type, every constraint has been reported.
        let template = template?;
        let constraints = self.with_implied(template, constraints);
        let count = constraints.len();
        self.constraints.push(Constraints {
            template,
            constraints,
            equal,
        });
        let index = self.constraints.len() - 1;
        for constraint in 0..count {
            self.declare_constraint_fns(index, constraint);
        }
        Some(index)
    }
    /// What each alias of the trait at `trait_index` in `traits` is for
    /// `param`, which a constraint says implements it, in order: the types
    /// it gives in `given`, by name, and otherwise a stand-in for whatever
    /// the implementation gives, as in `T.Rhs`.
    fn constraint_aliases(
        &mut self,
        param: &Type,
        trait_index: usize,
        mut given: HashMap<&str, Option<Type>>,
    ) -> Vec<(&'a str, ConstraintAlias)> {
        let file = self.cx.file;
        self.traits[trait_index]
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
            .collect()
    }
    /// `constraints`, on the parameters of the generic type at `template` in
    /// `templates`, followed by the constraints they imply. A type that
    /// implements a trait gives its aliases types that meet the trait's own
    /// constraints, so `T == some Call` implies `T.Input == some Tuple`, and
    /// `T == some Call<Input = U>` implies `U == some Tuple`. Only those on a
    /// parameter, or on a stand-in for an alias, are implied, since any
    /// other type either meets them or makes the constraint impossible.
    ///
    /// They're implied through each alias as deeply as they go, except
    /// through a trait that's already been implied on the way there, since a
    /// trait whose alias implements the trait itself would never end.
    fn with_implied(
        &mut self,
        template: usize,
        mut constraints: Vec<ConstraintInfo<'a>>,
    ) -> Vec<ConstraintInfo<'a>> {
        // The traits each constraint was implied through, including its own.
        let mut traits: Vec<_> = constraints
            .iter()
            .map(|constraint| vec![constraint.trait_index])
            .collect();
        let mut next = 0;
        while next < constraints.len() {
            let ConstraintInfo {
                decl,
                ref param,
                trait_index,
                ref aliases,
                ..
            } = constraints[next];
            let trait_ = self.traits[trait_index].clone();
            // What the trait's `Self` and aliases are for `param`. An alias
            // whose type is unknown has been reported.
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
            for implied in &trait_.constraints {
                let Some(ty) = params.get(&implied.alias.id).cloned() else {
                    continue;
                };
                let is_stand_in = self.templates[template].stand_ins.contains(&ty)
                    || constraints.iter().any(|constraint| {
                        constraint.aliases.iter().any(
                            |(_, alias)| matches!(alias, ConstraintAlias::Any(any) if *any == ty),
                        )
                    });
                let known = constraints.iter().any(|constraint| {
                    constraint.param == ty && constraint.trait_index == implied.trait_index
                });
                if !is_stand_in || known || traits[next].contains(&implied.trait_index) {
                    continue;
                }
                let mut given = HashMap::new();
                for (name, alias) in &implied.given {
                    let alias = alias.as_ref().and_then(|alias| {
                        if unknown
                            .iter()
                            .any(|stand_in| self.mentions(alias, stand_in))
                        {
                            return None;
                        }
                        self.substitute(alias, &params, decl.span)
                    });
                    given.insert(*name, alias);
                }
                let aliases = self.constraint_aliases(&ty, implied.trait_index, given);
                constraints.push(ConstraintInfo {
                    decl,
                    param: ty,
                    trait_index: implied.trait_index,
                    aliases,
                    functions: HashMap::new(),
                    implied: true,
                });
                let mut through = traits[next].clone();
                through.push(implied.trait_index);
                traits.push(through);
            }
            next += 1;
        }
        constraints
    }
    /// `param`, which `decl` constrains, if it's one of the parameters of the
    /// generic type at `template` in `templates`, reporting an error if it
    /// isn't. `None` if it's unknown. `of_type` is given if they're the
    /// generic type's own constraints, as in `define_constraints`.
    fn constrained_param(
        &mut self,
        param: Option<Type>,
        (template, of_type): (Option<usize>, Option<usize>),
        decl: &ast::Constraint,
    ) -> Option<Type> {
        match (param, template) {
            (Some(param), Some(template))
                if self.templates[template].stand_ins.contains(&param) =>
            {
                Some(param)
            }
            (Some(ty), _) => {
                let error = match of_type {
                    Some(template) => CheckError::TypeConstrainedNotParam {
                        generic: self.templates[template].name.to_string(),
                        ty,
                    },
                    None => CheckError::ConstrainedNotParam(ty),
                };
                self.error(decl.span, error);
                None
            }
            (None, _) => None,
        }
    }
    /// The type of the `impl` whose constraints are at `constraints` in
    /// `constraints`, if it has any, where its generic type is `generic`.
    /// Each parameter that a constraint says is a type is that type.
    pub(super) fn constrained_type(&mut self, generic: Type, constraints: Option<usize>) -> Type {
        let Some(constraints) = constraints else {
            return generic;
        };
        let params: HashMap<_, _> = self.constraints[constraints]
            .equal
            .iter()
            .map(|equal| (equal.param.id, equal.ty.clone()))
            .collect();
        if params.is_empty() {
            return generic;
        }
        let span = self.constraints[constraints].equal[0].decl.span;
        self.substitute(&generic, &params, span).unwrap_or(generic)
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
                    trait_index,
                    name: &declared.name,
                },
            );
            self.constraints[index].constraints[constraint]
                .functions
                .insert(&declared.name, function);
        }
    }
    /// The constraints on `ty` where the code being checked is, along with
    /// the generic type whose parameter it constrains, as an index into
    /// `templates`. They're those of the current `impl`, and if `ty` is a
    /// generic type's parameter, or an alias its constraints imply something
    /// of, its generic type's own, which hold wherever it can be used.
    fn constraints_on(&self, ty: &Type) -> impl Iterator<Item = (usize, &ConstraintInfo<'a>)> {
        let own = self
            .templates
            .iter()
            .filter_map(|template| template.constraints)
            .find(|&index| {
                self.constraints[index]
                    .constraints
                    .iter()
                    .any(|constraint| constraint.param == *ty)
            });
        self.cx
            .constraints
            .into_iter()
            .chain(own)
            .flat_map(move |index| {
                let Constraints {
                    template,
                    constraints,
                    ..
                } = &self.constraints[index];
                constraints
                    .iter()
                    .filter(move |constraint| constraint.param == *ty)
                    .map(move |constraint| (*template, constraint))
            })
    }
    /// The constraint where the code being checked is that `ty` implements
    /// the trait at `trait_index` in `traits`, if there is one, along with
    /// the generic type whose parameter it constrains, as `constraints_on`
    /// says.
    pub(super) fn constraint_on(
        &self,
        ty: &Type,
        trait_index: usize,
    ) -> Option<(usize, &ConstraintInfo<'a>)> {
        self.constraints_on(ty)
            .find(|(_, constraint)| constraint.trait_index == trait_index)
    }
    /// The method called `name` that `ty` has from the traits it's
    /// constrained to where the code being checked is.
    pub(super) fn constrained_method(&self, ty: &Type, name: &str) -> Option<FnRef> {
        let (template, index) = self.constraints_on(ty).find_map(|(template, constraint)| {
            Some((template, *constraint.functions.get(name)?))
        })?;
        let args = self.templates[template].stand_ins.clone();
        Some(FnRef { index, args })
    }
    /// The function of `ty` that implements the function called `name` of
    /// the trait at `trait_index` in `traits`, if it has an implementation of
    /// the trait, whether or not it meets that implementation's constraints.
    /// A parameter constrained to implement the trait, by the current `impl`
    /// or its own generic type, has the function standing for it.
    pub(super) fn trait_fn(&self, ty: &Type, trait_index: usize, name: &str) -> Option<FnRef> {
        if let Some((template, constraint)) = self.constraint_on(ty, trait_index) {
            let index = *constraint.functions.get(name)?;
            let args = self.templates[template].stand_ins.clone();
            return Some(FnRef { index, args });
        }
        let (owner, args) = self.owner(ty);
        let implementation = self.implementations.get(&(owner, trait_index))?;
        let index = *implementation.functions.get(name)?;
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
    /// Returns the constraint's declaration, and the stand-in for the
    /// parameter it constrains.
    pub(super) fn unmet_constraint(
        &self,
        constraints: Option<usize>,
        args: &[Type],
    ) -> Option<(&'a ast::Constraint, &Type)> {
        let Constraints {
            template,
            constraints,
            equal,
        } = &self.constraints[constraints?];
        let params = self.params(*template, args);
        let none = HashMap::new();
        let unequal = equal
            .iter()
            .find(|equal| !self.same_type(&equal.ty, &params, &params[&equal.param.id], &none))
            .map(|equal| (equal.decl, &equal.param));
        // The implied constraints are met whenever the others are.
        let unimplemented = || {
            let constraint = constraints
                .iter()
                .filter(|c| !c.implied)
                .find(|constraint| {
                    let arg = &params[&constraint.param.id];
                    let given_match = constraint.aliases.iter().all(|(name, alias)| {
                        let ConstraintAlias::Given(Some(given)) = alias else {
                            return true;
                        };
                        // An unknown type has already been reported.
                        self.alias_of(arg, constraint.trait_index, name).is_none_or(
                            |(found, found_params)| {
                                self.same_type(given, &params, &found, &found_params)
                            },
                        )
                    });
                    !(self.implements(arg, constraint.trait_index) && given_match)
                })?;
            Some((constraint.decl, &constraint.param))
        };
        unequal.or_else(unimplemented)
    }
    /// The error for `ty` not having its `member`, which is `declared`,
    /// because it doesn't meet the constraints of its `impl`, if that's why.
    pub(super) fn unmet(&self, ty: &Type, member: &str, declared: &FnRef) -> Option<CheckError> {
        let constraints = self.functions[declared.index].constraints;
        let (decl, param) = self.unmet_constraint(constraints, &declared.args)?;
        let template = self.constraints[constraints?].template;
        let arg = self.params(template, &declared.args)[&param.id].clone();
        Some(CheckError::UnmetConstraint {
            ty: ty.clone(),
            member: member.to_string(),
            constraint: decl.to_string(),
            param: param.name.clone(),
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
        if let Some((_, constraint)) = self.constraint_on(ty, trait_index) {
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
        // An implied constraint comes after those it's implied by, so what
        // its parameter stands for has been found by the time it's needed.
        for index in 0..self.constraints[constraints].constraints.len() {
            let constraint = &self.constraints[constraints].constraints[index];
            let Some(arg) = params.get(&constraint.param.id) else {
                continue;
            };
            let mut found = vec![];
            for (name, alias) in &constraint.aliases {
                if let ConstraintAlias::Any(stand_in) = alias {
                    let (ty, ty_params) = self
                        .alias_of(arg, constraint.trait_index, name)
                        .expect(NO_ERRORS);
                    found.push((stand_in.id, ty, ty_params));
                }
            }
            for (id, ty, ty_params) in found {
                let ty = self.substitute(&ty, &ty_params, span)?;
                params.insert(id, ty);
            }
        }
        Some(())
    }
    /// The function that `function` calls, which is itself unless it stands
    /// for a function of a constraint's trait, when it's the function of the
    /// type its parameter stands for in `function`'s arguments, which is
    /// made for the code at `span`. An implied constraint's parameter can be
    /// an alias, which is found from those arguments. Returns `None` if that
    /// makes a generic type nested too deeply, which has been reported.
    pub(super) fn constrained_callee(&mut self, function: FnRef, span: Span) -> Option<FnRef> {
        let Some(&FnStandIn {
            ref param,
            trait_index,
            name,
        }) = self.fn_stand_ins.get(&function.index)
        else {
            return Some(function);
        };
        let param = param.clone();
        let Signature {
            template,
            constraints,
            ..
        } = self.functions[function.index];
        let template = template.expect("only generic types have constraints");
        let mut params = self.params(template, &function.args);
        if !params.contains_key(&param.id) {
            let constraints = constraints.expect("only constraints have stand-ins");
            self.add_alias_params(constraints, &mut params, span)?;
        }
        // The constraint was met where the function was used.
        Some(
            self.trait_fn(&params[&param.id], trait_index, name)
                .expect(NO_ERRORS),
        )
    }
}
