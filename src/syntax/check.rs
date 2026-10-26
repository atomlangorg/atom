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

//! Checks that names are declared before use, that only `var`s are
//! reassigned, that functions are called and return correctly, that structs
//! are well formed, and that every expression is well typed.
//!
//! Every file is a module, named by its path from the project root, so
//! `dir/other.atom` is `/dir/other`. The top level of a module can only hold
//! declarations, which are hoisted so they can be used in any order, even
//! across modules. This is safe because functions can't capture values from
//! outside them, so calling one early never reads a value that hasn't been set
//! yet, and top-level values whose initialisers depend on themselves are
//! rejected.
//!
//! The standard library is a module called `stdlib`, which every module
//! imports without needing a `use`.

use std::{
    collections::{HashMap, HashSet},
    fmt, iter,
    sync::LazyLock,
};

use super::{
    ast::{
        Arg, BinOp, Expr, Field, FieldValue, ModulePath, Mutability, Name, Param, Program, Stmt,
        UnOp,
    },
    parser::parse,
};

/// The name of the standard library, which is how it's imported and
/// qualified, as in `stdlib.Int`.
const STDLIB_NAME: &str = "stdlib";

static STDLIB: LazyLock<Program> = LazyLock::new(|| {
    parse(include_str!("../stdlib.atom")).expect("the standard library should parse")
});

/// A parsed `.atom` file.
#[derive(Debug, Clone)]
pub struct Module {
    /// The file's path from the project root without `.atom`, such as
    /// `["dir", "other"]` for `dir/other.atom`.
    pub path: Vec<String>,
    pub program: Program,
}

/// Every type is a struct, including `Int`, `Bool` and `Void`, which are
/// declared in the standard library. Structs in different scopes can share a
/// name, so they're told apart by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    id: usize,
    name: String,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// An error, along with the module it's in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleError {
    /// The module's path, such as `/dir/other`, or `stdlib`.
    pub module: String,
    pub error: CheckError,
}

impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.module, self.error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A name was used or assigned without being declared.
    Undeclared(String),
    /// A function used a value declared outside it.
    Captured(String),
    /// A name without a module was declared in more than one of the modules
    /// it could come from.
    Ambiguous { name: String, modules: Vec<String> },
    /// A `use` named a module that doesn't exist.
    UnknownModule(String),
    /// A qualified name's module isn't the current module or one it imports.
    NotImported(String),
    /// A qualified name's module matched more than one module by name, so
    /// its full path is needed.
    AmbiguousModule { name: String, modules: Vec<String> },
    /// A top-level value's initialiser depends on the value itself.
    CyclicValue(String),
    /// A struct contains itself, directly or through other structs, so it
    /// would be infinitely large.
    RecursiveStruct(String),
    /// A name was declared twice.
    Redeclared(String),
    /// A `let` constant was assigned to.
    AssignToConstant(String),
    /// An operator was applied to an operand of the wrong type.
    InvalidOperand {
        op: &'static str,
        expected: Type,
        found: Type,
    },
    /// A name was given a value of a different type to its declared type,
    /// either in an explicitly typed declaration or a later assignment.
    AssignTypeMismatch {
        name: String,
        expected: Type,
        found: Type,
    },
    /// An `if` condition was not a `Bool`.
    ConditionNotBool(Type),
    /// An explicit type named a type that doesn't exist.
    UnknownType(String),
    /// A value or function was used as a type.
    NotAType(String),
    /// Something that isn't a function was called.
    NotAFunction(String),
    /// A function was used as a value, or assigned to.
    NotAValue(String),
    /// A type was used as a value, or assigned to.
    TypeAsValue(String),
    /// A function was called with the wrong number of arguments.
    WrongArgumentCount {
        function: String,
        expected: usize,
        found: usize,
    },
    /// An argument's label didn't match its parameter's label. `None` means
    /// no label.
    WrongLabel {
        function: String,
        expected: Option<String>,
        found: Option<String>,
    },
    /// A struct was constructed with the wrong number of fields.
    WrongFieldCount {
        ty: String,
        expected: usize,
        found: usize,
    },
    /// A struct was constructed with a field that isn't the next one it
    /// declares, either because the name is wrong or it's out of order.
    WrongField {
        ty: String,
        expected: String,
        found: String,
    },
    /// A struct was constructed with a field value of the wrong type.
    FieldTypeMismatch {
        ty: String,
        field: String,
        expected: Type,
        found: Type,
    },
    /// An argument had the wrong type.
    ArgumentTypeMismatch {
        function: String,
        /// The argument's label, or the parameter name if it has none.
        param: String,
        expected: Type,
        found: Type,
    },
    /// `return` had no value in a function that doesn't return `Void`.
    MissingReturnValue { function: String, expected: Type },
    /// A returned value had the wrong type.
    ReturnTypeMismatch {
        function: String,
        expected: Type,
        found: Type,
    },
    /// A function that doesn't return `Void` can finish without returning.
    MissingReturn { function: String, expected: Type },
}

/// Formats `items` as "`a`", "`a` and `b`", or "`a`, `b` and `c`", using
/// `conjunction` in place of "and".
fn quoted_list(items: &[String], conjunction: &str) -> String {
    let quoted: Vec<_> = items.iter().map(|item| format!("`{item}`")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} {conjunction} {last}", rest.join(", ")),
        None => String::new(),
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Undeclared(name) => write!(f, "`{name}` is not declared"),
            Self::Captured(name) => write!(
                f,
                "functions can't use `{name}`, because it is declared outside the function"
            ),
            Self::Ambiguous { name, modules } => write!(
                f,
                "`{name}` is declared in {}, so it must be qualified",
                quoted_list(modules, "and")
            ),
            Self::UnknownModule(path) => write!(f, "cannot find module `{path}`"),
            Self::NotImported(path) => write!(f, "`{path}` is not an imported module"),
            Self::AmbiguousModule { name, modules } => write!(
                f,
                "`{name}` could be {}, so the full path must be used",
                quoted_list(modules, "or")
            ),
            Self::CyclicValue(name) => write!(f, "`{name}` depends on its own value"),
            Self::RecursiveStruct(name) => write!(f, "`{name}` contains itself"),
            Self::Redeclared(name) => write!(f, "`{name}` is already declared"),
            Self::AssignToConstant(name) => {
                write!(
                    f,
                    "cannot assign to `{name}` because it was declared with `let`"
                )
            }
            Self::InvalidOperand {
                op,
                expected,
                found,
            } => write!(f, "operator `{op}` expects `{expected}`, found `{found}`"),
            Self::AssignTypeMismatch {
                name,
                expected,
                found,
            } => write!(
                f,
                "cannot assign `{found}` to `{name}`, which has type `{expected}`"
            ),
            Self::ConditionNotBool(found) => {
                write!(f, "`if` condition must be `Bool`, found `{found}`")
            }
            Self::UnknownType(name) => write!(f, "unknown type `{name}`"),
            Self::NotAFunction(name) => write!(f, "`{name}` is not a function"),
            Self::NotAType(name) => write!(f, "`{name}` is not a type"),
            Self::NotAValue(name) => write!(f, "`{name}` is a function, not a value"),
            Self::TypeAsValue(name) => write!(f, "`{name}` is a type, not a value"),
            Self::WrongArgumentCount {
                function,
                expected,
                found,
            } => {
                let s = if *expected == 1 { "" } else { "s" };
                write!(
                    f,
                    "`{function}` takes {expected} argument{s}, found {found}"
                )
            }
            Self::WrongLabel {
                function,
                expected,
                found,
            } => match (expected, found) {
                (Some(expected), Some(found)) => write!(
                    f,
                    "argument to `{function}` should be labelled `{expected}`, found `{found}`"
                ),
                (Some(expected), None) => write!(
                    f,
                    "argument to `{function}` is missing the label `{expected}`"
                ),
                (None, Some(found)) => write!(
                    f,
                    "argument to `{function}` should not have a label, found `{found}`"
                ),
                (None, None) => write!(f, "argument to `{function}` has the wrong label"),
            },
            Self::WrongFieldCount {
                ty,
                expected,
                found,
            } => {
                let s = if *expected == 1 { "" } else { "s" };
                write!(f, "`{ty}` has {expected} field{s}, found {found}")
            }
            Self::WrongField {
                ty,
                expected,
                found,
            } => write!(f, "expected field `{expected}` of `{ty}`, found `{found}`"),
            Self::FieldTypeMismatch {
                ty,
                field,
                expected,
                found,
            } => write!(
                f,
                "field `{field}` of `{ty}` expects `{expected}`, found `{found}`"
            ),
            Self::ArgumentTypeMismatch {
                function,
                param,
                expected,
                found,
            } => write!(
                f,
                "argument `{param}` of `{function}` expects `{expected}`, found `{found}`"
            ),
            Self::MissingReturnValue { function, expected } => {
                write!(f, "`{function}` must return a value of type `{expected}`")
            }
            Self::ReturnTypeMismatch {
                function,
                expected,
                found,
            } => write!(f, "`{function}` must return `{expected}`, found `{found}`"),
            Self::MissingReturn { function, expected } => write!(
                f,
                "`{function}` must return a value of type `{expected}` on every path"
            ),
        }
    }
}

/// Checks every module in a project, along with the standard library.
pub fn check(modules: &[Module]) -> Result<(), Vec<ModuleError>> {
    let mut checker = Checker::default();
    checker.add_module(STDLIB_NAME.to_string(), &[], STDLIB_NAME, &STDLIB);
    for module in modules {
        let name = module.path.last().expect("module paths aren't empty");
        let display = format!("/{}", module.path.join("/"));
        checker.add_module(display, &module.path, name, &module.program);
    }
    checker.check_modules();

    if checker.errors.is_empty() {
        Ok(())
    } else {
        Err(checker.errors)
    }
}

