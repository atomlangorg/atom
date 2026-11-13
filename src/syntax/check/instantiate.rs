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

//! Making a function for each set of arguments that each generic type's functions are used with, and the checked program.

use super::*;

/// What a program that has been checked is made of, once `Checker::instantiate`
/// has made a function for each set of arguments that each function of a
/// generic type is used with.
pub(super) struct Instantiated {
    pub(super) functions: Vec<ir::Function>,
    /// What each top-level value's initialiser compiles to.
    pub(super) globals: Vec<ir::Expr>,
    /// Calls `fn main() -> Int` from `/main`, if the project declares one.
    pub(super) main: Option<ir::Expr>,
}

/// The functions `Checker::instantiate` has made and is still to make.
#[derive(Default)]
pub(super) struct Instantiating {
    /// The index of each function it has made or will make.
    pub(super) made: HashMap<FnRef, usize>,
    /// The functions it's still to make, with their indexes, and the file and
    /// place of the code that first used them.
    pub(super) queue: Vec<(FnRef, usize, (usize, Span))>,
    /// The file and place of the code that first used the function being made,
    /// where a generic type nested too deeply because of it is reported.
    pub(super) site: (usize, Span),
}

impl Instantiating {
    /// The index of `function`, which is made if it hasn't been yet, for the
    /// code at `site`.
    fn index(&mut self, function: FnRef, site: (usize, Span)) -> usize {
        let count = self.made.len();
        match self.made.entry(function) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                self.queue.push((entry.key().clone(), count, site));
                *entry.insert(count)
            }
        }
    }
}

