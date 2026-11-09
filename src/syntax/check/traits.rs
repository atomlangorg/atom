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

//! Traits, and the `impl`s that implement them.

use super::*;

/// The names in the tuple type `ty` that are `Self` or one of `aliases`,
/// which stand for a different type in each `impl` of a trait.
pub(super) fn trait_names<'b>(
    ty: &'b TypeName,
    aliases: &'b [String],
) -> Box<dyn Iterator<Item = &'b Name> + 'b> {
    match ty {
        TypeName::Named(name) if name.file.is_none() => {
            let is_trait_name = name.name == "Self" || aliases.contains(&name.name);
            Box::new(is_trait_name.then_some(name).into_iter())
        }
        TypeName::Named(_) | TypeName::SelfParam(..) => Box::new(iter::empty()),
        TypeName::Tuple(elements) => Box::new(
            elements
                .iter()
                .flat_map(move |element| trait_names(&element.ty, aliases)),
        ),
        TypeName::Generic { args, .. } => {
            Box::new(args.iter().flat_map(move |arg| trait_names(arg, aliases)))
        }
    }
}

/// How `function` is written in its trait, such as `fn and(_ rhs: Self) ->
/// Bool`, for messages.
pub(super) fn signature_text(function: &TraitFunction) -> String {
    let params: Vec<_> = function
        .params
        .iter()
        .map(|param| match &param.label {
            None => format!("_ {}: {}", param.name, param.ty),
            Some(label) if *label == param.name => format!("{}: {}", param.name, param.ty),
            Some(label) => format!("{label} {}: {}", param.name, param.ty),
        })
        .collect();
    let static_ = if function.is_static { "static " } else { "" };
    let returns = if function.ret.is_void() {
        String::new()
    } else {
        format!(" -> {}", function.ret)
    };
    format!(
        "{static_}fn {}({}){returns}",
        function.name,
        params.join(", ")
    )
}