#[derive(Debug, Clone)]
struct Signature<'a> {
    params: Vec<ParamSignature<'a>>,
    /// `None` if the return type is unknown.
    returns: Option<Type>,
}

#[derive(Debug, Clone)]
struct ParamSignature<'a> {
    label: Option<&'a str>,
    name: &'a str,
    /// `None` if the parameter's type is unknown.
    ty: Option<Type>,
}

#[derive(Debug, Clone)]
struct FieldSignature<'a> {
    name: &'a str,
    /// `None` if the field's type is unknown.
    ty: Option<Type>,
}

#[derive(Debug, Clone)]
enum Binding {
    Value {
        mutability: Mutability,
        /// `None` if the type couldn't be determined because of an error, so
        /// that later uses don't report errors caused by the same mistake.
        ty: Option<Type>,
    },
    /// A top-level value, as an index into `Checker::globals`.
    Global(usize),
    /// An index into `Checker::functions`.
    Function(usize),
    Type(Type),
}

/// A value declared at the top level of a module. Its type is worked out when
/// it's first needed, since top-level values can be used in any order.
struct Global<'a> {
    module: usize,
    name: &'a str,
    ty: Option<&'a Name>,
    value: &'a Expr,
    state: GlobalState,
}

enum GlobalState {
    Unchecked,
    /// Its initialiser is being checked, so needing its type again means it
    /// depends on itself.
    Checking,
    Checked(Option<Type>),
}

/// The function whose body is being checked.
#[derive(Debug, Clone)]
struct FnContext<'a> {
    name: &'a str,
    /// `None` if the return type is unknown.
    returns: Option<Type>,
}

struct ModuleInfo<'a> {
    /// The module's path, such as `/dir/other`, or `stdlib`.
    display: String,
    /// The module's path from the project root. Empty for the standard
    /// library.
    path: &'a [String],
    /// The name used to qualify the module's names, such as `other` in
    /// `other.f`.
    name: &'a str,
    program: &'a Program,
    /// The names declared at the module's top level.
    scope: HashMap<&'a str, Binding>,
    /// The other modules whose top-level names this one can use.
    imports: Vec<usize>,
}

/// Why `Checker::lookup` couldn't find a name.
enum LookupError {
    Undeclared,
    Captured,
    /// The name was found in each of these modules.
    Ambiguous(Vec<String>),
    /// The name's module couldn't be found.
    Module(CheckError),
}

/// A top-level declaration that still needs checking once every module's
/// names have been declared.
enum Pending<'a> {
    Global(usize),
    Fn {
        index: usize,
        name: &'a str,
        body: &'a [Stmt],
    },
}

#[derive(Default)]
struct Checker<'a> {
    /// The standard library first, then the project's modules.
    modules: Vec<ModuleInfo<'a>>,
    /// The index in `modules` of the module being checked.
    module: usize,
    /// One scope per enclosing block inside functions, innermost last. Empty
    /// at the top level, where names are in the module's own scope. Values,
    /// functions and types all share the same names.
    scopes: Vec<HashMap<&'a str, Binding>>,
    /// The index in `scopes` of the current function's outermost scope. Names
    /// can't be redeclared from here inwards.
    fn_scope_start: usize,
    current_fn: Option<FnContext<'a>>,
    functions: Vec<Signature<'a>>,
    globals: Vec<Global<'a>>,
    /// For each struct, indexed by `id`, its fields in the order they're
    /// declared.
    struct_fields: Vec<Vec<FieldSignature<'a>>>,
    errors: Vec<ModuleError>,
}

impl<'a> Checker<'a> {
    fn add_module(
        &mut self,
        display: String,
        path: &'a [String],
        name: &'a str,
        program: &'a Program,
    ) {
        self.modules.push(ModuleInfo {
            display,
            path,
            name,
            program,
            scope: HashMap::new(),
            imports: vec![],
        });
    }

    fn error(&mut self, error: CheckError) {
        self.error_in(self.module, error);
    }

    fn error_in(&mut self, module: usize, error: CheckError) {
        let module = self.modules[module].display.clone();
        self.errors.push(ModuleError { module, error });
    }

    /// Checks every module. Each kind of top-level declaration is declared in
    /// every module before the next kind, and before anything is checked, so
    /// top-level names can be used in any order.
    fn check_modules(&mut self) {
        for module in 0..self.modules.len() {
            self.module = module;
            self.resolve_imports();
        }

        // Every struct is declared before any fields are resolved, so structs
        // can contain each other in any order.
        let mut structs = vec![];
        for module in 0..self.modules.len() {
            self.module = module;
            for stmt in self.modules[module].program {
                if let Stmt::Struct { name, fields } = stmt {
                    structs.push((module, self.declare_struct(name), name, fields));
                }
            }
        }
        for &(module, id, _, fields) in &structs {
            self.module = module;
            self.define_struct(id, fields);
        }
        for &(module, id, name, _) in &structs {
            self.module = module;
            self.check_not_recursive(id, name);
        }

        // Functions and values come after structs, since their types can be
        // structs.
        let mut pending = vec![];
        for module in 0..self.modules.len() {
            self.module = module;
            for stmt in self.modules[module].program {
                match stmt {
                    Stmt::Fn {
                        name,
                        params,
                        ret,
                        body,
                    } => {
                        let index = self.declare_fn(name, params, ret);
                        pending.push((module, Pending::Fn { index, name, body }));
                    }
                    Stmt::Decl {
                        name, ty, value, ..
                    } => {
                        self.globals.push(Global {
                            module,
                            name,
                            ty: ty.as_ref(),
                            value,
                            state: GlobalState::Unchecked,
                        });
                        let index = self.globals.len() - 1;
                        self.declare(name, Binding::Global(index));
                        pending.push((module, Pending::Global(index)));
                    }
                    Stmt::Use(_) | Stmt::Struct { .. } => {}
                    _ => unreachable!("the parser only allows declarations at the top level"),
                }
            }
        }

        for (module, pending) in pending {
            self.module = module;
            match pending {
                Pending::Global(index) => {
                    self.global_type(index);
                }
                Pending::Fn { index, name, body } => self.fn_body(index, name, body),
            }
        }
    }

    /// Finds the modules imported by the current module. The standard library
    /// is always imported.
    fn resolve_imports(&mut self) {
        let mut imports = vec![];
        if self.module != 0 {
            imports.push(0);
        }

        for stmt in self.modules[self.module].program {
            let Stmt::Use(path) = stmt else { continue };
            match self.resolve_path(path) {
                // Importing a module twice, or importing itself, is harmless.
                Some(module) if module == self.module || imports.contains(&module) => {}
                Some(module) => imports.push(module),
                None => self.error(CheckError::UnknownModule(path.to_string())),
            }
        }

        self.modules[self.module].imports = imports;
    }

    /// Finds the module at `path`, which is relative to the current module if
    /// it starts with `./`. Only libraries, such as `stdlib`, are found by
    /// name.
    fn resolve_path(&self, path: &ModulePath) -> Option<usize> {
        let full: Vec<&String> = match path {
            ModulePath::Absolute(segments) => segments.iter().collect(),
            ModulePath::Relative(segments) => {
                let current = self.modules[self.module].path;
                let dir = current.split_last().map_or(current, |(_, dir)| dir);
                dir.iter().chain(segments).collect()
            }
            ModulePath::Named(name) => return (name == STDLIB_NAME).then_some(0),
        };
        self.modules
            .iter()
            .position(|module| module.path.iter().eq(full.iter().copied()))
    }

