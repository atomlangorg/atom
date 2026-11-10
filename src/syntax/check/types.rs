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

//! Finding and defining types, including the types made from generic types.

use super::*;

/// The smallest and largest values of an integer type `bits` wide.
pub(super) fn int_range(bits: u32, signed: bool) -> (i128, i128) {
    if signed {
        (-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
    } else {
        (0, (1 << bits) - 1)
    }
}

impl<'a> Checker<'a> {
    /// Finds the type `ty`, reporting an error if it can't be found.
    pub(super) fn resolve_type(&mut self, ty: &TypeName) -> Option<Type> {
        match ty {
            TypeName::Named(name) => self.resolve_named_type(name),
            TypeName::Generic { name, args } => self.generic_type(name, args),
            TypeName::SelfParam(param, span) => self.self_param(param, *span),
            TypeName::Tuple(elements) => self.resolve_tuple(elements),
        }
    }
    /// Finds the tuple type with `elements`, whose elements are read by the
    /// names they're declared with, reporting an error if any of their types
    /// can't be found.
    pub(super) fn resolve_tuple(&mut self, elements: &[ast::TupleElement]) -> Option<Type> {
        // Every element is resolved, so each one's errors are found.
        let types: Vec<_> = elements
            .iter()
            .map(|element| self.resolve_type(&element.ty))
            .collect();
        let contract = elements
            .iter()
            .zip(types)
            .map(|(element, ty)| {
                Some(TupleElementType {
                    label: element.label.clone(),
                    ty: ty?,
                })
            })
            .collect::<Option<_>>()?;
        let names = elements.iter().map(|element| element.name.clone());
        Some(self.tuple_type(contract, Some(names.collect())))
    }
    /// The tuple type with `elements`, which is created the first time it's
    /// needed, read by `names`, or by its labels if they're `None`.
    pub(super) fn tuple_type(
        &mut self,
        elements: Vec<TupleElementType>,
        names: Option<Vec<Option<String>>>,
    ) -> Type {
        let labels: Vec<_> = elements
            .iter()
            .map(|element| element.label.clone())
            .collect();
        let names = names.unwrap_or_else(|| labels.clone());
        let show = |names: &[Option<String>]| {
            let shown: Vec<_> = elements
                .iter()
                .zip(names)
                .map(|(element, name)| {
                    let element = ast::TupleElement {
                        label: element.label.clone(),
                        name: name.clone(),
                        ty: TypeName::new(&element.ty.name),
                        span: Span::default(),
                    };
                    element.to_string()
                })
                .collect();
            format!("({})", shown.join(", "))
        };
        let name = show(&names);
        let key: Vec<_> = elements
            .iter()
            .map(|element| (element.label.clone(), element.ty.id))
            .collect();
        let types = elements.iter().map(|element| element.ty.clone()).collect();
        let id = match self.tuple_ids.get(&key) {
            Some(&id) => id,
            None => {
                let id = self.add_type(show(&labels), self.cx.file, TypeKind::Tuple(elements));
                self.tuple_ids.insert(key, id);
                id
            }
        };
        Type {
            id,
            name,
            parts: Some(Rc::new(Parts { types, names })),
        }
    }
    /// The names that the elements of the tuple type `ty` are read by.
    pub(super) fn element_names(&self, ty: &Type) -> Vec<Option<String>> {
        match &ty.parts {
            Some(parts) => parts.names.clone(),
            None => self
                .elements(ty.id)
                .unwrap_or_default()
                .iter()
                .map(|element| element.label.clone())
                .collect(),
        }
    }
    /// The types of the elements of the tuple type `ty`, as it was found.
    pub(super) fn element_types(&self, ty: &Type) -> Vec<Type> {
        match &ty.parts {
            Some(parts) => parts.types.clone(),
            None => self
                .elements(ty.id)
                .unwrap_or_default()
                .iter()
                .map(|element| element.ty.clone())
                .collect(),
        }
    }
    /// The generic type that `ty` is made from, as an index into
    /// `templates`, and its arguments, as it was found.
    pub(super) fn found_args(&self, ty: &Type) -> Option<(usize, Vec<Type>)> {
        let (template, args) = self.types[ty.id].instance.as_ref()?;
        let args = match &ty.parts {
            Some(parts) => parts.types.clone(),
            None => args.clone(),
        };
        Some((*template, args))
    }
    /// The type of the field at `index` of `ty`, as it was found, for the
    /// code at `span`. A type made from a generic type has the fields of
    /// that type, with its arguments as they were found.
    pub(super) fn field_type(&mut self, ty: &Type, index: usize, span: Span) -> Option<Type> {
        if self.elements(ty.id).is_some() {
            return self.element_types(ty).get(index).cloned();
        }
        let field = |checker: &Self, id: usize| checker.fields(id).get(index)?.ty.clone();
        self.as_found(ty, field, span)
    }
    /// The type of the value of the option at `index` of `ty`, as it was
    /// found, like `field_type`.
    pub(super) fn option_type(&mut self, ty: &Type, index: usize, span: Span) -> Option<Type> {
        let option = |checker: &Self, id: usize| checker.options(id)?.get(index)?.ty.clone();
        self.as_found(ty, option, span)
    }
    /// The part of `ty` that `part` finds in a type by its `id`, as `ty` was
    /// found, for the code at `span`. For a type made from a generic type,
    /// it's found in that type, with `ty`'s arguments in place of its
    /// parameters.
    pub(super) fn as_found(
        &mut self,
        ty: &Type,
        part: impl Fn(&Self, usize) -> Option<Type>,
        span: Span,
    ) -> Option<Type> {
        let generic = self.found_args(ty).and_then(|(template, args)| {
            let generic = self.templates[template].generic.as_ref()?;
            Some((generic.id, self.params(template, &args)))
        });
        match generic {
            Some((generic, params)) if ty.parts.is_some() => {
                let found = part(self, generic)?;
                self.substitute(&found, &params, span)
            }
            _ => part(self, ty.id),
        }
    }
    /// The type made from the generic type called `name` with `args`.
    pub(super) fn generic_type(&mut self, name: &Name, args: &[TypeName]) -> Option<Type> {
        let template = match self.lookup(name) {
            Ok(Binding::Generic(template)) => template,
            Ok(Binding::Type(_)) => {
                self.error(name.span, CheckError::NotGeneric(name.to_string()));
                return None;
            }
            _ => {
                self.resolve_named_type(name);
                return None;
            }
        };
        let expected = self.templates[template].params.len();
        if args.len() != expected {
            self.error(
                name.span,
                CheckError::WrongGenericCount {
                    ty: name.to_string(),
                    expected,
                    found: args.len(),
                },
            );
            return None;
        }
        // Every argument is resolved, so each one's errors are found.
        let args: Vec<_> = args.iter().map(|arg| self.resolve_type(arg)).collect();
        let args = args.into_iter().collect::<Option<Vec<_>>>()?;
        self.named_instance(template, args, name)
    }
    /// The type made from the generic type called `name`, at `template` in
    /// `templates`, with `args`.
    pub(super) fn named_instance(
        &mut self,
        template: usize,
        args: Vec<Type>,
        name: &Name,
    ) -> Option<Type> {
        if self.is_only_for_choices(template) && self.options(args[0].id).is_none() {
            let error = CheckError::NotAChoice {
                generic: name.to_string(),
                found: args[0].clone(),
            };
            self.error(name.span, error);
            return None;
        }
        self.instance(template, args, name.span)
    }
    /// Whether `ty` is made from the generic type at `template` in
    /// `templates`.
    pub(super) fn is_made_from(&self, ty: &Type, template: usize) -> bool {
        self.types[ty.id]
            .instance
            .as_ref()
            .is_some_and(|(found, _)| *found == template)
    }
    /// Whether the generic type at `template` in `templates` has values only
    /// when its first argument is a choice.
    pub(super) fn is_only_for_choices(&self, template: usize) -> bool {
        template == self.std_template("ChoiceTag") || template == self.std_template("ChoiceOption")
    }
    /// What `Self.param` stands for in the `impl` of a type made from a
    /// generic type.
    pub(super) fn self_param(&mut self, param: &str, span: Span) -> Option<Type> {
        let impl_type = match &self.cx.impl_type {
            Some(Some(ty)) => ty.clone(),
            // The `impl`'s type is unknown, which has been reported.
            Some(None) => return None,
            None => {
                self.error(span, CheckError::SelfParamOutsideGeneric(param.to_string()));
                return None;
            }
        };
        let Some((template, args)) = self.found_args(&impl_type) else {
            self.error(span, CheckError::SelfParamOutsideGeneric(param.to_string()));
            return None;
        };
        match self.templates[template]
            .params
            .iter()
            .position(|found| found == param)
        {
            Some(index) => Some(args[index].clone()),
            None => {
                let error = CheckError::UnknownSelfParam {
                    ty: impl_type,
                    param: param.to_string(),
                };
                self.error(span, error);
                None
            }
        }
    }
    /// The type `type_name` refers to where a value of one of `contexts` is
    /// expected or matched. A generic type's name without arguments refers to
    /// the first of `contexts` that's made from it, since that's what its
    /// arguments must be.
    pub(super) fn type_in_context(
        &mut self,
        type_name: &TypeName,
        contexts: &[Option<&Type>],
    ) -> Option<Type> {
        if let TypeName::Named(name) = type_name
            && let Ok(Binding::Generic(template)) = self.lookup(name)
        {
            let found = contexts
                .iter()
                .flatten()
                .find(|ty| self.is_made_from(ty, template));
            if found.is_none() {
                self.error(name.span, CheckError::CannotInferGeneric(name.to_string()));
            }
            return found.map(|&ty| ty.clone());
        }
        self.resolve_type(type_name)
    }
    /// Declares the generic group, choice or primitive called `name`, with
    /// `params`, which `stmt` declares.
    pub(super) fn declare_template(&mut self, name: &'a str, params: &'a [String], stmt: &'a Stmt) {
        let mut seen = HashSet::new();
        for param in params {
            if !seen.insert(param) {
                self.error(stmt.span, CheckError::Redeclared(param.clone()));
            }
        }
        self.templates.push(Template {
            name,
            file: self.cx.file,
            params,
            stmt,
            stand_ins: vec![],
            generic: None,
        });
        self.declare(name, stmt.span, Binding::Generic(self.templates.len() - 1));
    }
    /// Defines the generic type at `template` in `templates` once, as the type
    /// made from it with a stand-in for each parameter, which can't be used as
    /// anything else. Returns that type, with the file it's in and where it's
    /// declared.
    pub(super) fn check_template(&mut self, template: usize) -> Option<(usize, Type, Span)> {
        let Template {
            file, params, stmt, ..
        } = self.templates[template];
        // Its first parameter stands for a choice if its arguments must be.
        let only_for_choices = self.is_only_for_choices(template);
        let stand_ins = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let kind = if only_for_choices && index == 0 {
                    TypeKind::Choice(vec![])
                } else {
                    TypeKind::Group(vec![])
                };
                Type {
                    id: self.add_type(param.clone(), file, kind),
                    name: param.clone(),
                    parts: None,
                }
            })
            .collect::<Vec<_>>();
        self.templates[template].stand_ins = stand_ins.clone();
        let name = self.instance_name(template, &stand_ins);
        let id = self.new_instance(template, stand_ins.clone(), &name);
        self.at_top_level(file, Some((template, stand_ins)), |checker| {
            match &stmt.kind {
                StmtKind::Group { fields, .. } => checker.define_group(id, &name, fields),
                StmtKind::Choice { options, .. } => checker.define_choice(id, &name, options),
                StmtKind::Primitive { name, .. } => checker.define_primitive(id, name, stmt.span),
                _ => unreachable!("only groups, choices and primitives are generic"),
            }
        });
        let ty = Type {
            id,
            name,
            parts: None,
        };
        self.templates[template].generic = Some(ty.clone());
        self.fill_instances();
        Some((file, ty, stmt.span))
    }
    /// The type made from the generic type at `template` in `templates` with
    /// `args`, which is made the first time it's needed, for the code at
    /// `span`.
    pub(super) fn instance(
        &mut self,
        template: usize,
        args: Vec<Type>,
        span: Span,
    ) -> Option<Type> {
        // Arguments nested deeper than this almost certainly mean a type uses
        // itself with ever larger arguments, which would never end.
        const MAX_DEPTH: usize = 32;

        let name = self.instance_name(template, &args);
        let key = (template, args.iter().map(|arg| arg.id).collect());
        // It keeps its arguments as they were found.
        let parts = Some(Rc::new(Parts {
            types: args.clone(),
            names: vec![],
        }));
        if let Some(&id) = self.instance_ids.get(&key) {
            return Some(Type { id, name, parts });
        }
        let depth = 1 + args
            .iter()
            .map(|arg| self.type_depth(arg.id))
            .max()
            .unwrap_or(0);
        if depth > MAX_DEPTH {
            // The types around it are too deep for the same reason, so it's
            // only reported once.
            if !self
                .errors
                .iter()
                .any(|error| matches!(error.error, CheckError::GenericTooDeep(_)))
            {
                self.error(span, CheckError::GenericTooDeep(name));
            }
            return None;
        }
        let id = self.new_instance(template, args, &name);
        self.unfilled.push(id);
        self.fill_instances();
        Some(Type { id, name, parts })
    }
    /// How the type made from the generic type at `template` in `templates`
    /// with `args` is shown, as in `Optional<Int>`.
    pub(super) fn instance_name(&self, template: usize, args: &[Type]) -> String {
        let shown: Vec<_> = args.iter().map(|arg| arg.name.as_str()).collect();
        format!("{}<{}>", self.templates[template].name, shown.join(", "))
    }
    /// Adds the type called `name` made from the generic type at `template`
    /// in `templates` with `args`, without its fields or options, returning
    /// its `id`.
    pub(super) fn new_instance(&mut self, template: usize, args: Vec<Type>, name: &str) -> usize {
        let Template { file, stmt, .. } = self.templates[template];
        let kind = match stmt.kind {
            StmtKind::Choice { .. } => TypeKind::Choice(vec![]),
            _ => TypeKind::Group(vec![]),
        };
        let full_name = format!("{}::{name}", self.files[file].display);
        let id = self.add_type(full_name, file, kind);
        let key = (template, args.iter().map(|arg| arg.id).collect());
        self.instance_ids.insert(key, id);
        self.types[id].instance = Some((template, args));
        id
    }
    /// Gives each of `unfilled` the fields or options of the generic type
    /// it's made from, once that's been defined, with its arguments in place
    /// of the generic type's parameters.
    pub(super) fn fill_instances(&mut self) {
        // Filling one can make others, which this fills too.
        if std::mem::replace(&mut self.filling, true) {
            return;
        }
        while let Some(position) = self.unfilled.iter().position(|&id| {
            let (template, _) = self.types[id]
                .instance
                .as_ref()
                .expect("made from a template");
            self.templates[*template].generic.is_some()
        }) {
            let id = self.unfilled.remove(position);
            let (template, args) = self.types[id]
                .instance
                .clone()
                .expect("made from a template");
            let Template {
                file,
                stmt,
                ref generic,
                ..
            } = self.templates[template];
            let generic = generic.as_ref().expect("found above").id;
            let params = self.params(template, &args);
            // A generic type nested too deeply is found here, so it's
            // reported where its generic type is declared.
            let kind = self.types[generic].kind.clone();
            let kind = self.at_top_level(file, None, |checker| {
                let mut substitute = |ty: Option<Type>| {
                    ty.and_then(|ty| checker.substitute(&ty, &params, stmt.span))
                };
                match kind {
                    TypeKind::Group(fields) => TypeKind::Group(
                        fields
                            .into_iter()
                            .map(|field| FieldSignature {
                                ty: substitute(field.ty),
                                ..field
                            })
                            .collect(),
                    ),
                    TypeKind::Choice(options) => TypeKind::Choice(
                        options
                            .into_iter()
                            .map(|option| OptionSignature {
                                ty: substitute(option.ty),
                                ..option
                            })
                            .collect(),
                    ),
                    TypeKind::Primitive(primitive) => TypeKind::Primitive(primitive),
                    TypeKind::Tuple(_) => unreachable!("tuples aren't generic"),
                }
            });
            self.types[id].kind = kind;
        }
        self.filling = false;
    }
    /// What each stand-in for a parameter of the generic type at `template`
    /// in `templates` stands for, by `id`, where `args` are its arguments.
    pub(super) fn params(&self, template: usize, args: &[Type]) -> HashMap<usize, Type> {
        let stand_ins = &self.templates[template].stand_ins;
        stand_ins
            .iter()
            .map(|stand_in| stand_in.id)
            .zip(args.iter().cloned())
            .collect()
    }
    /// `ty`, with what `params` says stand-ins stand for in place of them, as
    /// `Checker::params` gives. Returns `None` if that would make a generic
    /// type nested too deeply, which is reported at `span`.
    pub(super) fn substitute(
        &mut self,
        ty: &Type,
        params: &HashMap<usize, Type>,
        span: Span,
    ) -> Option<Type> {
        if let Some(arg) = params.get(&ty.id) {
            return Some(arg.clone());
        }
        if let Some((template, args)) = self.found_args(ty) {
            let args = args
                .iter()
                .map(|arg| self.substitute(arg, params, span))
                .collect::<Option<_>>()?;
            return self.instance(template, args, span);
        }
        if let Some(elements) = self.elements(ty.id) {
            let elements = elements.to_vec();
            let types = self.element_types(ty);
            let elements = elements
                .into_iter()
                .zip(types)
                .map(|(element, ty)| {
                    Some(TupleElementType {
                        ty: self.substitute(&ty, params, span)?,
                        ..element
                    })
                })
                .collect::<Option<_>>()?;
            // A tuple that's unchanged keeps the name it's used by, such as
            // `Void` for `()`, and it's always read by the same names.
            let substituted = self.tuple_type(elements, Some(self.element_names(ty)));
            return Some(if substituted == *ty {
                ty.clone()
            } else {
                substituted
            });
        }
        Some(ty.clone())
    }
    /// How deeply the type with `id` nests types made from generic types and
    /// tuples, which is 0 for other types.
    pub(super) fn type_depth(&self, id: usize) -> usize {
        let parts: Vec<_> = match (&self.types[id].instance, self.elements(id)) {
            (Some((_, args)), _) => args.iter().map(|arg| arg.id).collect(),
            (None, Some(elements)) => elements.iter().map(|element| element.ty.id).collect(),
            (None, None) => return 0,
        };
        1 + parts
            .into_iter()
            .map(|part| self.type_depth(part))
            .max()
            .unwrap_or(0)
    }
    /// Finds the type called `name`, reporting an error if it can't be found.
    pub(super) fn resolve_named_type(&mut self, name: &Name) -> Option<Type> {
        match self.lookup(name) {
            Ok(Binding::Type(ty)) => Some(ty),
            Ok(Binding::Generic(_)) => {
                self.error(name.span, CheckError::GenericArgsNeeded(name.to_string()));
                None
            }
            // A value from outside the function is still not a type.
            Ok(_) | Err(LookupError::Captured) => {
                self.error(name.span, CheckError::NotAType(name.to_string()));
                None
            }
            Err(LookupError::Undeclared) => {
                self.error(name.span, CheckError::UnknownType(name.to_string()));
                None
            }
            Err(error) => {
                self.lookup_error(name, error);
                None
            }
        }
    }
    /// The type that the alias at `index` in `aliases` is an alias of, working
    /// it out if that hasn't been done yet. Returns `None` if it's unknown.
    pub(super) fn alias_type(&mut self, index: usize) -> Option<Type> {
        let alias = &self.aliases[index];
        match &alias.state {
            LazyType::Checked(ty) => return ty.clone(),
            LazyType::Checking => {
                let error = CheckError::CyclicAlias(alias.name.to_string());
                self.error_in(alias.file, alias.span, error);
                return None;
            }
            LazyType::Unchecked => {}
        }

        // Worked out at the top level of its own file, even when another file
        // needs it first.
        let (file, target) = (alias.file, alias.target);
        self.aliases[index].state = LazyType::Checking;
        let ty = self.at_top_level(file, None, |checker| checker.resolve_type(target));

        self.aliases[index].state = LazyType::Checked(ty.clone());
        ty
    }
    /// Declares an alias called `name` of the type called `target`, whose
    /// type has been worked out if `state` says so.
    pub(super) fn declare_alias(
        &mut self,
        name: &'a str,
        span: Span,
        target: &'a TypeName,
        state: LazyType,
    ) {
        self.aliases.push(Alias {
            file: self.cx.file,
            name,
            span,
            target,
            state,
        });
        self.declare(name, span, Binding::Alias(self.aliases.len() - 1));
    }
    /// Adds a type with the full name `name`, declared in the file at `file`
    /// in `files`, returning its `id`.
    pub(super) fn add_type(&mut self, name: String, file: usize, kind: TypeKind<'a>) -> usize {
        self.types.push(TypeInfo {
            name,
            file,
            kind,
            methods: HashMap::new(),
            statics: HashMap::new(),
            instance: None,
        });
        self.types.len() - 1
    }
    /// The fields of the type with `id`, in the order they're declared. Only
    /// groups have any.
    pub(super) fn fields(&self, id: usize) -> &[FieldSignature<'a>] {
        match &self.types[id].kind {
            TypeKind::Group(fields) => fields,
            _ => &[],
        }
    }
    /// The options of the type with `id` in the order they're declared, if
    /// it's a choice.
    pub(super) fn options(&self, id: usize) -> Option<&[OptionSignature<'a>]> {
        match &self.types[id].kind {
            TypeKind::Choice(options) => Some(options),
            _ => None,
        }
    }
    /// The elements of the type with `id`, if it's a tuple.
    pub(super) fn elements(&self, id: usize) -> Option<&[TupleElementType]> {
        match &self.types[id].kind {
            TypeKind::Tuple(elements) => Some(elements),
            _ => None,
        }
    }
    /// How the compiler represents the type with `id`, if it's a primitive.
    pub(super) fn primitive(&self, id: usize) -> Option<ir::Primitive> {
        match self.types[id].kind {
            TypeKind::Primitive(primitive) => Some(primitive),
            _ => None,
        }
    }
    /// Declares the type called `name` that `stmt` declares, without its
    /// fields or options, returning its `id`. A choice can be used as one
    /// before its options are defined.
    pub(super) fn declare_type(&mut self, name: &'a str, stmt: &Stmt) -> usize {
        let span = stmt.span;
        let kind = match stmt.kind {
            StmtKind::Choice { .. } => TypeKind::Choice(vec![]),
            _ => TypeKind::Group(vec![]),
        };
        let id = self.add_type(self.full_name(name), self.cx.file, kind);
        let ty = Type {
            id,
            name: name.to_string(),
            parts: None,
        };
        self.declare(name, span, Binding::Type(ty));
        id
    }
    /// Resolves the fields or options of `stmt`, which declares the type with
    /// `id`.
    pub(super) fn define_type(&mut self, id: usize, stmt: &'a Stmt) {
        match &stmt.kind {
            StmtKind::Group { name, fields, .. } => self.define_group(id, name, fields),
            StmtKind::Choice { name, options, .. } => self.define_choice(id, name, options),
            StmtKind::Primitive { name, .. } => self.define_primitive(id, name, stmt.span),
            _ => unreachable!("only groups, choices and primitives declare types"),
        }
    }
    /// Records the primitive type called `name` with `id`, which must be
    /// declared in the standard library.
    pub(super) fn define_primitive(&mut self, id: usize, name: &str, span: Span) {
        if self.files[self.cx.file].package != Some(STDLIB_PACKAGE) {
            self.error(span, CheckError::PrimitiveOutsideStdlib(name.to_string()));
            return;
        }
        let int = |bits, signed| ir::Primitive::Int { bits, signed };
        let primitive = match name {
            "IntU8" => int(8, false),
            "IntU16" => int(16, false),
            "IntU32" => int(32, false),
            "IntU64" => int(64, false),
            "IntS8" => int(8, true),
            "IntS16" => int(16, true),
            "IntS32" => int(32, true),
            "IntS64" => int(64, true),
            "Bool" => ir::Primitive::Bool,
            "Fn" => ir::Primitive::Fn,
            "ChoiceTag" | "ChoiceOption" => ir::Primitive::Tag,
            _ => panic!("the compiler doesn't know how to represent `{name}`"),
        };
        self.types[id].kind = TypeKind::Primitive(primitive);
    }
    /// Resolves the options of the choice called `name` with `id`.
    pub(super) fn define_choice(&mut self, id: usize, name: &str, options: &'a [ChoiceOption]) {
        // Its options can use it as a choice.
        self.types[id].kind = TypeKind::Choice(vec![]);
        // Option names only need to be unique within their choice.
        let mut option_names = HashSet::new();
        let mut signatures = vec![];
        for option in options {
            if !option_names.insert(&option.name) {
                self.error(
                    option.span,
                    CheckError::MemberRedeclared {
                        ty: Type {
                            id,
                            name: name.to_string(),
                            parts: None,
                        },
                        member: option.name.clone(),
                        existing: MemberKind::Option,
                    },
                );
            }
            signatures.push(OptionSignature {
                decl: option,
                ty: self.resolve_tuple(&option.values),
            });
        }
        self.types[id].kind = TypeKind::Choice(signatures);
    }
    /// Resolves the fields of the group called `name` with `id`.
    pub(super) fn define_group(&mut self, id: usize, name: &str, fields: &'a [Field]) {
        // Field names only need to be unique within their group.
        let mut field_names = HashSet::new();
        for field in fields {
            if !field_names.insert(&field.name) {
                self.error(
                    field.span,
                    CheckError::MemberRedeclared {
                        ty: Type {
                            id,
                            name: name.to_string(),
                            parts: None,
                        },
                        member: field.name.clone(),
                        existing: MemberKind::Field,
                    },
                );
            }
            let ty = self.resolve_type(&field.ty);
            if let TypeKind::Group(fields) = &mut self.types[id].kind {
                fields.push(FieldSignature {
                    name: &field.name,
                    ty,
                });
            }
        }
    }
    /// The `id`s of the types that the type with `id` holds directly: a
    /// group's fields, the values of a choice's options, or a tuple's
    /// elements.
    pub(super) fn contained_types(&self, id: usize) -> Vec<usize> {
        match &self.types[id].kind {
            TypeKind::Group(fields) => fields
                .iter()
                .filter_map(|field| field.ty.as_ref())
                .collect(),
            TypeKind::Choice(options) => options
                .iter()
                .filter_map(|option| option.ty.as_ref())
                .collect(),
            TypeKind::Tuple(elements) => elements.iter().map(|element| &element.ty).collect(),
            TypeKind::Primitive(_) => vec![],
        }
        .into_iter()
        .map(|ty| ty.id)
        .collect()
    }
    /// Reports an error if the group or choice with `id` contains itself,
    /// which would make it infinitely large.
    pub(super) fn check_not_recursive(&mut self, id: usize, name: &str, span: Span) {
        let mut seen = HashSet::new();
        let mut unvisited = self.contained_types(id);
        while let Some(next) = unvisited.pop() {
            if next == id {
                self.error(span, CheckError::RecursiveType(name.to_string()));
                return;
            }
            if seen.insert(next) {
                unvisited.extend(self.contained_types(next));
            }
        }
    }
    /// The width and signedness of `ty`, if it's an integer type.
    pub(super) fn int_type(&self, ty: &Type) -> Option<(u32, bool)> {
        match self.primitive(ty.id) {
            Some(ir::Primitive::Int { bits, signed }) => Some((bits, signed)),
            _ => None,
        }
    }
    /// The type of an integer that's used as `expected`, which is `expected`
    /// itself if it's an integer type, and `Int` otherwise.
    pub(super) fn int_type_for(&self, expected: Option<&Type>) -> Type {
        expected
            .filter(|ty| self.int_type(ty).is_some())
            .cloned()
            .unwrap_or_else(|| self.std_type("Int"))
    }
    /// Works out which types `stand_ins` stand for from a value of type
    /// `found` being given where `param`, which can use them, is expected,
    /// adding them to `inferred`. The first type found for one is kept, so
    /// any others are reported as mismatches.
    pub(super) fn infer(
        &self,
        param: &Type,
        found: &Type,
        stand_ins: &[Type],
        inferred: &mut [Option<Type>],
    ) {
        if let Some(index) = stand_ins.iter().position(|stand_in| stand_in == param) {
            inferred[index].get_or_insert_with(|| found.clone());
            return;
        }
        let parts: Vec<(Type, Type)> = match (self.found_args(param), self.found_args(found)) {
            (Some((expected, params)), Some((template, args))) if expected == template => {
                params.into_iter().zip(args).collect()
            }
            _ if self.elements(param.id).is_some() && self.elements(found.id).is_some() => self
                .element_types(param)
                .into_iter()
                .zip(self.element_types(found))
                .collect(),
            _ => vec![],
        };
        for (param, found) in parts {
            self.infer(&param, &found, stand_ins, inferred);
        }
    }
}