impl<'a> Checker<'a> {
    /// The program made of everything that's been checked, which must not have
    /// any errors.
    pub(super) fn program(self, instantiated: Instantiated) -> ir::Program {
        let Instantiated {
            functions,
            globals,
            main,
        } = instantiated;
        let type_id = |ty: &Option<Type>| ty.as_ref().expect(NO_ERRORS).id;
        let types = self
            .types
            .iter()
            .map(|info| {
                let name = info.name.clone();
                match &info.kind {
                    TypeKind::Primitive(primitive) => ir::TypeDef::Primitive(*primitive),
                    // A tuple is laid out like a group of its elements.
                    TypeKind::Tuple(elements) => ir::TypeDef::Group {
                        name,
                        fields: elements.iter().map(|element| element.ty.id).collect(),
                    },
                    TypeKind::Choice(options) => ir::TypeDef::Choice {
                        name,
                        options: options.iter().map(|option| type_id(&option.ty)).collect(),
                    },
                    TypeKind::Group(fields) => ir::TypeDef::Group {
                        name,
                        fields: fields.iter().map(|field| type_id(&field.ty)).collect(),
                    },
                }
            })
            .collect();
        let globals = self
            .globals
            .into_iter()
            .zip(globals)
            .map(|(global, value)| ir::Global {
                name: format!("{}::{}", self.files[global.file].display, global.name),
                ty: match global.state {
                    LazyType::Checked(Some(ty)) => ty.id,
                    _ => panic!("{NO_ERRORS}"),
                },
                value,
            })
            .collect();

        ir::Program {
            types,
            main,
            functions,
            globals,
            global_order: self.global_order,
        }
    }
    /// The function `fn main() -> Int` from `/main`, as an index into
    /// `functions`, or `None` if there isn't one.
    pub(super) fn main(&self) -> Option<usize> {
        let file = self
            .files
            .iter()
            .find(|file| file.package.is_none() && file.path == ["main"])?;
        let Some(&Binding::Function(index)) = file.scope.get("main") else {
            return None;
        };
        let signature = &self.functions[index];
        if !signature.params.is_empty() || signature.returns != Some(self.std_type("Int")) {
            return None;
        }
        Some(index)
    }
    /// Makes the functions the program uses, which are those that aren't for
    /// a generic type, then the ones they use, each once for every set of
    /// arguments its generic type is used with. Returns them, along with what
    /// top-level values and the call to `main` compile to, which use them.
    /// Returns `None` if that makes a generic type nested too deeply, which
    /// has been reported.
    pub(super) fn instantiate(&mut self) -> Option<Instantiated> {
        let mut state = Instantiating::default();
        let root = (0, Span::default());
        for index in 0..self.functions.len() {
            if self.functions[index].template.is_none() {
                state.index(
                    FnRef {
                        index,
                        args: vec![],
                    },
                    root,
                );
            }
        }
        let none = HashMap::new();
        let mut globals = vec![];
        for index in 0..self.globals.len() {
            let mut value = self.globals[index].lowered.clone().expect(NO_ERRORS);
            self.instantiate_expr(&mut value, &none, &mut state)?;
            globals.push(value);
        }
        let main = self.main().map(|index| ir::Expr::Call {
            function: state.index(
                FnRef {
                    index,
                    args: vec![],
                },
                root,
            ),
            args: vec![],
        });

        let mut functions = vec![];
        while let Some((function, index, site)) = state.queue.pop() {
            let mut lowered = self.lowered_functions[function.index]
                .clone()
                .expect(NO_ERRORS);
            let mut params = match self.functions[function.index].template {
                Some(template) => {
                    // Named after the type made from the generic type, rather
                    // than the generic type itself.
                    let generic = &self.templates[template].generic;
                    let generic = &generic.as_ref().expect(NO_ERRORS).name;
                    let instance = self.instance_name(template, &function.args);
                    lowered.name = lowered.name.replacen(
                        &format!("::{generic}."),
                        &format!("::{instance}."),
                        1,
                    );
                    self.params(template, &function.args)
                }
                None => HashMap::new(),
            };
            state.site = site;
            // The aliases its constraints don't give are its arguments'.
            if let Some(constraints) = self.functions[function.index].constraints {
                let (file, span) = site;
                self.cx.file = file;
                self.add_alias_params(constraints, &mut params, span)?;
            }
            for local in &mut lowered.locals {
                *local = self.concrete(*local, &params, &state)?;
            }
            lowered.returns = self.concrete(lowered.returns, &params, &state)?;
            if let ir::Body::Stmts(stmts) = &mut lowered.body {
                for stmt in stmts {
                    self.instantiate_stmt(stmt, &params, &mut state)?;
                }
            }
            if functions.len() <= index {
                functions.resize_with(index + 1, || None);
            }
            functions[index] = Some(lowered);
        }
        let functions = functions
            .into_iter()
            .map(|function| function.expect(NO_ERRORS));
        Some(Instantiated {
            functions: functions.collect(),
            globals,
            main,
        })
    }
    /// The `id` of the type with `id`, with what `params` says stand-ins stand
    /// for in place of them, for the use of a function that `state` is making.
    pub(super) fn concrete(
        &mut self,
        id: usize,
        params: &HashMap<usize, Type>,
        state: &Instantiating,
    ) -> Option<usize> {
        if params.is_empty() {
            return Some(id);
        }
        let ty = Type {
            id,
            name: self.types[id].name.clone(),
            parts: None,
        };
        let (file, span) = state.site;
        self.cx.file = file;
        Some(self.substitute(&ty, params, span)?.id)
    }
    /// Makes `stmt` use the functions and types that `state` makes, with what
    /// `params` says stand-ins stand for in place of them.
    pub(super) fn instantiate_stmt(
        &mut self,
        stmt: &mut ir::Stmt,
        params: &HashMap<usize, Type>,
        state: &mut Instantiating,
    ) -> Option<()> {
        match stmt {
            ir::Stmt::Assign { value, .. } | ir::Stmt::Return(value) | ir::Stmt::Expr(value) => {
                self.instantiate_expr(value, params, state)?;
            }
            ir::Stmt::If {
                cond,
                then_block,
                else_block,
            } => {
                self.instantiate_expr(cond, params, state)?;
                for stmt in then_block.iter_mut().chain(else_block) {
                    self.instantiate_stmt(stmt, params, state)?;
                }
            }
            ir::Stmt::Match { value, ty, arms } => {
                self.instantiate_expr(value, params, state)?;
                *ty = self.concrete(*ty, params, state)?;
                for stmt in arms.iter_mut().flat_map(|arm| &mut arm.body) {
                    self.instantiate_stmt(stmt, params, state)?;
                }
            }
        }
        Some(())
    }
    /// Makes `expr` use the functions and types that `state` makes, like
    /// `instantiate_stmt`.
    pub(super) fn instantiate_expr(
        &mut self,
        expr: &mut ir::Expr,
        params: &HashMap<usize, Type>,
        state: &mut Instantiating,
    ) -> Option<()> {
        match expr {
            ir::Expr::Bool(_)
            | ir::Expr::Local(_)
            | ir::Expr::Global(_)
            | ir::Expr::Tag(_)
            | ir::Expr::Error => {}
            ir::Expr::Int { ty, .. } => *ty = self.concrete(*ty, params, state)?,
            ir::Expr::Call { function, args } => {
                *function = self.instantiate_callee(*function, params, state)?;
                for arg in args {
                    self.instantiate_expr(arg, params, state)?;
                }
            }
            ir::Expr::Function(function) => {
                *function = self.instantiate_callee(*function, params, state)?;
            }
            ir::Expr::Construct { group, fields } => {
                *group = self.concrete(*group, params, state)?;
                for field in fields {
                    self.instantiate_expr(field, params, state)?;
                }
            }
            ir::Expr::Field { receiver, .. } => self.instantiate_expr(receiver, params, state)?,
        }
        Some(())
    }
    /// The index among the functions that `state` makes of the function at
    /// `callee` in `callees`, with what `params` says stand-ins stand for in
    /// place of them in its arguments. A function standing for one of a
    /// constraint's trait's methods is the method its arguments have.
    pub(super) fn instantiate_callee(
        &mut self,
        callee: usize,
        params: &HashMap<usize, Type>,
        state: &mut Instantiating,
    ) -> Option<usize> {
        let (function, file, span) = self.callees[callee].clone();
        self.cx.file = file;
        let args = function
            .args
            .iter()
            .map(|arg| self.substitute(arg, params, span))
            .collect::<Option<_>>()?;
        let function = self.constrained_callee(FnRef {
            index: function.index,
            args,
        });
        Some(state.index(function, (file, span)))
    }
}