    /// The modules whose top-level names the current module can use: itself,
    /// then its imports.
    fn visible_modules(&self) -> impl Iterator<Item = usize> + '_ {
        iter::once(self.module).chain(self.modules[self.module].imports.iter().copied())
    }

    /// Finds the module a qualified name refers to. It must be the current
    /// module or one it imports.
    fn resolve_qualifier(&self, path: &ModulePath) -> Result<usize, CheckError> {
        if let ModulePath::Named(name) = path {
            let matches: Vec<_> = self
                .visible_modules()
                .filter(|&module| self.modules[module].name == name)
                .collect();
            return match matches[..] {
                [] => Err(CheckError::NotImported(name.clone())),
                [module] => Ok(module),
                _ => Err(CheckError::AmbiguousModule {
                    name: name.clone(),
                    modules: matches
                        .iter()
                        .map(|&module| self.modules[module].display.clone())
                        .collect(),
                }),
            };
        }

        self.resolve_path(path)
            .filter(|module| self.visible_modules().any(|visible| visible == *module))
            .ok_or_else(|| CheckError::NotImported(path.to_string()))
    }

    /// A type from the standard library, such as `Int` for integer literals.
    /// Found in the standard library itself, so it can't be shadowed.
    fn std_type(&self, name: &str) -> Type {
        match self.modules[0].scope.get(name) {
            Some(Binding::Type(ty)) => ty.clone(),
            _ => panic!("the standard library should declare `{name}`"),
        }
    }

    fn lookup(&self, name: &Name) -> Result<Binding, LookupError> {
        let binding = match &name.module {
            Some(path) => {
                let module = self.resolve_qualifier(path).map_err(LookupError::Module)?;
                self.modules[module]
                    .scope
                    .get(name.name.as_str())
                    .ok_or(LookupError::Undeclared)?
            }
            None => {
                let local = self
                    .scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, scope)| scope.get(name.name.as_str()).map(|b| (index, b)));
                match local {
                    // Functions can't capture values from outside them.
                    // Functions and types can still be used, since they never
                    // depend on a value.
                    Some((index, Binding::Value { .. })) if index < self.fn_scope_start => {
                        return Err(LookupError::Captured);
                    }
                    Some((_, binding)) => return Ok(binding.clone()),
                    None => {}
                }

                // A name that isn't qualified must only be declared in one of
                // the modules it could come from.
                let found: Vec<_> = self
                    .visible_modules()
                    .filter_map(|module| {
                        let binding = self.modules[module].scope.get(name.name.as_str())?;
                        Some((module, binding))
                    })
                    .collect();
                match found[..] {
                    [] => return Err(LookupError::Undeclared),
                    [(_, binding)] => binding,
                    _ => {
                        let modules = found
                            .iter()
                            .map(|&(module, _)| self.modules[module].display.clone())
                            .collect();
                        return Err(LookupError::Ambiguous(modules));
                    }
                }
            }
        };

        // Top-level values are outside every function.
        if matches!(binding, Binding::Global(_)) && self.current_fn.is_some() {
            return Err(LookupError::Captured);
        }
        Ok(binding.clone())
    }

    /// Reports why `name` couldn't be found.
    fn lookup_error(&mut self, name: &Name, error: LookupError) {
        let name_string = name.to_string();
        let error = match error {
            LookupError::Undeclared => CheckError::Undeclared(name_string),
            LookupError::Captured => CheckError::Captured(name_string),
            LookupError::Ambiguous(modules) => CheckError::Ambiguous {
                name: name_string,
                modules,
            },
            LookupError::Module(error) => error,
        };
        self.error(error);
    }

    fn declare(&mut self, name: &'a str, binding: Binding) {
        // Names can't be redeclared within a function, even to shadow an
        // outer block. A function's own scope can reuse names from outside it.
        // At the top level, names only need to be unique within their module.
        let scope = match self.scopes.last_mut() {
            Some(_) => {
                if self.scopes[self.fn_scope_start..]
                    .iter()
                    .any(|scope| scope.contains_key(name))
                {
                    self.error(CheckError::Redeclared(name.to_string()));
                }
                self.scopes.last_mut().expect("checked above")
            }
            None => {
                if self.modules[self.module].scope.contains_key(name) {
                    self.error(CheckError::Redeclared(name.to_string()));
                }
                &mut self.modules[self.module].scope
            }
        };
        scope.insert(name, binding);
    }

    fn resolve_type(&mut self, name: &Name) -> Option<Type> {
        match self.lookup(name) {
            Ok(Binding::Type(ty)) => Some(ty),
            // A value from outside the function is still not a type.
            Ok(_) | Err(LookupError::Captured) => {
                self.error(CheckError::NotAType(name.to_string()));
                None
            }
            Err(LookupError::Undeclared) => {
                self.error(CheckError::UnknownType(name.to_string()));
                None
            }
            Err(error) => {
                self.lookup_error(name, error);
                None
            }
        }
    }

    /// The type of the top-level value at `index` in `globals`, checking its
    /// declaration if that hasn't been done yet.
    fn global_type(&mut self, index: usize) -> Option<Type> {
        let global = &self.globals[index];
        match &global.state {
            GlobalState::Checked(ty) => return ty.clone(),
            GlobalState::Checking => {
                let error = CheckError::CyclicValue(global.name.to_string());
                self.error_in(global.module, error);
                return None;
            }
            GlobalState::Unchecked => {}
        }

        // Checked as part of its own module, even when another module needs
        // it first. Top-level values are only used at the top level, so
        // there are no local scopes to set aside.
        let (module, name, type_name, value) =
            (global.module, global.name, global.ty, global.value);
        self.globals[index].state = GlobalState::Checking;
        let outer_module = std::mem::replace(&mut self.module, module);
        let ty = self.decl_type(name, type_name, value);
        self.module = outer_module;

        self.globals[index].state = GlobalState::Checked(ty.clone());
        ty
    }

    /// Checks a `let` or `var` declaration's value, returning the type it's
    /// declared with.
    fn decl_type(&mut self, name: &str, type_name: Option<&Name>, value: &Expr) -> Option<Type> {
        // Checked first so that `let x = x` refers to an earlier `x`.
        let found = self.expr(value);

        // An explicit type takes precedence over the value's type, so later
        // uses are checked against what the programmer wrote.
        match type_name {
            Some(type_name) => {
                let expected = self.resolve_type(type_name);
                self.check_assignable(name, expected.clone(), found);
                expected
            }
            None => found,
        }
    }

    /// Declares a function, returning its index into `functions`.
    fn declare_fn(&mut self, name: &'a str, params: &'a [Param], ret: &Name) -> usize {
        let params = params
            .iter()
            .map(|param| ParamSignature {
                label: param.label.as_deref(),
                name: &param.name,
                ty: self.resolve_type(&param.ty),
            })
            .collect();
        let returns = self.resolve_type(ret);

        self.functions.push(Signature { params, returns });
        let index = self.functions.len() - 1;
        self.declare(name, Binding::Function(index));
        index
    }

    /// Checks the body of the function at `index` in `functions`.
    fn fn_body(&mut self, index: usize, name: &'a str, body: &'a [Stmt]) {
        let Signature { params, returns } = self.functions[index].clone();

        // The body starts a new function scope holding the parameters. It can
        // see the functions and types declared outside it, but not values.
        let outer_scope_start = std::mem::replace(&mut self.fn_scope_start, self.scopes.len());
        let outer_fn = self.current_fn.replace(FnContext {
            name,
            returns: returns.clone(),
        });
        self.scopes.push(HashMap::new());

        for param in params {
            let binding = Binding::Value {
                mutability: Mutability::Let,
                ty: param.ty,
            };
            self.declare(param.name, binding);
        }
        for stmt in body {
            self.stmt(stmt);
        }

        self.scopes.pop();
        self.current_fn = outer_fn;
        self.fn_scope_start = outer_scope_start;

        // A `Void` function can finish without an explicit `return`.
        if let Some(expected) = returns
            && expected != self.std_type("Void")
            && !always_returns(body)
        {
            self.error(CheckError::MissingReturn {
                function: name.to_string(),
                expected,
            });
        }
    }

    /// Declares a struct without its fields, returning its `id`.
    fn declare_struct(&mut self, name: &'a str) -> usize {
        let id = self.struct_fields.len();
        self.struct_fields.push(vec![]);
        let ty = Type {
            id,
            name: name.to_string(),
        };
        self.declare(name, Binding::Type(ty));
        id
    }

    /// Resolves the fields of the struct with `id`.
    fn define_struct(&mut self, id: usize, fields: &'a [Field]) {
        // Field names only need to be unique within their struct.
        let mut field_names = HashSet::new();
        for field in fields {
            if !field_names.insert(&field.name) {
                self.error(CheckError::Redeclared(field.name.clone()));
            }
            let ty = self.resolve_type(&field.ty);
            self.struct_fields[id].push(FieldSignature {
                name: &field.name,
                ty,
            });
        }
    }

    /// The `id`s of the structs that the fields of the struct with `id` have
    /// as types.
    fn contained_structs(&self, id: usize) -> impl Iterator<Item = usize> + '_ {
        self.struct_fields[id]
            .iter()
            .filter_map(|field| field.ty.as_ref().map(|ty| ty.id))
    }

    /// Reports an error if the struct with `id` contains itself, which would
    /// make it infinitely large.
    fn check_not_recursive(&mut self, id: usize, name: &str) {
        let mut seen = HashSet::new();
        let mut unvisited: Vec<_> = self.contained_structs(id).collect();
        while let Some(next) = unvisited.pop() {
            if next == id {
                self.error(CheckError::RecursiveStruct(name.to_string()));
                return;
            }
            if seen.insert(next) {
                unvisited.extend(self.contained_structs(next));
            }
        }
    }

    /// Checks `stmts` in a new scope, so their declarations aren't visible
    /// afterwards.
    fn block(&mut self, stmts: &'a [Stmt]) {
        self.scopes.push(HashMap::new());
        for stmt in stmts {
            self.stmt(stmt);
        }
        self.scopes.pop();
    }

    /// Checks a statement inside a function.
    fn stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Use(_) => unreachable!("the parser only allows `use` at the top level"),
            Stmt::Decl {
                mutability,
                name,
                ty: type_name,
                value,
            } => {
                let ty = self.decl_type(name, type_name.as_ref(), value);
                let binding = Binding::Value {
                    mutability: *mutability,
                    ty,
                };
                self.declare(name, binding);
            }
            Stmt::Assign { name, value } => {
                let found = self.expr(value);
                let target = Name::new(name);
                match self.lookup(&target) {
                    Ok(Binding::Value {
                        mutability: Mutability::Var,
                        ty: expected,
                    }) => self.check_assignable(name, expected, found),
                    Ok(Binding::Value {
                        mutability: Mutability::Let,
                        ..
                    }) => {
                        self.error(CheckError::AssignToConstant(name.clone()));
                    }
                    Ok(Binding::Function(_)) => {
                        self.error(CheckError::NotAValue(name.clone()));
                    }
                    Ok(Binding::Type(_)) => {
                        self.error(CheckError::TypeAsValue(name.clone()));
                    }
                    // `lookup` never finds top-level values from inside a
                    // function, which is the only place assignments can be.
                    Ok(Binding::Global(_)) => self.lookup_error(&target, LookupError::Captured),
                    Err(error) => self.lookup_error(&target, error),
                }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
            } => {
                if let Some(found) = self.expr(cond)
                    && found != self.std_type("Bool")
                {
                    self.error(CheckError::ConditionNotBool(found));
                }

                self.block(then_block);
                if let Some(else_block) = else_block {
                    self.block(else_block);
                }
            }
            // Inside functions, functions and structs are only visible after
            // they're declared.
            Stmt::Fn {
                name,
                params,
                ret,
                body,
            } => {
                // Declared before the body is checked, so it can call itself.
                let index = self.declare_fn(name, params, ret);
                self.fn_body(index, name, body);
            }
            Stmt::Struct { name, fields } => {
                let id = self.declare_struct(name);
                self.define_struct(id, fields);
                self.check_not_recursive(id, name);
            }
            Stmt::Return(value) => {
                let found = value.as_ref().map(|value| self.expr(value));
                let FnContext { name, returns } = self
                    .current_fn
                    .clone()
                    .expect("statements are only checked inside functions");

                // A bare `return` returns `Void`.
                let function = name.to_string();
                match (returns, found) {
                    (Some(expected), None) if expected != self.std_type("Void") => {
                        self.error(CheckError::MissingReturnValue { function, expected });
                    }
                    (Some(expected), Some(Some(found))) if expected != found => {
                        self.error(CheckError::ReturnTypeMismatch {
                            function,
                            expected,
                            found,
                        });
                    }
                    _ => {}
                }
            }
            Stmt::Expr(expr) => match expr {
                // A call's return value can be ignored.
                Expr::Call { callee, args } => {
                    self.call(callee, args);
                }
                _ => {
                    self.expr(expr);
                }
            },
        }
    }

    /// Returns the type of `expr`, or `None` if it can't be determined
    /// because of an error that has already been reported.
    fn expr(&mut self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Int(_) => Some(self.std_type("Int")),
            Expr::Bool(_) => Some(self.std_type("Bool")),
            Expr::Var(name) => match self.lookup(name) {
                Ok(Binding::Value { ty, .. }) => ty,
                Ok(Binding::Global(index)) => self.global_type(index),
                Ok(Binding::Function(_)) => {
                    self.error(CheckError::NotAValue(name.to_string()));
                    None
                }
                Ok(Binding::Type(_)) => {
                    self.error(CheckError::TypeAsValue(name.to_string()));
                    None
                }
                Err(error) => {
                    self.lookup_error(name, error);
                    None
                }
            },
            // Operators always produce their result type, even if an operand
            // is invalid, so one bad operand doesn't cause further errors.
            Expr::Unary(op, operand) => {
                let ty = self.std_type(unary_type(op));
                self.operand(op.symbol(), operand, ty.clone());
                Some(ty)
            }
            Expr::Binary(lhs, op, rhs) => {
                let ty = self.std_type(binary_type(op));
                self.operand(op.symbol(), lhs, ty.clone());
                self.operand(op.symbol(), rhs, ty.clone());
                Some(ty)
            }
            Expr::Call { callee, args } => self.call(callee, args),
            Expr::Construct { ty, fields } => self.construct(ty, fields),
        }
    }

    /// Checks the construction of a struct, returning its type, or `None` if
    /// it can't be determined.
    fn construct(&mut self, type_name: &Name, values: &[FieldValue]) -> Option<Type> {
        // Values are checked first, so their errors come before the struct's.
        let found: Vec<_> = values.iter().map(|field| self.expr(&field.value)).collect();

        let ty = self.resolve_type(type_name)?;
        let fields = self.struct_fields[ty.id].clone();
        let name = type_name.to_string();

        // Every field must be given, in the order the struct declares them.
        if values.len() != fields.len() {
            self.error(CheckError::WrongFieldCount {
                ty: name,
                expected: fields.len(),
                found: values.len(),
            });
            return Some(ty);
        }

        for ((value, found), field) in values.iter().zip(found).zip(&fields) {
            if value.name != field.name {
                self.error(CheckError::WrongField {
                    ty: name.clone(),
                    expected: field.name.to_string(),
                    found: value.name.clone(),
                });
            } else if let (Some(expected), Some(found)) = (&field.ty, found)
                && *expected != found
            {
                self.error(CheckError::FieldTypeMismatch {
                    ty: name.clone(),
                    field: field.name.to_string(),
                    expected: expected.clone(),
                    found,
                });
            }
        }

        Some(ty)
    }

    /// Checks a call, returning the function's return type, or `None` if it
    /// can't be determined.
    fn call(&mut self, callee: &Name, args: &[Arg]) -> Option<Type> {
        // Arguments are checked first, so their errors come before the call's.
        let found: Vec<_> = args.iter().map(|arg| self.expr(&arg.value)).collect();

        let index = match self.lookup(callee) {
            Ok(Binding::Function(index)) => index,
            Ok(Binding::Value { .. } | Binding::Global(_) | Binding::Type(_)) => {
                self.error(CheckError::NotAFunction(callee.to_string()));
                return None;
            }
            Err(error) => {
                self.lookup_error(callee, error);
                return None;
            }
        };
        let signature = self.functions[index].clone();
        let function = callee.to_string();

        if args.len() != signature.params.len() {
            self.error(CheckError::WrongArgumentCount {
                function,
                expected: signature.params.len(),
                found: args.len(),
            });
            return signature.returns;
        }

        for ((arg, found), param) in args.iter().zip(found).zip(&signature.params) {
            if arg.label.as_deref() != param.label {
                self.error(CheckError::WrongLabel {
                    function: function.clone(),
                    expected: param.label.map(str::to_string),
                    found: arg.label.clone(),
                });
            } else if let (Some(expected), Some(found)) = (&param.ty, found)
                && *expected != found
            {
                self.error(CheckError::ArgumentTypeMismatch {
                    function: function.clone(),
                    param: param.label.unwrap_or(param.name).to_string(),
                    expected: expected.clone(),
                    found,
                });
            }
        }

        signature.returns
    }

    /// Reports an error if a value of type `found` can't be given to `name`,
    /// whose type is `expected`. Unknown types are assumed to be fine.
    fn check_assignable(&mut self, name: &str, expected: Option<Type>, found: Option<Type>) {
        if let (Some(expected), Some(found)) = (expected, found)
            && expected != found
        {
            self.error(CheckError::AssignTypeMismatch {
                name: name.to_string(),
                expected,
                found,
            });
        }
    }

    fn operand(&mut self, op: &'static str, operand: &Expr, expected: Type) {
        if let Some(found) = self.expr(operand)
            && found != expected
        {
            self.error(CheckError::InvalidOperand {
                op,
                expected,
                found,
            });
        }
    }
}