impl<'a> Checker<'a> {
    /// Declares the functions of `impl ty`, whose type is `self_ty`, `with`
    /// a trait if given, returning their bodies to check, with the files
    /// they're written in.
    pub(super) fn declare_impl(
        &mut self,
        ty: &'a Name,
        self_ty: Option<Type>,
        with: Option<&'a Name>,
        aliases: &'a [ImplAlias],
        functions: &'a [ImplFunction],
    ) -> Vec<(usize, Pending<'a>)> {
        // A generic type's functions are named with its parameters, and each
        // type made from it names them with its arguments.
        let type_name = match &self_ty {
            Some(self_ty) if self.types[self_ty.id].instance.is_some() => self_ty.name.clone(),
            _ => ty.name.clone(),
        };
        let mut pending = vec![];
        let trait_index = with.and_then(|with| self.resolve_trait(with));
        if with.is_none() {
            for alias in aliases {
                let name = alias.name.clone();
                self.error(alias.span, CheckError::ImplAliasWithoutTrait(name));
            }
        }
        // Signatures can use `Self` and the `impl`'s aliases, just like
        // bodies.
        let impl_aliases = self.in_impl(self_ty.clone(), None, |checker| {
            checker.define_impl_aliases(aliases, trait_index)
        });
        let mut indexes = vec![];
        self.in_impl(self_ty.clone(), impl_aliases, |checker| {
            for ImplFunction {
                is_static,
                is_primitive,
                function,
            } in functions
            {
                let name = format!("{type_name}.{}", function.name);
                let full_name = checker.full_name(&name);
                let index = checker.declare_method(
                    self_ty.as_ref(),
                    (&function.name, &function.params, &function.ret),
                    function.span,
                    *is_static,
                    full_name,
                );
                indexes.push(index);
                let kind = if *is_static {
                    FnKind::Static(self_ty.clone())
                } else {
                    FnKind::Method(self_ty.clone())
                };
                if *is_primitive {
                    let owner = Some(ty.name.as_str());
                    checker.define_primitive_fn(index, owner, &function.name, function.span, kind);
                    continue;
                }
                let body = Pending::Fn {
                    index,
                    name: &function.name,
                    span: function.span,
                    body: &function.body,
                    kind,
                    aliases: impl_aliases,
                };
                pending.push((checker.cx.file, body));
            }
        });
        if let Some((with, (trait_index, aliases))) = with.zip(trait_index.zip(impl_aliases)) {
            let implemented: Vec<_> = functions.iter().zip(&indexes).collect();
            self.infer_impl_aliases(ty, with, trait_index, aliases, &implemented);
        }
        if let Some(trait_index) = trait_index {
            let defaults = self.declare_defaults(
                &type_name,
                self_ty.as_ref(),
                trait_index,
                functions,
                impl_aliases,
            );
            pending.extend(defaults);
        }
        if let Some((with, (trait_index, self_ty))) = with.zip(trait_index.zip(self_ty)) {
            self.implementations.insert((self_ty.id, trait_index));
            let implemented = functions.iter().zip(indexes).collect();
            self.check_implements(
                Implementation {
                    ty,
                    self_ty: &self_ty,
                    trait_name: with,
                    index: trait_index,
                    aliases,
                    alias_types: impl_aliases,
                },
                implemented,
            );
        }
        pending
    }
    /// Declares the trait called `name` in the current file. Its signatures
    /// are worked out by `define_trait`, once types are defined.
    pub(super) fn declare_trait(
        &mut self,
        name: &'a str,
        span: Span,
        aliases: &'a [String],
        functions: &'a [TraitFunction],
    ) {
        self.traits.push(TraitInfo {
            name,
            span,
            file: self.cx.file,
            aliases,
            functions,
            stand_ins: vec![],
            signatures: vec![],
        });
        self.declare(name, span, Binding::Trait(self.traits.len() - 1));
    }
    /// Works out the types in the signatures of the trait at `index` in
    /// `traits`, where `Self` stands for whichever type implements it, and
    /// its aliases for whichever types the implementation gives.
    pub(super) fn define_trait(&mut self, index: usize) {
        let TraitInfo {
            name,
            span,
            file,
            aliases,
            functions,
            ..
        } = self.traits[index];
        self.cx.file = file;
        // Aliases and functions have separate names, like types and methods.
        let mut alias_names = HashSet::new();
        let mut function_names = HashSet::new();
        let redeclared = aliases
            .iter()
            .filter(|alias| !alias_names.insert(*alias))
            .map(|alias| (span, alias))
            .chain(
                functions
                    .iter()
                    .filter(|function| !function_names.insert(&function.name))
                    .map(|function| (function.span, &function.name)),
            );
        let errors: Vec<_> = redeclared
            .map(|(span, member)| {
                let error = CheckError::TraitMemberRedeclared {
                    trait_name: name.to_string(),
                    member: member.clone(),
                };
                (span, error)
            })
            .collect();
        for (span, error) in errors {
            self.error(span, error);
        }

        // The signatures are resolved with `Self` and the aliases standing
        // for their stand-ins.
        let stand_ins: Vec<_> = iter::once("Self")
            .chain(aliases.iter().map(String::as_str))
            .map(|name| Type {
                id: self.add_type(name.to_string(), file, TypeKind::Group(vec![])),
                name: name.to_string(),
            })
            .collect();
        let alias_types = aliases.iter().zip(&stand_ins[1..]);
        let alias_types =
            alias_types.map(|(alias, ty)| (alias.as_str(), ImplAliasType::Known(Some(ty.clone()))));
        self.impl_aliases.push(alias_types.collect());
        let alias_types = Some(self.impl_aliases.len() - 1);
        let signatures = self.in_impl(Some(stand_ins[0].clone()), alias_types, |checker| {
            let mut signatures = vec![];
            for function in functions {
                let params = function
                    .params
                    .iter()
                    .map(|param| checker.trait_type(aliases, &param.ty))
                    .collect();
                signatures.push((params, checker.trait_type(aliases, &function.ret)));
            }
            signatures
        });
        self.traits[index].stand_ins = stand_ins;
        self.traits[index].signatures = signatures;
    }
    /// Resolves the type `ty` in the signature of a trait with `aliases`.
    pub(super) fn trait_type(&mut self, aliases: &[String], ty: &TypeName) -> Option<Type> {
        // `Self` and the aliases are a different type in each `impl`, so
        // they can't be inside another type, which is always the same.
        if !matches!(ty, TypeName::Named(_))
            && let Some(name) = trait_names(ty, aliases).next()
        {
            self.error(name.span, CheckError::TraitTypeInTuple(name.name.clone()));
            return None;
        }
        self.resolve_type(ty)
    }
    /// Works out the types of an `impl`'s `aliases`, which can use `Self`
    /// and the aliases before them, returning their index in `impl_aliases`
    /// if there are any, or it has a trait, whose index in `traits` is
    /// `trait_index`. While its functions are declared, they can be used.
    ///
    /// The trait's aliases that aren't given are inferred by
    /// `infer_impl_aliases` once its functions are declared, so until then,
    /// they can't be used.
    pub(super) fn define_impl_aliases(
        &mut self,
        aliases: &'a [ImplAlias],
        trait_index: Option<usize>,
    ) -> Option<usize> {
        if aliases.is_empty() && trait_index.is_none() {
            return None;
        }
        let inferred = trait_index
            .map(|index| self.traits[index].aliases)
            .unwrap_or_default()
            .iter()
            .filter(|declared| !aliases.iter().any(|alias| alias.name == **declared))
            .map(|declared| (declared.as_str(), ImplAliasType::Inferring));
        self.impl_aliases.push(inferred.collect());
        let index = self.impl_aliases.len() - 1;
        self.cx.impl_aliases = Some(index);
        for alias in aliases {
            let ty = ImplAliasType::Known(self.resolve_type(&alias.ty));
            if self.impl_aliases[index].insert(&alias.name, ty).is_some() {
                self.error(alias.span, CheckError::Redeclared(alias.name.clone()));
            }
        }
        Some(index)
    }
    /// Infers the types of the aliases of `impl ty with trait_name` that it
    /// doesn't give, at `aliases` in `impl_aliases`. Each is the type that
    /// `functions`, each with its index into `functions`, have where the
    /// trait at `trait_index` in `traits` uses the alias, such as
    /// `alias Rhs = Bool` from `fn and(_ rhs: Bool)` for `fn and(_ rhs: Rhs)`.
    /// The first use decides it, and other uses must match it.
    pub(super) fn infer_impl_aliases(
        &mut self,
        ty: &Name,
        trait_name: &Name,
        trait_index: usize,
        aliases: usize,
        functions: &[(&ImplFunction, &usize)],
    ) {
        let trait_ = &self.traits[trait_index];
        let mut inferred = vec![];
        let mut errors = vec![];
        for (name, stand_in) in trait_.aliases.iter().zip(&trait_.stand_ins[1..]) {
            if !matches!(
                self.impl_aliases[aliases].get(name.as_str()),
                Some(ImplAliasType::Inferring)
            ) {
                continue;
            }
            let is_alias = |expected: &Option<Type>| expected.as_ref() == Some(stand_in);
            let uses_alias =
                |(params, returns): &(Vec<_>, _)| params.iter().any(is_alias) || is_alias(returns);
            // A function that's missing is reported by `check_implements`,
            // unless it has a default implementation, which can't show what
            // the alias is.
            let missing_use =
                trait_
                    .functions
                    .iter()
                    .zip(&trait_.signatures)
                    .any(|(declared, signature)| {
                        declared.body.is_none()
                            && uses_alias(signature)
                            && !functions
                                .iter()
                                .any(|(function, _)| function.function.name == declared.name)
                    });
            // Where the `impl`'s functions have the alias.
            let found = trait_.functions.iter().zip(&trait_.signatures).find_map(
                |(declared, (params, returns))| {
                    let &(_, &index) = functions
                        .iter()
                        .find(|(function, _)| function.function.name == declared.name)?;
                    let signature = &self.functions[index];
                    let found = signature.params.iter().map(|param| &param.ty);
                    params
                        .iter()
                        .zip(found)
                        .chain(iter::once((returns, &signature.returns)))
                        .find(|(expected, _)| is_alias(expected))
                        .map(|(_, found)| found.clone())
                },
            );
            if found.is_none() && !missing_use {
                errors.push((
                    trait_name.span,
                    CheckError::UninferableAlias {
                        ty: ty.to_string(),
                        trait_name: trait_name.to_string(),
                        alias: name.clone(),
                    },
                ));
            }
            inferred.push((name, found.flatten()));
        }
        for (name, ty) in inferred {
            self.impl_aliases[aliases].insert(name, ImplAliasType::Known(ty));
        }
        for (span, error) in errors {
            self.error(span, error);
        }
    }
    /// Declares the default implementations that `impl ty with` the trait at
    /// `trait_index` in `traits` uses, since it doesn't implement them in
    /// `functions`, returning their bodies to check, with the trait's file.
    /// Names mean what they do where they're written, so they're checked in
    /// that file, but `Self`, which is `self_ty`, and the aliases, at
    /// `aliases` in `impl_aliases`, are the `impl`'s, and they can be used like
    /// its own functions.
    pub(super) fn declare_defaults(
        &mut self,
        type_name: &str,
        self_ty: Option<&Type>,
        trait_index: usize,
        functions: &'a [ImplFunction],
        aliases: Option<usize>,
    ) -> Vec<(usize, Pending<'a>)> {
        let is_implemented = |declared: &TraitFunction| {
            functions
                .iter()
                .any(|implemented| implemented.function.name == declared.name)
        };
        let declared = self.traits[trait_index].functions;
        // A missing function is reported, so the defaults that might call it
        // aren't checked.
        let complete = declared
            .iter()
            .all(|declared| declared.body.is_some() || is_implemented(declared));

        let trait_file = self.traits[trait_index].file;
        let mut pending = vec![];
        for declared in declared.iter().filter(|declared| !is_implemented(declared)) {
            let Some(body) = &declared.body else {
                continue;
            };
            let name = format!("{type_name}.{}", declared.name);
            // Named after the `impl`, since each one has its own copy.
            let full_name = self.full_name(&name);
            // Declared as written in the trait, in its file, but as part of
            // the `impl`.
            let index = self.at_top_level(trait_file, None, |checker| {
                checker.in_impl(self_ty.cloned(), aliases, |checker| {
                    checker.declare_method(
                        self_ty,
                        (&declared.name, &declared.params, &declared.ret),
                        declared.span,
                        declared.is_static,
                        full_name,
                    )
                })
            });
            let kind = if declared.is_static {
                FnKind::Static(self_ty.cloned())
            } else {
                FnKind::Method(self_ty.cloned())
            };
            if complete {
                let body = Pending::Fn {
                    index,
                    name: &declared.name,
                    span: declared.span,
                    body,
                    kind,
                    aliases,
                };
                pending.push((trait_file, body));
            }
        }
        pending
    }
    /// Finds the trait called `name`, returning its index into `traits`.
    pub(super) fn resolve_trait(&mut self, name: &Name) -> Option<usize> {
        match self.lookup(name) {
            Ok(Binding::Trait(index)) => Some(index),
            Ok(_) | Err(LookupError::Captured) => {
                self.error(name.span, CheckError::NotATrait(name.to_string()));
                None
            }
            Err(LookupError::Undeclared) => {
                self.error(name.span, CheckError::UnknownTrait(name.to_string()));
                None
            }
            Err(error) => {
                self.lookup_error(name, error);
                None
            }
        }
    }
    /// Checks that `functions`, from `impl ty with trait_name`, each with its
    /// index into `functions`, are exactly the functions of the trait at
    /// `index` in `traits`, which `self_ty` implements. Labels and types must
    /// match, but parameter names don't need to, since callers never see
    /// them.
    pub(super) fn check_implements(
        &mut self,
        implementation: Implementation,
        functions: Vec<(&ImplFunction, usize)>,
    ) {
        let Implementation {
            ty,
            self_ty,
            trait_name,
            index,
            aliases,
            alias_types,
        } = implementation;
        let trait_ = &self.traits[index];
        // What `Self` and each alias stand for in this `impl`, or `None` for
        // an alias that couldn't be worked out.
        let alias_type = |alias: &str| match self.impl_aliases[alias_types?].get(alias)? {
            ImplAliasType::Known(ty) => ty.as_ref(),
            ImplAliasType::Inferring => None,
        };
        let stands_for: HashMap<_, _> = iter::once(Some(self_ty))
            .chain(trait_.aliases.iter().map(|alias| alias_type(alias)))
            .zip(&trait_.stand_ins)
            .map(|(ty, stand_in)| (stand_in.id, ty))
            .collect();
        let matches = |found: &Option<Type>, expected: &Option<Type>| match (found, expected) {
            (Some(found), Some(expected)) => match stands_for.get(&expected.id) {
                Some(stands_for) => stands_for.is_none_or(|expected| found == expected),
                None => found == expected,
            },
            // An unknown type has already been reported.
            _ => true,
        };

        let mut errors = vec![];
        for alias in aliases {
            if !trait_.aliases.contains(&alias.name) {
                errors.push((
                    alias.span,
                    CheckError::NotInTrait {
                        trait_name: trait_name.to_string(),
                        member: alias.name.clone(),
                    },
                ));
            }
        }
        for &(
            ImplFunction {
                is_static,
                function,
                ..
            },
            implemented,
        ) in &functions
        {
            let Some(position) = trait_
                .functions
                .iter()
                .position(|declared| declared.name == function.name)
            else {
                errors.push((
                    function.span,
                    CheckError::NotInTrait {
                        trait_name: trait_name.to_string(),
                        member: function.name.clone(),
                    },
                ));
                continue;
            };
            let declared = &trait_.functions[position];
            let (params, returns) = &trait_.signatures[position];
            let signature = &self.functions[implemented];
            let same = *is_static == declared.is_static
                && signature.params.len() == params.len()
                && signature
                    .params
                    .iter()
                    .zip(&declared.params)
                    .zip(params)
                    .all(|((found, declared), expected)| {
                        found.label == declared.label.as_deref() && matches(&found.ty, expected)
                    })
                && matches(&signature.returns, returns);
            if !same {
                errors.push((
                    function.span,
                    CheckError::TraitFnMismatch {
                        trait_name: trait_name.to_string(),
                        expected: signature_text(declared),
                    },
                ));
            }
        }
        // Functions with a default implementation don't need implementing.
        for declared in trait_
            .functions
            .iter()
            .filter(|declared| declared.body.is_none())
        {
            if !functions
                .iter()
                .any(|(implemented, _)| implemented.function.name == declared.name)
            {
                errors.push((
                    trait_name.span,
                    CheckError::MissingTraitMember {
                        ty: ty.to_string(),
                        trait_name: trait_name.to_string(),
                        expected: signature_text(declared),
                    },
                ));
            }
        }
        for (span, error) in errors {
            self.error(span, error);
        }
    }
}