/// Whether `stmts` returns on every path. Both branches of an `if` must return
/// for it to count, so an `if` without an `else` never does.
fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(_) => true,
        Stmt::If {
            then_block,
            else_block: Some(else_block),
            ..
        } => always_returns(then_block) && always_returns(else_block),
        _ => false,
    })
}

/// The name of the standard library type of a unary operator's operand,
/// which is also its result type.
fn unary_type(op: &UnOp) -> &'static str {
    match op {
        UnOp::Neg => "Int",
        UnOp::Not => "Bool",
    }
}

/// The name of the standard library type of a binary operator's operands,
/// which is also its result type.
fn binary_type(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => "Int",
        BinOp::And | BinOp::Or => "Bool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    /// The standard library type called `name`.
    fn ty(name: &str) -> Type {
        let mut checker = Checker::default();
        checker.add_module(STDLIB_NAME.to_string(), &[], STDLIB_NAME, &STDLIB);
        checker.check_modules();
        checker.std_type(name)
    }

    /// Checks each `(path, src)` pair as a module, with paths like
    /// `/dir/other`.
    fn check_files(files: &[(&str, &str)]) -> Result<(), Vec<ModuleError>> {
        let modules: Vec<_> = files
            .iter()
            .map(|(path, src)| Module {
                path: path[1..].split('/').map(str::to_string).collect(),
                program: parse(src).expect("source should parse"),
            })
            .collect();
        check(&modules)
    }

    fn file_messages(files: &[(&str, &str)]) -> Vec<String> {
        match check_files(files) {
            Ok(()) => vec![],
            Err(errs) => errs.iter().map(ToString::to_string).collect(),
        }
    }

    /// Checks `src` as the only module, `/main`.
    fn check_top(src: &str) -> Result<(), Vec<CheckError>> {
        check_files(&[("/main", src)])
            .map_err(|errs| errs.into_iter().map(|err| err.error).collect())
    }

    fn messages_top(src: &str) -> Vec<String> {
        match check_top(src) {
            Ok(()) => vec![],
            Err(errs) => errs.iter().map(ToString::to_string).collect(),
        }
    }

    /// Checks `src` as the body of a function, since statements other than
    /// declarations aren't allowed at the top level.
    fn check_body(src: &str) -> Result<(), Vec<CheckError>> {
        check_top(&format!("fn body() {{\n{src}\n}}"))
    }

    fn invalid_operand(op: &'static str) -> CheckError {
        CheckError::InvalidOperand {
            op,
            expected: ty("Int"),
            found: ty("Bool"),
        }
    }

    #[test]
    fn accepts_valid_program() {
        assert_eq!(
            check_body("let x = 1\nvar y = x\ny = y + x\ny = -y"),
            Ok(())
        );
    }

    #[test]
    fn accepts_booleans() {
        assert_eq!(check_body("let t = true\nvar b = t\nb = false"), Ok(()));
    }

    #[test]
    fn accepts_boolean_operators() {
        assert_eq!(
            check_body("let t = true\nvar b = !t & (false | t)\nb = !!b | b"),
            Ok(())
        );
    }

    #[test]
    fn rejects_boolean_operators_on_integers() {
        let invalid = |op| CheckError::InvalidOperand {
            op,
            expected: ty("Bool"),
            found: ty("Int"),
        };

        assert_eq!(check_body("let x = !1"), Err(vec![invalid("!")]));
        assert_eq!(check_body("let x = true & 1"), Err(vec![invalid("&")]));
        assert_eq!(
            check_body("let x = 1 | 2"),
            Err(vec![invalid("|"), invalid("|")])
        );
    }

    #[test]
    fn boolean_operators_produce_booleans() {
        assert_eq!(
            check_body("let x = (true | false) + 1"),
            Err(vec![invalid_operand("+")])
        );
        assert_eq!(
            check_body("var n = 0\nn = !false"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "n".to_string(),
                expected: ty("Int"),
                found: ty("Bool"),
            }])
        );
    }

    #[test]
    fn rejects_mixing_negations() {
        // `-` needs an `Int` and `!` needs a `Bool`, so `-!x` and `!-x` can
        // never be valid.
        assert_eq!(
            check_body("let x = -!true"),
            Err(vec![invalid_operand("-")])
        );
    }

    #[test]
    fn rejects_assignment_without_declaration() {
        assert_eq!(
            check_body("x = 1"),
            Err(vec![CheckError::Undeclared("x".to_string())])
        );
    }

    #[test]
    fn rejects_assignment_to_constant() {
        assert_eq!(
            check_body("let x = 1\nx = 2"),
            Err(vec![CheckError::AssignToConstant("x".to_string())])
        );
    }

    #[test]
    fn rejects_use_of_undeclared_name() {
        assert_eq!(
            check_body("let x = y + 1"),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
    }

    #[test]
    fn rejects_self_reference_in_declaration() {
        assert_eq!(
            check_body("var x = x"),
            Err(vec![CheckError::Undeclared("x".to_string())])
        );
    }

    #[test]
    fn rejects_redeclaration() {
        for src in [
            "let x = 1\nlet x = 2",
            "var x = 1\nlet x = 2",
            "let x = 1\nvar x = 2",
        ] {
            assert_eq!(
                check_body(src),
                Err(vec![CheckError::Redeclared("x".to_string())]),
                "{src:?}"
            );
        }
    }

    #[test]
    fn rejects_arithmetic_on_booleans() {
        assert_eq!(
            check_body("let x = true + 1"),
            Err(vec![invalid_operand("+")])
        );
        assert_eq!(
            check_body("let x = 1 - false"),
            Err(vec![invalid_operand("-")])
        );
        assert_eq!(check_body("let x = -true"), Err(vec![invalid_operand("-")]));
        assert_eq!(
            check_body("let x = 4 / (true)"),
            Err(vec![invalid_operand("/")])
        );
        assert_eq!(
            check_body("let b = true\nlet x = b * b"),
            Err(vec![invalid_operand("*"), invalid_operand("*")])
        );
    }

    #[test]
    fn rejects_assignment_of_different_type() {
        assert_eq!(
            check_body("var x = 1\nx = true"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "x".to_string(),
                expected: ty("Int"),
                found: ty("Bool"),
            }])
        );
        assert_eq!(
            check_body("var b = false\nb = 1 + 2"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "b".to_string(),
                expected: ty("Bool"),
                found: ty("Int"),
            }])
        );
    }

    #[test]
    fn does_not_cascade_errors() {
        // `x` has no known type, so later uses of it report nothing further.
        assert_eq!(
            check_body("var x = y\nlet z = x + 1\nx = true"),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
        assert_eq!(
            check_body("let x = -true\nlet y = x + 1"),
            Err(vec![invalid_operand("-")])
        );
    }

    #[test]
    fn type_errors_use_type_names() {
        let messages = |src| {
            check_body(src)
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            messages("let x = true + 1"),
            ["operator `+` expects `Int`, found `Bool`"]
        );
        assert_eq!(
            messages("var b = false\nb = 1"),
            ["cannot assign `Int` to `b`, which has type `Bool`"]
        );
        assert_eq!(
            messages("if 1 {}"),
            ["`if` condition must be `Bool`, found `Int`"]
        );
        assert_eq!(messages("let x: int = 1"), ["unknown type `int`"]);
    }

    #[test]
    fn accepts_matching_explicit_types() {
        assert_eq!(
            check_body(
                "
                let x: Int = 3
                var y: Bool = true
                var z: Int = x * 2
                y = !y
                z = 4
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_value_of_wrong_explicit_type() {
        assert_eq!(
            check_body("let x: Int = true"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "x".to_string(),
                expected: ty("Int"),
                found: ty("Bool"),
            }])
        );
        assert_eq!(
            check_body("var y: Bool = 1 + 2"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "y".to_string(),
                expected: ty("Bool"),
                found: ty("Int"),
            }])
        );
    }

    #[test]
    fn rejects_unknown_types() {
        for name in ["Foo", "int", "bool", "x"] {
            assert_eq!(
                check_body(&format!("let x: {name} = 1")),
                Err(vec![CheckError::UnknownType(name.to_string())]),
                "{name:?}"
            );
        }
    }

    #[test]
    fn explicit_type_is_used_after_a_mismatch() {
        // `x` is an `Int` as written, so only the declaration is an error.
        assert_eq!(
            check_body("var x: Int = true\nx = 1\nlet y = x + 1"),
            Err(vec![CheckError::AssignTypeMismatch {
                name: "x".to_string(),
                expected: ty("Int"),
                found: ty("Bool"),
            }])
        );
    }

    #[test]
    fn unknown_type_does_not_cascade() {
        assert_eq!(
            check_body("var x: Foo = 1\nx = true\nlet y = x + 1"),
            Err(vec![CheckError::UnknownType("Foo".to_string())])
        );
    }

    #[test]
    fn redeclaring_std_names_makes_them_ambiguous() {
        // `/main` and `stdlib` are different modules, so both can declare
        // `Int`, but then it must be qualified.
        assert_eq!(
            check_top(
                "
                struct Int {}
                let a: Int = 1
                let b: stdlib.Int = 1
                fn f(x: main.Int) -> stdlib.Bool {
                    return true
                }
                "
            ),
            Err(vec![CheckError::Ambiguous {
                name: "Int".to_string(),
                modules: vec!["/main".to_string(), "stdlib".to_string()],
            }])
        );
    }

    #[test]
    fn functions_can_reuse_std_names() {
        // Like any other outer name.
        assert_eq!(
            check_body(
                "
                fn f(Int: Int) {
                    let Bool = 1
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn std_types_are_structs() {
        assert_eq!(
            check_body(
                "
                struct Wrapper {
                    let int: Int
                    let bool: Bool
                    let void: Void
                }
                fn unwrap(wrapper: Wrapper) -> Wrapper {
                    return wrapper
                }
                "
            ),
            Ok(())
        );
        assert_eq!(
            messages("fn f(flag: Bool) -> Int {\nreturn flag\n}"),
            ["`f` must return `Int`, found `Bool`"]
        );
    }

    #[test]
    fn accepts_if_else() {
        assert_eq!(
            check_body(
                "
                let a = true
                var x = 0
                if a {
                    x = 1
                } else if !a {
                    x = 2
                } else {
                    x = 3
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_non_boolean_condition() {
        assert_eq!(
            check_body("if 1 + 2 {}"),
            Err(vec![CheckError::ConditionNotBool(ty("Int"))])
        );
        assert_eq!(
            check_body("if true {} else if 0 {}"),
            Err(vec![CheckError::ConditionNotBool(ty("Int"))])
        );
        // An undeclared condition is only reported once.
        assert_eq!(
            check_body("if c {}"),
            Err(vec![CheckError::Undeclared("c".to_string())])
        );
    }

    #[test]
    fn checks_statements_inside_blocks() {
        assert_eq!(
            check_body(
                "
                let x = 1
                if true {
                    x = 2
                } else {
                    let y = z
                }
                "
            ),
            Err(vec![
                CheckError::AssignToConstant("x".to_string()),
                CheckError::Undeclared("z".to_string()),
            ])
        );
    }

    #[test]
    fn blocks_can_use_and_assign_outer_names() {
        assert_eq!(
            check_body(
                "
                let a = 1
                var b = 2
                if true {
                    if true {
                        b = a + b
                    }
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn block_declarations_are_not_visible_outside() {
        assert_eq!(
            check_body(
                "
                if true {
                    let y = 1
                }
                let z = y
                "
            ),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
        assert_eq!(
            check_body(
                "
                if true {
                    let y = 1
                } else {
                    y = 2
                }
                "
            ),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
    }

    #[test]
    fn separate_blocks_can_reuse_names() {
        assert_eq!(
            check_body(
                "
                if true {
                    let y = 1
                } else {
                    let y = false
                }
                let y = 2
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_shadowing_outer_names() {
        assert_eq!(
            check_body(
                "
                let x = 1
                if true {
                    let x = 2
                }
                "
            ),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
    }

    #[test]
    fn reports_every_error() {
        assert_eq!(
            check_body("let x = 1\nx = 2\ny = z"),
            Err(vec![
                CheckError::AssignToConstant("x".to_string()),
                CheckError::Undeclared("z".to_string()),
                CheckError::Undeclared("y".to_string()),
            ])
        );
    }

    fn messages(src: &str) -> Vec<String> {
        match check_body(src) {
            Ok(()) => vec![],
            Err(errs) => errs.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn accepts_function_definition_and_calls() {
        assert_eq!(
            check_body(
                "
                fn addFive(to value: Int) -> Int {
                    return value + 5
                }
                let x: Int = addFive(to: 3)
                let y = addFive(to: addFive(to: x))
                addFive(to: 1) // The result can be ignored.
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_all_label_forms() {
        assert_eq!(
            check_body(
                "
                fn f(a: Int, to b: Bool, _ c: Int) -> Bool {
                    return b & !b | b
                }
                let r: Bool = f(a: 1, to: true, 2)
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_function_without_return_type() {
        assert_eq!(
            check_body(
                "
                fn reset() {
                    var count = 1
                    count = 0
                    return
                }
                reset()
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn functions_cannot_capture_values() {
        assert_eq!(
            messages(
                "
                let step = 2
                var total = 0
                fn add(amount: Int) {
                    total = total + amount * step
                }
                add(amount: 3)
                "
            ),
            [
                "functions can't use `total`, because it is declared outside the function",
                "functions can't use `step`, because it is declared outside the function",
                "functions can't use `total`, because it is declared outside the function",
            ]
        );
        // Not even from an enclosing function, or a block around them.
        assert_eq!(
            check_body(
                "
                fn outer(x: Int) {
                    let y = 1
                    fn inner() -> Int {
                        return x + y
                    }
                }
                if true {
                    let z = 1
                    fn f() -> Int {
                        return z
                    }
                }
                "
            ),
            Err(vec![
                CheckError::Captured("x".to_string()),
                CheckError::Captured("y".to_string()),
                CheckError::Captured("z".to_string()),
            ])
        );
    }

    #[test]
    fn functions_can_use_outer_functions_and_types() {
        assert_eq!(
            check_body(
                "
                fn outer() {
                    struct Point {}
                    fn helper(point: Point) {}
                    fn inner(point: Point) {
                        helper(point: point)
                        outer()
                    }
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn functions_can_recurse() {
        assert_eq!(
            check_body(
                "
                fn sum(upTo n: Int, acc: Int) -> Int {
                    return sum(upTo: n - 1, acc: acc + n)
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn top_level_functions_can_be_used_in_any_order() {
        assert_eq!(
            check_top(
                "
                let x: Int = f()
                fn f() -> Int {
                    return g(flag: true)
                }
                fn g(flag: Bool) -> Int {
                    if flag {
                        return f()
                    }
                    return 1
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn nested_functions_cannot_be_called_before_declaration() {
        for src in [
            "
            fn outer() {
                let x = f()
                fn f() -> Int {
                    return 1
                }
            }
            ",
            "
            if true {
                let x = f()
                fn f() -> Int {
                    return 1
                }
            }
            ",
        ] {
            assert_eq!(
                check_body(src),
                Err(vec![CheckError::Undeclared("f".to_string())]),
                "{src:?}"
            );
        }
    }

    #[test]
    fn parameters_are_constants() {
        assert_eq!(
            check_body(
                "
                fn f(x: Int) {
                    x = 2
                }
                "
            ),
            Err(vec![CheckError::AssignToConstant("x".to_string())])
        );
    }

    #[test]
    fn function_scope_is_not_visible_outside() {
        assert_eq!(
            check_body(
                "
                fn f(x: Int) {
                    let y = x
                }
                let a = x
                let b = y
                "
            ),
            Err(vec![
                CheckError::Undeclared("x".to_string()),
                CheckError::Undeclared("y".to_string()),
            ])
        );
    }

    #[test]
    fn functions_can_reuse_outer_names() {
        assert_eq!(
            check_body(
                "
                let x = true
                fn f(x: Int) -> Int {
                    let y = x + 1
                    return y
                }
                let y = false
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_redeclaration_within_function() {
        assert_eq!(
            check_body("fn f(x: Int, x: Int) {}"),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
        assert_eq!(
            check_body(
                "
                fn f(x: Int) {
                    let x = 1
                }
                "
            ),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
        // Blocks inside a function still can't shadow.
        assert_eq!(
            check_body(
                "
                fn f(x: Int) {
                    if true {
                        let x = 1
                    }
                }
                "
            ),
            Err(vec![CheckError::Redeclared("x".to_string())])
        );
        // Functions share names with values.
        assert_eq!(
            check_body("let f = 1\nfn f() {}"),
            Err(vec![CheckError::Redeclared("f".to_string())])
        );
    }

    #[test]
    fn rejects_wrong_argument_count() {
        assert_eq!(
            messages("fn f(a: Int) {}\nf()\nf(a: 1, a: 2)"),
            [
                "`f` takes 1 argument, found 0",
                "`f` takes 1 argument, found 2",
            ]
        );
        assert_eq!(
            messages("fn g() {}\ng(1)"),
            ["`g` takes 0 arguments, found 1"]
        );
    }

    #[test]
    fn rejects_wrong_labels() {
        assert_eq!(
            messages(
                "
                fn f(to value: Int, _ other: Int) {}
                f(from: 1, 2)
                f(1, 2)
                f(to: 1, other: 2)
                f(value: 1, 2)
                "
            ),
            [
                "argument to `f` should be labelled `to`, found `from`",
                "argument to `f` is missing the label `to`",
                "argument to `f` should not have a label, found `other`",
                "argument to `f` should be labelled `to`, found `value`",
            ]
        );
    }

    #[test]
    fn labels_must_be_in_order() {
        assert_eq!(
            messages("fn f(a: Int, b: Int) {}\nf(b: 1, a: 2)"),
            [
                "argument to `f` should be labelled `a`, found `b`",
                "argument to `f` should be labelled `b`, found `a`",
            ]
        );
    }

    #[test]
    fn rejects_wrong_argument_types() {
        assert_eq!(
            messages("fn f(to value: Int, _ flag: Bool) {}\nf(to: true, 1)"),
            [
                "argument `to` of `f` expects `Int`, found `Bool`",
                "argument `flag` of `f` expects `Bool`, found `Int`",
            ]
        );
    }

    #[test]
    fn checks_arguments_even_when_call_is_wrong() {
        assert_eq!(
            check_body("fn f() {}\nf(x)\nlet y = 1\ny(z)"),
            Err(vec![
                CheckError::Undeclared("x".to_string()),
                CheckError::WrongArgumentCount {
                    function: "f".to_string(),
                    expected: 0,
                    found: 1,
                },
                CheckError::Undeclared("z".to_string()),
                CheckError::NotAFunction("y".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_using_functions_as_values() {
        assert_eq!(
            messages("fn f() {}\nlet x = f\nf = 2"),
            [
                "`f` is a function, not a value",
                "`f` is a function, not a value",
            ]
        );
    }

    #[test]
    fn rejects_calling_undeclared_or_non_functions() {
        assert_eq!(
            messages("let x = 1\nx()\ng()"),
            ["`x` is not a function", "`g` is not declared"]
        );
    }

    #[test]
    fn functions_without_return_type_return_void() {
        assert_eq!(
            messages(
                "
                fn f() {}
                let x = f()
                let y: Void = x
                let z = 1 + f()
                let w: Int = f()
                if f() {}
                "
            ),
            [
                "operator `+` expects `Int`, found `Void`",
                "cannot assign `Void` to `w`, which has type `Int`",
                "`if` condition must be `Bool`, found `Void`",
            ]
        );
    }

    #[test]
    fn return_type_is_used_by_callers() {
        assert_eq!(
            messages(
                "
                fn f() -> Bool {
                    return true
                }
                let x = f() + 1
                let y: Int = f()
                "
            ),
            [
                "operator `+` expects `Int`, found `Bool`",
                "cannot assign `Bool` to `y`, which has type `Int`",
            ]
        );
    }

    #[test]
    fn rejects_wrong_return_values() {
        assert_eq!(
            messages(
                "
                fn a() -> Int {
                    return true
                }
                fn b() -> Int {
                    return
                }
                fn c() {
                    return 1
                }
                "
            ),
            [
                "`a` must return `Int`, found `Bool`",
                "`b` must return a value of type `Int`",
                "`c` must return `Void`, found `Int`",
            ]
        );
    }

    #[test]
    fn explicit_void_is_the_same_as_no_return_type() {
        assert_eq!(
            check_body(
                "
                fn a() -> Void {}
                fn b() {}
                fn c(flag: Bool) -> Void {
                    if flag {
                        return
                    }
                }
                let x: Void = a()
                let y: Void = b()
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn void_functions_can_return_void_values() {
        assert_eq!(
            check_body(
                "
                fn log() {}
                fn wrapper() -> Void {
                    return log()
                }
                fn other() {
                    return wrapper()
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn void_can_be_used_like_other_types() {
        assert_eq!(
            check_body(
                "
                fn take(value: Void) -> Void {
                    return value
                }
                fn nothing() {}
                take(value: nothing())
                "
            ),
            Ok(())
        );
        assert_eq!(
            messages("fn take(value: Void) {}\ntake(value: 1)"),
            ["argument `value` of `take` expects `Void`, found `Int`"]
        );
    }

    #[test]
    fn non_void_function_cannot_return_void() {
        assert_eq!(
            messages(
                "
                fn log() {}
                fn f() -> Int {
                    return log()
                }
                "
            ),
            ["`f` must return `Int`, found `Void`"]
        );
    }

    #[test]
    fn requires_return_on_every_path() {
        let missing = |function: &str| CheckError::MissingReturn {
            function: function.to_string(),
            expected: ty("Int"),
        };

        assert_eq!(check_body("fn f() -> Int {}"), Err(vec![missing("f")]));
        assert_eq!(
            check_body(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        return 1
                    }
                }
                "
            ),
            Err(vec![missing("f")])
        );
        assert_eq!(
            check_body(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        return 1
                    } else if !c {
                        return 2
                    }
                }
                "
            ),
            Err(vec![missing("f")])
        );
    }

    #[test]
    fn accepts_return_on_every_path() {
        assert_eq!(
            check_body(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        return 1
                    } else if !c {
                        return 2
                    } else {
                        return 3
                    }
                }
                fn g(c: Bool) -> Int {
                    if c {
                        return 1
                    }
                    return 2
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn structs_can_be_used_as_types() {
        assert_eq!(
            check_body(
                "
                struct User {
                    let age: Int
                    var isAdmin: Bool
                }
                struct Post {
                    let author: User
                }
                fn pick(first a: User, or b: User) -> User {
                    let user: User = a
                    var other = b
                    other = user
                    return other
                }
                fn repost(post: Post) -> Post {
                    return repost(post: post)
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_empty_and_void_structs() {
        assert_eq!(
            check_body(
                "
                struct Empty {}
                struct Nothing {
                    let value: Void
                    let empty: Empty
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn type_errors_use_struct_names() {
        assert_eq!(
            messages(
                "
                struct User {}
                fn f(user: User) -> User {
                    let x: Int = user
                    let y = user + 1
                    if user {}
                    f(user: 1)
                    return true
                }
                "
            ),
            [
                "cannot assign `User` to `x`, which has type `Int`",
                "operator `+` expects `Int`, found `User`",
                "`if` condition must be `Bool`, found `User`",
                "argument `user` of `f` expects `User`, found `Int`",
                "`f` must return `User`, found `Bool`",
            ]
        );
    }

    #[test]
    fn different_structs_are_different_types() {
        assert_eq!(
            messages(
                "
                struct A {
                    let x: Int
                }
                struct B {
                    let x: Int
                }
                fn f(a: A) -> B {
                    return a
                }
                "
            ),
            ["`f` must return `B`, found `A`"]
        );
    }

    #[test]
    fn structs_with_the_same_name_are_different_types() {
        // The inner `A` is a different struct, even though it has the same
        // name and fields.
        assert_eq!(
            messages(
                "
                struct A {}
                fn f(a: A) {
                    struct A {}
                    fn g(b: A) {}
                    g(b: a)
                }
                "
            ),
            ["argument `b` of `g` expects `A`, found `A`"]
        );
    }

    #[test]
    fn rejects_unknown_field_types() {
        assert_eq!(
            check_body(
                "
                struct User {
                    let age: int
                    var friend: Person
                }
                "
            ),
            Err(vec![
                CheckError::UnknownType("int".to_string()),
                CheckError::UnknownType("Person".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_duplicate_fields() {
        assert_eq!(
            check_body(
                "
                struct User {
                    let age: Int
                    var age: Bool
                }
                "
            ),
            Err(vec![CheckError::Redeclared("age".to_string())])
        );
    }

    #[test]
    fn fields_can_reuse_outer_names() {
        assert_eq!(
            check_body(
                "
                let age = 1
                struct A {
                    let age: Int
                }
                struct B {
                    let age: Int
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn top_level_structs_can_be_used_in_any_order() {
        assert_eq!(
            check_top(
                "
                fn f(user: User) -> Post {
                    return f(user: user)
                }
                struct Post {
                    let author: User
                }
                struct User {
                    let address: Address
                }
                struct Address {}
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn nested_structs_must_be_declared_before_use() {
        assert_eq!(
            check_body(
                "
                fn outer() {
                    fn f(user: User) {}
                    struct Post {
                        let author: User
                    }
                    struct User {}
                }
                "
            ),
            Err(vec![
                CheckError::UnknownType("User".to_string()),
                CheckError::UnknownType("User".to_string()),
            ])
        );
    }

    #[test]
    fn structs_cannot_contain_themselves() {
        assert_eq!(
            check_body(
                "
                struct Node {
                    let next: Node
                }
                fn f() {
                    struct Inner {
                        let inner: Inner
                    }
                }
                "
            ),
            Err(vec![
                CheckError::RecursiveStruct("Node".to_string()),
                CheckError::RecursiveStruct("Inner".to_string()),
            ])
        );
        // Every struct in a cycle is reported, but not one that only contains
        // a struct in a cycle.
        assert_eq!(
            check_top(
                "
                struct A {
                    let b: B
                }
                struct B {
                    let c: C
                }
                struct C {
                    let a: A
                }
                struct D {
                    let a: A
                }
                "
            ),
            Err(vec![
                CheckError::RecursiveStruct("A".to_string()),
                CheckError::RecursiveStruct("B".to_string()),
                CheckError::RecursiveStruct("C".to_string()),
            ])
        );
    }

    #[test]
    fn structs_can_contain_the_same_struct_twice() {
        assert_eq!(
            check_top(
                "
                struct Line {
                    let start: Point
                    let end: Point
                }
                struct Point {}
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_redeclared_structs() {
        assert_eq!(
            check_body("struct A {}\nstruct A {}"),
            Err(vec![CheckError::Redeclared("A".to_string())])
        );
        // A function can declare its own `Int`, which is a different type,
        // but literals still have the standard library's `Int`.
        assert_eq!(
            messages(
                "
                fn f() {
                    struct Int {}
                    let x: Int = 1
                }
                "
            ),
            ["cannot assign `Int` to `x`, which has type `Int`"]
        );
    }

    #[test]
    fn structs_share_names_with_values_and_functions() {
        for src in [
            "struct User {}\nlet User = 1",
            "let User = 1\nstruct User {}",
            "struct User {}\nfn User() {}",
            "fn User() {}\nstruct User {}",
            "fn f(User: Int) {\nstruct User {}\n}",
        ] {
            assert_eq!(
                check_body(src),
                Err(vec![CheckError::Redeclared("User".to_string())]),
                "{src:?}"
            );
        }
    }

    #[test]
    fn rejects_using_types_as_values() {
        assert_eq!(
            messages(
                "
                struct User {}
                let x = User
                User = 1
                let y = Int + 1
                User()
                "
            ),
            [
                "`User` is a type, not a value",
                "`User` is a type, not a value",
                "`Int` is a type, not a value",
                "`User` is not a function",
            ]
        );
    }

    #[test]
    fn rejects_using_values_and_functions_as_types() {
        assert_eq!(
            messages(
                "
                let x = 1
                let y: x = 1
                fn f() {}
                fn g(a: f) {}
                fn h(a: Int) {
                    let b: a = 1
                    let c: x = 1
                    struct S {
                        let field: f
                    }
                }
                "
            ),
            [
                "`x` is not a type",
                "`f` is not a type",
                "`a` is not a type",
                "`x` is not a type",
                "`f` is not a type",
            ]
        );
    }

    #[test]
    fn structs_are_scoped_like_other_names() {
        assert_eq!(
            check_body(
                "
                if true {
                    struct A {}
                    fn f(a: A) {}
                }
                fn g(a: A) {}
                "
            ),
            Err(vec![CheckError::UnknownType("A".to_string())])
        );
        assert_eq!(
            check_body(
                "
                fn f() {
                    struct A {}
                    fn g(a: A) {}
                }
                fn h(a: A) {}
                "
            ),
            Err(vec![CheckError::UnknownType("A".to_string())])
        );
        // Separate blocks can reuse names.
        assert_eq!(
            check_body(
                "
                if true {
                    struct A {}
                } else {
                    struct A {}
                }
                "
            ),
            Ok(())
        );
        // Blocks can't shadow, but functions can.
        assert_eq!(
            check_body(
                "
                struct A {}
                if true {
                    struct A {}
                }
                fn f() {
                    struct A {}
                }
                "
            ),
            Err(vec![CheckError::Redeclared("A".to_string())])
        );
    }

    #[test]
    fn nested_functions_have_their_own_return_type() {
        assert_eq!(
            check_body(
                "
                fn outer() -> Bool {
                    fn inner() -> Int {
                        return 1
                    }
                    // `inner`'s `return` is checked against `Int`, and this
                    // one against `Bool`.
                    return inner()
                }
                "
            ),
            Err(vec![CheckError::ReturnTypeMismatch {
                function: "outer".to_string(),
                expected: ty("Bool"),
                found: ty("Int"),
            }])
        );
    }

    #[test]
    fn checks_the_standard_library_alone() {
        assert_eq!(check(&[]), Ok(()));
    }

    #[test]
    fn imports_every_top_level_name() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /lib
                    use ./dir/other

                    let total: Int = limit + 1
                    fn f(point: Point) -> Point {
                        return identity(of: point)
                    }
                    ",
                ),
                ("/lib", "let limit = 10"),
                (
                    "/dir/other",
                    "
                    struct Point {}
                    fn identity(of point: Point) -> Point {
                        return point
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn relative_paths_start_from_the_current_directory() {
        assert_eq!(
            check_files(&[
                ("/dir/a", "use ./b\nuse ./sub/c\nlet x = b + c"),
                ("/dir/b", "let b = 1"),
                ("/dir/sub/c", "let c = 2"),
                // Not found by `./b`, since `/dir/a` isn't at the root.
                ("/b", "let b = true"),
            ]),
            Ok(())
        );
        assert_eq!(
            file_messages(&[("/dir/a", "use ./dir/b"), ("/dir/b", "")]),
            ["/dir/a: cannot find module `./dir/b`"]
        );
    }

    #[test]
    fn rejects_unknown_modules() {
        assert_eq!(
            file_messages(&[("/main", "use /missing\nuse ./missing\nuse other")]),
            [
                "/main: cannot find module `/missing`",
                "/main: cannot find module `./missing`",
                "/main: cannot find module `other`",
            ]
        );
    }

    #[test]
    fn names_need_importing() {
        assert_eq!(
            file_messages(&[
                ("/main", "let a = x\nlet b = lib.x\nlet c = /lib.x"),
                ("/lib", "let x = 1"),
            ]),
            [
                "/main: `x` is not declared",
                "/main: `lib` is not an imported module",
                "/main: `/lib` is not an imported module",
            ]
        );
    }

    #[test]
    fn imports_are_not_transitive() {
        assert_eq!(
            file_messages(&[
                ("/main", "use /a\nlet x = y"),
                ("/a", "use /b"),
                ("/b", "let y = 1"),
            ]),
            ["/main: `y` is not declared"]
        );
    }

    #[test]
    fn importing_twice_or_importing_itself_is_harmless() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /main
                    use ./main
                    use /lib
                    use ./lib
                    use stdlib
                    let x: Int = y + 1
                    let z = x
                    ",
                ),
                ("/lib", "let y = 1"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn names_from_several_modules_must_be_qualified() {
        let a = "fn f() -> Int {\nreturn 1\n}";
        let b = "fn f() -> Bool {\nreturn true\n}";
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /a
                    use /dir/b
                    let x = f()
                    let y: Int = a.f()
                    let z: Bool = b.f()
                    let w: Bool = /dir/b.f()
                    ",
                ),
                ("/a", a),
                ("/dir/b", b),
            ]),
            ["/main: `f` is declared in `/a` and `/dir/b`, so it must be qualified"]
        );
        // Including when the module itself declares the name.
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /a
                    fn f() -> Bool {
                        return f()
                    }
                    let x: Int = a.f()
                    let y: Bool = main.f()
                    ",
                ),
                ("/a", a),
            ]),
            ["/main: `f` is declared in `/main` and `/a`, so it must be qualified"]
        );
    }

    #[test]
    fn modules_with_the_same_name_need_full_paths() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /a/other
                    use /b/other
                    let x = other.value
                    let y: Int = /a/other.value
                    let z: Bool = ./b/other.value
                    ",
                ),
                ("/a/other", "let value = 1"),
                ("/b/other", "let value = true"),
            ]),
            ["/main: `other` could be `/a/other` or `/b/other`, so the full path must be used"]
        );
    }

    #[test]
    fn qualified_names_must_exist() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "use /lib\nlet x = lib.missing\nlet y: lib.Missing = 1",
                ),
                ("/lib", ""),
            ]),
            [
                "/main: `lib.missing` is not declared",
                "/main: unknown type `lib.Missing`",
            ]
        );
    }

    #[test]
    fn std_names_can_be_qualified() {
        assert_eq!(
            check_top("let x: stdlib.Int = 1\nlet y: Int = x\nlet z: stdlib.Bool = true"),
            Ok(())
        );
    }

    #[test]
    fn modules_can_import_each_other() {
        assert_eq!(
            check_files(&[
                (
                    "/a",
                    "
                    use /b
                    struct A {
                        let b: B
                    }
                    fn f(b: B) -> Int {
                        return g(b: b)
                    }
                    let x = y + 1
                    ",
                ),
                (
                    "/b",
                    "
                    use /a
                    struct B {}
                    fn g(b: B) -> Int {
                        return 1
                    }
                    let y = 2
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn structs_cannot_contain_themselves_across_modules() {
        assert_eq!(
            file_messages(&[
                ("/a", "use /b\nstruct A {\nlet b: B\n}"),
                ("/b", "use /a\nstruct B {\nlet a: A\n}"),
            ]),
            ["/a: `A` contains itself", "/b: `B` contains itself"]
        );
    }

    #[test]
    fn top_level_values_can_be_used_in_any_order() {
        assert_eq!(
            check_top("let a: Int = b + c\nlet b = c * 2\nlet c = 1"),
            Ok(())
        );
        // Their types are known wherever they're used first.
        assert_eq!(
            messages_top("let a = b + 1\nlet b = true"),
            ["operator `+` expects `Int`, found `Bool`"]
        );
    }

    #[test]
    fn rejects_values_that_depend_on_themselves() {
        assert_eq!(
            check_top("let a = b\nlet b = a + 1\nvar c: Int = c"),
            Err(vec![
                CheckError::CyclicValue("a".to_string()),
                CheckError::CyclicValue("c".to_string()),
            ])
        );
        assert_eq!(
            file_messages(&[
                ("/main", "use /other\nlet a = b"),
                ("/other", "use /main\nlet b = a"),
            ]),
            ["/main: `a` depends on its own value"]
        );
    }

    #[test]
    fn functions_cannot_use_top_level_values() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /lib
                    let a = 1
                    fn f() -> Int {
                        return a + b + lib.b
                    }
                    // So a value can't depend on itself through a function.
                    let c = g()
                    fn g() -> Int {
                        return c
                    }
                    ",
                ),
                ("/lib", "let b = 2"),
            ]),
            [
                "/main: functions can't use `a`, because it is declared outside the function",
                "/main: functions can't use `b`, because it is declared outside the function",
                "/main: functions can't use `lib.b`, because it is declared outside the function",
                "/main: functions can't use `c`, because it is declared outside the function",
            ]
        );
    }

    #[test]
    fn rejects_redeclaration_within_a_module() {
        assert_eq!(
            check_top("struct a {}\nfn a() {}\nlet a = 1"),
            Err(vec![
                CheckError::Redeclared("a".to_string()),
                CheckError::Redeclared("a".to_string()),
            ])
        );
    }

    #[test]
    fn errors_name_their_module() {
        assert_eq!(
            file_messages(&[("/main", "let x = 1"), ("/dir/other", "let y = z"),]),
            ["/dir/other: `z` is not declared"]
        );
    }

    const USER: &str = "
        struct User {
            let age: Int
            var isAdmin: Bool
        }
    ";

    #[test]
    fn constructs_structs() {
        assert_eq!(
            check_top(&format!(
                "
                {USER}
                struct Team {{
                    let lead: User
                    let size: Int
                }}
                struct Empty {{}}

                let admin: User = User {{ age: 42, isAdmin: true }}
                let team = Team {{ lead: User {{ age: 30, isAdmin: false }}, size: 1 + 2 }}
                let empty = Empty {{}}

                fn promote(user: User) -> User {{
                    return User {{ age: 18, isAdmin: true }}
                }}
                fn newTeam() -> Team {{
                    let lead = promote(user: User {{ age: 20, isAdmin: false }})
                    return Team {{ lead: lead, size: 1 }}
                }}
                "
            )),
            Ok(())
        );
    }

    #[test]
    fn constructors_give_the_struct_type() {
        assert_eq!(
            messages(&format!(
                "
                {USER}
                let x: Int = User {{ age: 1, isAdmin: true }}
                let y = User {{ age: 1, isAdmin: true }} + 1
                "
            )),
            [
                "cannot assign `User` to `x`, which has type `Int`",
                "operator `+` expects `Int`, found `User`",
            ]
        );
    }

    #[test]
    fn constructors_need_every_field() {
        assert_eq!(
            messages(&format!(
                "
                {USER}
                let a = User {{ age: 1 }}
                let b = User {{ age: 1, isAdmin: true, extra: 2 }}
                let c = User {{}}
                "
            )),
            [
                "`User` has 2 fields, found 1",
                "`User` has 2 fields, found 3",
                "`User` has 2 fields, found 0",
            ]
        );
    }

    #[test]
    fn constructor_fields_must_be_in_order() {
        assert_eq!(
            messages(&format!(
                "
                {USER}
                let a = User {{ isAdmin: true, age: 1 }}
                let b = User {{ age: 1, admin: true }}
                "
            )),
            [
                "expected field `age` of `User`, found `isAdmin`",
                "expected field `isAdmin` of `User`, found `age`",
                "expected field `isAdmin` of `User`, found `admin`",
            ]
        );
    }

    #[test]
    fn constructor_fields_are_type_checked() {
        assert_eq!(
            messages(&format!(
                "
                {USER}
                let a = User {{ age: true, isAdmin: 1 }}
                let b = User {{ age: -true, isAdmin: false }}
                "
            )),
            [
                "field `age` of `User` expects `Int`, found `Bool`",
                "field `isAdmin` of `User` expects `Bool`, found `Int`",
                // The `-` makes `age` an `Int`, so it's only reported once.
                "operator `-` expects `Int`, found `Bool`",
            ]
        );
    }

    #[test]
    fn constructors_need_a_struct() {
        assert_eq!(
            messages(
                "
                let x = 1
                fn f() {}
                let a = Missing { value: y }
                let b = x {}
                let c = f {}
                "
            ),
            [
                "`y` is not declared",
                "unknown type `Missing`",
                "`x` is not a type",
                "`f` is not a type",
            ]
        );
    }

    #[test]
    fn unknown_field_types_accept_any_value() {
        assert_eq!(
            messages(
                "
                struct Box {
                    let value: Missing
                }
                let b = Box { value: 1 }
                "
            ),
            ["unknown type `Missing`"]
        );
    }

    #[test]
    fn constructors_work_across_modules() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use ./users/user
                    let a: User = User { age: 1, isAdmin: true }
                    let b: user.User = user.User { age: 2, isAdmin: false }
                    fn f() -> User {
                        return /users/user.User { age: 3, isAdmin: true }
                    }
                    ",
                ),
                ("/users/user", USER),
            ]),
            Ok(())
        );
    }

    #[test]
    fn std_types_can_be_constructed() {
        assert_eq!(
            check_body("let v: Void = Void {}\nlet b: stdlib.Bool = stdlib.Bool {}"),
            Ok(())
        );
    }

    #[test]
    fn constructor_values_follow_capture_rules() {
        assert_eq!(
            file_messages(&[(
                "/main",
                &format!(
                    "
                    {USER}
                    let age = 1
                    let a = User {{ age: age, isAdmin: true }}
                    fn f() -> User {{
                        return User {{ age: age, isAdmin: true }}
                    }}
                    // A value can't contain itself.
                    let b: User = User {{ age: b, isAdmin: true }}
                    "
                ),
            )]),
            [
                "/main: functions can't use `age`, because it is declared outside the function",
                "/main: `b` depends on its own value",
            ]
        );
    }
}
