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
//! reassigned, that functions are called and return correctly, that groups
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
//! A module's own top-level names come before the names it imports, which
//! must not clash with each other.
//!
//! The standard library is a package called `stdlib`, whose files are modules
//! named from the package, such as `stdlib/core`. Every program imports
//! `stdlib/core` without needing a `use`, but only the standard library itself
//! can import its other files. Only the standard library can declare
//! primitive types, such as `Int`, which the compiler represents itself, and
//! whose values only come from literals and operators.
//!
//! As it checks, the checker lowers the AST into an `ir::Program`. That program
//! is only returned when there are no errors, so everything in it is known.

use std::{
    collections::{HashMap, HashSet},
    fmt, iter,
    sync::LazyLock,
};

use super::{
    ast::{
        Arg, BinOp, ChoiceOption, Expr, Field, FieldValue, Function, ImplFunction, Import,
        MatchOption, ModulePath, Mutability, Name, Param, Program, Stmt, UnOp,
    },
    parser::parse,
};
use crate::ir;

/// The package holding the standard library.
const STDLIB_PACKAGE: &str = "stdlib";

/// The files of the standard library, by their paths within it.
static STDLIB: LazyLock<Vec<Module>> = LazyLock::new(|| {
    [("core", include_str!("../stdlib/core.atom"))]
        .into_iter()
        .map(|(name, src)| Module {
            path: vec![name.to_string()],
            program: parse(src)
                .unwrap_or_else(|errs| panic!("`stdlib/{name}` should parse: {errs:?}")),
        })
        .collect()
});

/// A parsed `.atom` file.
#[derive(Debug, Clone)]
pub struct Module {
    /// The file's path from the project root without `.atom`, such as
    /// `["dir", "other"]` for `dir/other.atom`.
    pub path: Vec<String>,
    pub program: Program,
}

/// Every type is a group or a choice. `Int`, `Bool` and `Void` are groups
/// declared in the standard library. Types in different scopes can share a
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
    /// A `use` of a directory found no modules that could be imported.
    EmptyDirectory(String),
    /// A package was used where one of its modules was needed.
    PackageNotModule(String),
    /// A module was imported from outside its package, which doesn't export
    /// it.
    PrivateModule { module: String, package: String },
    /// A primitive type was constructed, such as `Int`, whose values come
    /// from literals.
    ConstructedPrimitive(Type),
    /// A primitive type was declared outside the standard library.
    PrimitiveOutsideStdlib(String),
    /// A group was constructed outside the `impl`s of its type.
    ConstructedOutsideImpl(Type),
    /// A group was constructed outside the module that declares it.
    ConstructedOutsideModule { ty: Type, module: String },
    /// `::` was used after a name that's declared, but isn't a module.
    NotAModule(String),
    /// A qualified name's module matched more than one module by name, so
    /// its full path is needed.
    AmbiguousModule { name: String, modules: Vec<String> },
    /// A top-level value's initialiser depends on the value itself.
    CyclicValue(String),
    /// A group or choice contains itself, directly or through other types,
    /// so it would be infinitely large.
    RecursiveType(String),
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
    /// A comparison's left operand has a type that can't be compared.
    IncomparableOperand { op: &'static str, found: Type },
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
    /// A group was constructed with the wrong number of fields.
    WrongFieldCount {
        ty: String,
        expected: usize,
        found: usize,
    },
    /// A group was constructed with a field that isn't the next one it
    /// declares, either because the name is wrong or it's out of order.
    WrongField {
        ty: String,
        expected: String,
        found: String,
    },
    /// A group was constructed with a field value of the wrong type.
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
    /// A field or method was used that the receiver's type doesn't have.
    NoMember { ty: Type, member: String },
    /// A static function was used that the type doesn't have.
    NoStaticFunction { ty: Type, function: String },
    /// An option or static function was used that the choice doesn't have.
    NoOption { ty: Type, option: String },
    /// An option holding a value other than `Void` was used without one.
    OptionNeedsValue {
        ty: Type,
        option: String,
        expected: Type,
    },
    /// A choice was constructed like a group, rather than from an option.
    ConstructedChoice(Type),
    /// A `match` was used on a value that isn't a choice.
    MatchNotChoice(Type),
    /// A `match` names an option of a different type to the matched value's.
    WrongPatternType { expected: Type, found: Type },
    /// A `match` names an option that the choice doesn't have.
    UnknownOption { ty: Type, option: String },
    /// A `match` matches an option more than once, such as `Shape.circle`.
    DuplicateOption(String),
    /// A `match` doesn't match some options, such as `Shape.circle`.
    MissingOptions(Vec<String>),
    /// A field was called like a method.
    FieldNotMethod { ty: Type, field: String },
    /// A method or static function was used without calling it.
    MemberNotValue {
        ty: Type,
        member: String,
        kind: MemberKind,
    },
    /// A static function or option was used on a value, rather than its
    /// type.
    NeedsType {
        ty: Type,
        member: String,
        kind: MemberKind,
    },
    /// A field or method was used on a type, rather than a value of it.
    NeedsValue {
        ty: Type,
        member: String,
        kind: MemberKind,
    },
    /// `.` was used after a module's name, where `::` was probably meant.
    ModuleNotValue { module: String, member: String },
    /// A method or static function was called from a module that doesn't
    /// import the module declaring it.
    MemberNotImported {
        ty: Type,
        member: String,
        module: String,
    },
    /// A member was declared with the same name as another member of its
    /// type that it shares names with. Fields share names with methods, and
    /// options share names with static functions.
    MemberRedeclared {
        ty: Type,
        member: String,
        existing: MemberKind,
    },
    /// `self` was used outside the methods of an `impl`.
    SelfOutsideImpl,
    /// `self` was used in a static function, which isn't called on a value.
    SelfInStatic,
}

/// What kind of member of a type something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Field,
    Method,
    StaticFunction,
    /// An option of a choice.
    Option,
}

impl MemberKind {
    /// The kind with "a" or "an" before it, such as "an option".
    fn with_article(self) -> String {
        match self {
            Self::Option => format!("an {self}"),
            _ => format!("a {self}"),
        }
    }
}

impl fmt::Display for MemberKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Field => write!(f, "field"),
            Self::Method => write!(f, "method"),
            Self::StaticFunction => write!(f, "static function"),
            Self::Option => write!(f, "option"),
        }
    }
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
            Self::EmptyDirectory(import) => {
                write!(
                    f,
                    "`{import}` doesn't match any modules that can be imported"
                )
            }
            Self::PackageNotModule(package) => write!(
                f,
                "`{package}` is a package, so use one of its modules, such as `{package}/core`"
            ),
            Self::PrivateModule { module, package } => write!(
                f,
                "`{module}` can only be imported from inside the `{package}` package"
            ),
            Self::ConstructedOutsideModule { ty, module } => write!(
                f,
                "`{ty}` can only be constructed in `{module}`, which declares it"
            ),
            Self::ConstructedOutsideImpl(ty) => {
                write!(f, "`{ty}` can only be constructed inside an `impl {ty}`")
            }
            Self::ConstructedPrimitive(ty) => write!(
                f,
                "`{ty}` is a primitive, so it can only be created by the compiler, such as with a literal"
            ),
            Self::PrimitiveOutsideStdlib(name) => write!(
                f,
                "`{name}` can't be a primitive, since only the standard library can declare them"
            ),
            Self::NotAModule(name) => write!(
                f,
                "`{name}` is not a module, so it can't be followed by `::`"
            ),
            Self::AmbiguousModule { name, modules } => write!(
                f,
                "`{name}` could be {}, so the full path must be used",
                quoted_list(modules, "or")
            ),
            Self::CyclicValue(name) => write!(f, "`{name}` depends on its own value"),
            Self::RecursiveType(name) => write!(f, "`{name}` contains itself"),
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
            Self::IncomparableOperand { op, found } => {
                write!(
                    f,
                    "operator `{op}` expects `Int` or `Bool`, found `{found}`"
                )
            }
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
            Self::NoStaticFunction { ty, function } => {
                write!(f, "`{ty}` has no static function `{function}`")
            }
            Self::NoOption { ty, option } => {
                write!(f, "`{ty}` has no option or static function `{option}`")
            }
            Self::MatchNotChoice(ty) => write!(f, "`match` needs a choice, found `{ty}`"),
            Self::WrongPatternType { expected, found } => write!(
                f,
                "`match` is on `{expected}`, so it can't match an option of `{found}`"
            ),
            Self::UnknownOption { ty, option } => {
                write!(f, "`{ty}` has no option `{option}`")
            }
            Self::DuplicateOption(option) => {
                write!(f, "`{option}` is matched more than once")
            }
            Self::MissingOptions(options) => {
                let s = if options.len() == 1 { "" } else { "s" };
                write!(
                    f,
                    "`match` is missing the option{s} {}",
                    quoted_list(options, "and")
                )
            }
            Self::OptionNeedsValue {
                ty,
                option,
                expected,
            } => write!(
                f,
                "`{ty}.{option}` holds a value of type `{expected}`, so it must be given one, as in `{ty}.{option}(value)`"
            ),
            Self::ConstructedChoice(ty) => write!(
                f,
                "`{ty}` is a choice, so it can only be created from one of its options"
            ),
            Self::NoMember { ty, member } => {
                write!(f, "`{ty}` has no field or method `{member}`")
            }
            Self::FieldNotMethod { ty, field } => {
                write!(f, "`{ty}.{field}` is a field, not a method")
            }
            Self::MemberNotValue { ty, member, kind } => {
                write!(f, "`{ty}.{member}` is {}, not a value", kind.with_article())
            }
            Self::NeedsType { ty, member, kind } => write!(
                f,
                "`{ty}.{member}` is {}, so it must be used on `{ty}` itself rather than a value",
                kind.with_article()
            ),
            Self::NeedsValue { ty, member, kind } => write!(
                f,
                "`{ty}.{member}` is {}, so it must be used on a value of `{ty}` rather than `{ty}` itself",
                kind.with_article()
            ),
            Self::ModuleNotValue { module, member } => write!(
                f,
                "`{module}` is a module, not a value, so use `{module}::{member}` to refer to a name in it"
            ),
            Self::MemberNotImported { ty, member, module } => write!(
                f,
                "`{ty}.{member}` is declared in `{module}`, which is not imported"
            ),
            Self::MemberRedeclared {
                ty,
                member,
                existing,
            } => write!(
                f,
                "`{ty}` already has {} `{member}`",
                existing.with_article()
            ),
            Self::SelfOutsideImpl => write!(f, "`self` can only be used inside an `impl`"),
            Self::SelfInStatic => write!(f, "`self` can't be used in a static function"),
        }
    }
}

/// Checks every module in a project, along with the standard library,
/// returning the program they make up.
pub fn check(modules: &[Module]) -> Result<ir::Program, Vec<ModuleError>> {
    let mut checker = Checker::default();
    checker.add_stdlib();
    for module in modules {
        checker.add_module(None, module);
    }
    checker.check_modules();

    if checker.errors.is_empty() {
        Ok(checker.program())
    } else {
        Err(checker.errors)
    }
}

#[derive(Debug, Clone)]
struct Signature<'a> {
    /// Its full name, as in `ir::Function::name`.
    name: String,
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
struct OptionSignature<'a> {
    name: &'a str,
    /// The type of the value it holds, or `None` if it's unknown.
    ty: Option<Type>,
}

#[derive(Debug, Clone)]
enum Binding {
    Value {
        mutability: Mutability,
        /// `None` if the type couldn't be determined because of an error, so
        /// that later uses don't report errors caused by the same mistake.
        ty: Option<Type>,
        /// An index into the locals of the function declaring it.
        local: usize,
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
    /// What its initialiser compiles to, once it's been checked, or `None` if
    /// it has an error.
    lowered: Option<ir::Expr>,
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
    /// Its full name, as in `ir::Function::name`.
    full_name: String,
    /// `None` if the return type is unknown.
    returns: Option<Type>,
    /// Whether it's a `static fn`, which has no `self`.
    is_static: bool,
    /// The types of the values declared in it so far, including `self` and
    /// its parameters, which are `None` if they're unknown.
    locals: Vec<Option<Type>>,
}

/// What kind of function a body belongs to, which decides whether it has
/// `self`, and which type it can construct.
#[derive(Debug, Clone)]
enum FnKind {
    Function,
    /// A method, with the type of `self`, which is `None` if it's unknown.
    Method(Option<Type>),
    /// A static function, with the type whose `impl` it's in, which is `None`
    /// if it's unknown.
    Static(Option<Type>),
}

/// What `.member` is used on.
enum Receiver {
    /// A value of the type, for its fields and methods, along with what it
    /// compiles to, or `None` if it has an error.
    Value(Type, Option<ir::Expr>),
    /// The type itself, for its static functions.
    Type(Type),
}

struct ModuleInfo<'a> {
    /// The module's path, such as `/dir/other` or `stdlib/core`.
    display: String,
    /// The package the module is in, or `None` if it's in the project.
    package: Option<&'a str>,
    /// The module's path from the root of its package or the project.
    path: &'a [String],
    /// Whether modules outside its package can import it.
    exported: bool,
    /// The name used to qualify the module's names, such as `other` in
    /// `other::f`.
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

/// A field or method of a type, which are found with `.name` on a value.
/// Whether it's called is decided by the brackets after it, not by which kind
/// it is.
enum Member {
    /// The field's index in the order its group declares them, and its type,
    /// which is `None` if it's unknown.
    Field(usize, Option<Type>),
    Method(Method),
}

/// A method or static function from an `impl`.
#[derive(Debug, Clone, Copy)]
struct Method {
    /// The index in `Checker::modules` of the module whose `impl` declares it.
    module: usize,
    /// An index into `Checker::functions`.
    function: usize,
}

/// The result of checking an expression.
struct Typed {
    /// `None` if the type couldn't be determined because of an error that has
    /// already been reported.
    ty: Option<Type>,
    /// What the expression compiles to, or `None` if it has an error.
    expr: Option<ir::Expr>,
}

impl Typed {
    /// An expression whose error has already been reported.
    const UNKNOWN: Self = Self {
        ty: None,
        expr: None,
    };
}

/// Splits checked expressions into their types, and what they compile to if
/// none of them have errors.
fn split(typed: Vec<Typed>) -> (Vec<Option<Type>>, Option<Vec<ir::Expr>>) {
    let (types, exprs): (Vec<_>, Vec<_>) = typed.into_iter().map(|t| (t.ty, t.expr)).unzip();
    (types, exprs.into_iter().collect())
}

/// A top-level declaration that still needs checking once every module's
/// names have been declared.
enum Pending<'a> {
    Global(usize),
    Fn {
        index: usize,
        name: &'a str,
        body: &'a [Stmt],
        kind: FnKind,
    },
}

#[derive(Default)]
struct Checker<'a> {
    /// The standard library first, then the project's modules.
    modules: Vec<ModuleInfo<'a>>,
    /// The index in `modules` of `stdlib/core`, which every project module
    /// imports, and which declares the types of literals.
    core: usize,
    /// The primitive types, by `id`.
    primitives: HashMap<usize, ir::Primitive>,
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
    /// The type whose `impl` the code being checked is in, including inside
    /// functions nested in its methods. Only it can be constructed. `Some(None)`
    /// if the `impl`'s type is unknown.
    impl_type: Option<Option<Type>>,
    functions: Vec<Signature<'a>>,
    /// For each of `functions`, what it compiles to, once its body has been
    /// checked, or `None` if it has an error.
    lowered_functions: Vec<Option<ir::Function>>,
    /// For each type `id`, its methods by name. Methods aren't names in any
    /// scope, and are only found through the type of the value they're called
    /// on, in the modules that can see the module declaring them. They share
    /// names with the type's fields.
    methods: HashMap<usize, HashMap<&'a str, Method>>,
    /// For each type `id`, its static functions by name, which are found like
    /// methods but on the type itself. They have their own names, separate
    /// from the type's fields and methods.
    statics: HashMap<usize, HashMap<&'a str, Method>>,
    globals: Vec<Global<'a>>,
    /// Indexes into `globals`, in the order their types were worked out, so
    /// that each comes after the values its initialiser uses.
    global_order: Vec<usize>,
    /// For each type, indexed by `id`, its fields in the order they're
    /// declared. Choices don't have any.
    group_fields: Vec<Vec<FieldSignature<'a>>>,
    /// For each type, indexed by `id`, the index in `modules` of the module
    /// that declares it, which is the only one that can construct it if it's
    /// a group.
    group_modules: Vec<usize>,
    /// For each type, indexed by `id`, its full name, such as `/main::Point`.
    type_names: Vec<String>,
    /// For each choice, by `id`, its options in the order they're declared.
    /// Other types aren't in it.
    choice_options: HashMap<usize, Vec<OptionSignature<'a>>>,
    errors: Vec<ModuleError>,
}

impl<'a> Checker<'a> {
    /// Adds a module from `package`, or from the project if that's `None`.
    /// Returns its index in `modules`.
    fn add_module(&mut self, package: Option<&'a str>, module: &'a Module) -> usize {
        let path = module.path.join("/");
        let display = match package {
            Some(package) => format!("{package}/{path}"),
            None => format!("/{path}"),
        };
        self.modules.push(ModuleInfo {
            display,
            package,
            path: &module.path,
            exported: package.is_none(),
            name: module.path.last().expect("module paths aren't empty"),
            program: &module.program,
            scope: HashMap::new(),
            imports: vec![],
        });
        self.modules.len() - 1
    }

    /// Adds the standard library's modules. Only `stdlib/core` can be imported
    /// from outside it.
    fn add_stdlib(&mut self) {
        for module in STDLIB.iter() {
            let index = self.add_module(Some(STDLIB_PACKAGE), module);
            if module.path == ["core"] {
                self.core = index;
                self.modules[index].exported = true;
            }
        }
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

        // Every type is declared before any fields or options are resolved,
        // so types can contain each other in any order.
        let mut types = vec![];
        for module in 0..self.modules.len() {
            self.module = module;
            for stmt in self.modules[module].program {
                if let Stmt::Group { name, .. }
                | Stmt::Choice { name, .. }
                | Stmt::Primitive { name } = stmt
                {
                    types.push((module, self.declare_type(name), stmt));
                }
            }
        }
        for &(module, id, stmt) in &types {
            self.module = module;
            self.define_type(id, stmt);
        }
        for &(module, id, stmt) in &types {
            self.module = module;
            if let Stmt::Group { name, .. } | Stmt::Choice { name, .. } = stmt {
                self.check_not_recursive(id, name);
            }
        }

        // Functions, methods and values come after groups, since their types
        // can be groups.
        let mut pending = vec![];
        for module in 0..self.modules.len() {
            self.module = module;
            for stmt in self.modules[module].program {
                match stmt {
                    Stmt::Fn(function) => {
                        let index =
                            self.declare_fn(&function.name, &function.params, &function.ret);
                        pending.push((
                            module,
                            Pending::Fn {
                                index,
                                name: &function.name,
                                body: &function.body,
                                kind: FnKind::Function,
                            },
                        ));
                    }
                    Stmt::Impl { ty, functions } => {
                        let self_ty = self.resolve_type(ty);
                        for ImplFunction {
                            is_static,
                            function,
                        } in functions
                        {
                            let full_name =
                                self.full_name(&format!("{}.{}", ty.name, function.name));
                            let index = self.declare_method(
                                self_ty.as_ref(),
                                function,
                                *is_static,
                                full_name,
                            );
                            let kind = if *is_static {
                                FnKind::Static(self_ty.clone())
                            } else {
                                FnKind::Method(self_ty.clone())
                            };
                            pending.push((
                                module,
                                Pending::Fn {
                                    index,
                                    name: &function.name,
                                    body: &function.body,
                                    kind,
                                },
                            ));
                        }
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
                            lowered: None,
                        });
                        let index = self.globals.len() - 1;
                        self.declare(name, Binding::Global(index));
                        pending.push((module, Pending::Global(index)));
                    }
                    Stmt::Use(_)
                    | Stmt::Group { .. }
                    | Stmt::Choice { .. }
                    | Stmt::Primitive { .. } => {}
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
                Pending::Fn {
                    index,
                    name,
                    body,
                    kind,
                } => self.fn_body(index, name, body, kind),
            }
        }
    }

    /// Finds the modules imported by the current module. Modules in the
    /// project always import `stdlib/core`.
    fn resolve_imports(&mut self) {
        let mut imports = vec![];
        if self.modules[self.module].package.is_none() {
            imports.push(self.core);
        }

        for stmt in self.modules[self.module].program {
            let Stmt::Use(import) = stmt else { continue };
            let path = match import {
                Import::Module(path) => path,
                Import::Directory(dir) => {
                    let found = self.resolve_directory(dir);
                    if found.is_empty() {
                        self.error(CheckError::EmptyDirectory(import.to_string()));
                    }
                    for module in found {
                        if !imports.contains(&module) {
                            imports.push(module);
                        }
                    }
                    continue;
                }
            };
            match self.resolve_path(path) {
                // Importing a module twice, or importing itself, is harmless.
                Some(module) if module == self.module || imports.contains(&module) => {}
                Some(module) if !self.can_import(module) => {
                    let module = self.modules[module].display.clone();
                    self.error(CheckError::PrivateModule {
                        module,
                        package: STDLIB_PACKAGE.to_string(),
                    });
                }
                Some(module) => imports.push(module),
                None => self.error(self.unknown_module(path)),
            }
        }

        self.modules[self.module].imports = imports;
    }

    /// Whether the current module can import `module`, which must be in the
    /// same package, or exported from its package.
    fn can_import(&self, module: usize) -> bool {
        let target = &self.modules[module];
        target.exported || target.package == self.modules[self.module].package
    }

    /// The error for a module path that doesn't lead to a module.
    fn unknown_module(&self, path: &ModulePath) -> CheckError {
        match path {
            ModulePath::Named(name) if name == STDLIB_PACKAGE => {
                CheckError::PackageNotModule(name.clone())
            }
            _ => CheckError::UnknownModule(path.to_string()),
        }
    }

    /// Finds the module at `path`.
    fn resolve_path(&self, path: &ModulePath) -> Option<usize> {
        let (package, full) = self.full_path(path)?;
        self.modules.iter().position(|module| {
            module.package == package && module.path.iter().eq(full.iter().copied())
        })
    }

    /// Finds the modules in the directory at `path`, including in its
    /// subdirectories, that the current module can import, other than itself.
    fn resolve_directory(&self, path: &ModulePath) -> Vec<usize> {
        let Some((package, dir)) = self.full_path(path) else {
            return vec![];
        };
        (0..self.modules.len())
            .filter(|&index| {
                let module = &self.modules[index];
                module.package == package
                    && module.path.len() > dir.len()
                    && module.path.iter().zip(&dir).all(|(a, b)| a == *b)
                    && index != self.module
                    && self.can_import(index)
            })
            .collect()
    }

    /// The package and path within it that `path` leads to. A path starting
    /// with `/` is from the root of the project, one starting with `./` is
    /// from the current module's directory, and one starting with a package's
    /// name is from the root of that package. Modules can't be found by name
    /// alone.
    fn full_path(&self, path: &'a ModulePath) -> Option<(Option<&'a str>, Vec<&'a String>)> {
        let current = &self.modules[self.module];
        Some(match path {
            ModulePath::Absolute(segments) => (None, segments.iter().collect()),
            ModulePath::Relative(segments) => {
                let dir = current
                    .path
                    .split_last()
                    .map_or(current.path, |(_, dir)| dir);
                (current.package, dir.iter().chain(segments).collect())
            }
            ModulePath::Package(segments) => {
                (Some(segments[0].as_str()), segments[1..].iter().collect())
            }
            ModulePath::Named(_) => return None,
        })
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
                // Modules aren't identifiers, so a name can be both. If it's
                // only an identifier, that's probably the mistake.
                [] if self.is_declared(name) => Err(CheckError::NotAModule(name.clone())),
                [] if name == STDLIB_PACKAGE => Err(CheckError::PackageNotModule(name.clone())),
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

    /// Whether `name` is declared anywhere the current code could see it,
    /// even if it's a value that a function can't use.
    fn is_declared(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains_key(name))
            || self
                .visible_modules()
                .any(|module| self.modules[module].scope.contains_key(name))
    }

    /// A type from `stdlib/core`, such as `Int` for integer literals. Found in
    /// the standard library itself, so it can't be shadowed.
    fn std_type(&self, name: &str) -> Type {
        match self.modules[self.core].scope.get(name) {
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

                // The current module's own names come first. Otherwise, a name
                // that isn't qualified must only be declared in one of the
                // modules it imports.
                if let Some(binding) = self.modules[self.module].scope.get(name.name.as_str()) {
                    return self.top_level(binding);
                }
                let found: Vec<_> = self.modules[self.module]
                    .imports
                    .iter()
                    .filter_map(|&module| {
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

        self.top_level(binding)
    }

    /// Checks that a module's top-level `binding` can be used here. Top-level
    /// values are outside every function.
    fn top_level(&self, binding: &Binding) -> Result<Binding, LookupError> {
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
        let Typed { ty, expr } = self.decl_type(name, type_name, value);
        self.module = outer_module;

        self.globals[index].state = GlobalState::Checked(ty.clone());
        self.globals[index].lowered = expr;
        self.global_order.push(index);
        ty
    }

    /// Checks a `let` or `var` declaration's value, returning the type it's
    /// declared with, along with what the value compiles to.
    fn decl_type(&mut self, name: &str, type_name: Option<&Name>, value: &Expr) -> Typed {
        // Checked first so that `let x = x` refers to an earlier `x`.
        let Typed { ty: found, expr } = self.expr(value);

        // An explicit type takes precedence over the value's type, so later
        // uses are checked against what the programmer wrote.
        let ty = match type_name {
            Some(type_name) => {
                let expected = self.resolve_type(type_name);
                self.check_assignable(name, expected.clone(), found);
                expected
            }
            None => found,
        };
        Typed { ty, expr }
    }

    /// The full name of something called `name` declared here, as in
    /// `ir::Function::name`. Names inside a function start with the
    /// function's own.
    fn full_name(&self, name: &str) -> String {
        let outer = match &self.current_fn {
            Some(function) => &function.full_name,
            None => &self.modules[self.module].display,
        };
        format!("{outer}::{name}")
    }

    /// Declares a function, returning its index into `functions`.
    fn declare_fn(&mut self, name: &'a str, params: &'a [Param], ret: &Name) -> usize {
        let index = self.signature(params, ret, self.full_name(name));
        self.declare(name, Binding::Function(index));
        index
    }

    /// Declares a method or static function of `self_ty`, which is `None` if
    /// it's unknown, returning its index into `functions`.
    fn declare_method(
        &mut self,
        self_ty: Option<&Type>,
        method: &'a Function,
        is_static: bool,
        full_name: String,
    ) -> usize {
        let index = self.signature(&method.params, &method.ret, full_name);
        if let Some(self_ty) = self_ty {
            // Methods must be unique across every module, even ones that
            // can't see each other, since a call can't say which module's
            // method it means.
            let existing = if is_static {
                let name = &method.name;
                match (
                    self.option(self_ty, name),
                    self.static_function(self_ty, name),
                ) {
                    (Some(_), _) => Some(MemberKind::Option),
                    (None, Some(_)) => Some(MemberKind::StaticFunction),
                    (None, None) => None,
                }
            } else {
                match self.member(self_ty, &method.name) {
                    Some(Member::Field(..)) => Some(MemberKind::Field),
                    Some(Member::Method(_)) => Some(MemberKind::Method),
                    None => None,
                }
            };
            if let Some(existing) = existing {
                self.error(CheckError::MemberRedeclared {
                    ty: self_ty.clone(),
                    member: method.name.clone(),
                    existing,
                });
            } else {
                let found = Method {
                    module: self.module,
                    function: index,
                };
                let table = if is_static {
                    &mut self.statics
                } else {
                    &mut self.methods
                };
                table
                    .entry(self_ty.id)
                    .or_default()
                    .insert(&method.name, found);
            }
        }
        index
    }

    /// Resolves the parameter and return types of the function called
    /// `full_name`, returning its index into `functions`.
    fn signature(&mut self, params: &'a [Param], ret: &Name, full_name: String) -> usize {
        let params = params
            .iter()
            .map(|param| ParamSignature {
                label: param.label.as_deref(),
                name: &param.name,
                ty: self.resolve_type(&param.ty),
            })
            .collect();
        let returns = self.resolve_type(ret);

        self.functions.push(Signature {
            name: full_name,
            params,
            returns,
        });
        self.lowered_functions.push(None);
        self.functions.len() - 1
    }

    /// Checks the body of the function at `index` in `functions`.
    fn fn_body(&mut self, index: usize, name: &'a str, body: &'a [Stmt], kind: FnKind) {
        let Signature {
            name: full_name,
            params,
            returns,
        } = self.functions[index].clone();

        // The body starts a new function scope holding the parameters. It can
        // see the functions and types declared outside it, but not values.
        let outer_scope_start = std::mem::replace(&mut self.fn_scope_start, self.scopes.len());
        let outer_fn = self.current_fn.replace(FnContext {
            name,
            full_name: full_name.clone(),
            returns: returns.clone(),
            is_static: matches!(kind, FnKind::Static(_)),
            locals: vec![],
        });
        self.scopes.push(HashMap::new());
        let outer_impl_type = match &kind {
            FnKind::Method(ty) | FnKind::Static(ty) => self.impl_type.replace(ty.clone()),
            FnKind::Function => self.impl_type.clone(),
        };

        // `self` is a keyword, so it can't clash with a parameter. Like any
        // other value, functions nested in the method can't use it.
        if let FnKind::Method(ty) = kind {
            self.declare_local("self", Mutability::Let, ty);
        }
        for param in params {
            self.declare_local(param.name, Mutability::Let, param.ty);
        }
        let param_count = self.current_fn.as_ref().expect("set above").locals.len();
        let mut lowered: Vec<_> = body.iter().filter_map(|stmt| self.stmt(stmt)).collect();

        self.scopes.pop();
        let locals = std::mem::replace(&mut self.current_fn, outer_fn)
            .expect("set above")
            .locals;
        self.fn_scope_start = outer_scope_start;
        self.impl_type = outer_impl_type;

        // A `Void` function can finish without an explicit `return`.
        if !always_returns(body) {
            match &returns {
                Some(expected) if *expected != self.std_type("Void") => {
                    self.error(CheckError::MissingReturn {
                        function: name.to_string(),
                        expected: expected.clone(),
                    });
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
                body: lowered,
            });
    }

    /// Declares a value in the current function, returning its index among
    /// the function's locals.
    fn declare_local(&mut self, name: &'a str, mutability: Mutability, ty: Option<Type>) -> usize {
        let locals = &mut self
            .current_fn
            .as_mut()
            .expect("values are only declared inside functions")
            .locals;
        locals.push(ty.clone());
        let local = locals.len() - 1;
        self.declare(
            name,
            Binding::Value {
                mutability,
                ty,
                local,
            },
        );
        local
    }

    /// Creates a `Void`, which has no fields.
    fn void(&self) -> ir::Expr {
        ir::Expr::Construct {
            group: self.std_type("Void").id,
            fields: vec![],
        }
    }

    /// Declares a type without its fields or options, returning its `id`.
    fn declare_type(&mut self, name: &'a str) -> usize {
        let id = self.group_fields.len();
        self.group_fields.push(vec![]);
        self.group_modules.push(self.module);
        self.type_names.push(self.full_name(name));
        let ty = Type {
            id,
            name: name.to_string(),
        };
        self.declare(name, Binding::Type(ty));
        id
    }

    /// Resolves the fields or options of `stmt`, which declares the type with
    /// `id`.
    fn define_type(&mut self, id: usize, stmt: &'a Stmt) {
        match stmt {
            Stmt::Group { name, fields } => self.define_group(id, name, fields),
            Stmt::Choice { name, options } => self.define_choice(id, name, options),
            Stmt::Primitive { name } => self.define_primitive(id, name),
            _ => unreachable!("only groups, choices and primitives declare types"),
        }
    }

    /// Records the primitive type called `name` with `id`, which must be
    /// declared in the standard library.
    fn define_primitive(&mut self, id: usize, name: &str) {
        if self.modules[self.module].package != Some(STDLIB_PACKAGE) {
            self.error(CheckError::PrimitiveOutsideStdlib(name.to_string()));
            return;
        }
        let primitive = match name {
            "Int" => ir::Primitive::Int,
            "Bool" => ir::Primitive::Bool,
            _ => panic!("the compiler doesn't know how to represent `{name}`"),
        };
        self.primitives.insert(id, primitive);
    }

    /// Resolves the options of the choice called `name` with `id`.
    fn define_choice(&mut self, id: usize, name: &str, options: &'a [ChoiceOption]) {
        // Option names only need to be unique within their choice.
        let mut option_names = HashSet::new();
        let mut signatures = vec![];
        for option in options {
            if !option_names.insert(&option.name) {
                self.error(CheckError::MemberRedeclared {
                    ty: Type {
                        id,
                        name: name.to_string(),
                    },
                    member: option.name.clone(),
                    existing: MemberKind::Option,
                });
            }
            signatures.push(OptionSignature {
                name: &option.name,
                ty: self.resolve_type(&option.ty),
            });
        }
        self.choice_options.insert(id, signatures);
    }

    /// Resolves the fields of the group called `name` with `id`.
    fn define_group(&mut self, id: usize, name: &str, fields: &'a [Field]) {
        // Field names only need to be unique within their group.
        let mut field_names = HashSet::new();
        for field in fields {
            if !field_names.insert(&field.name) {
                self.error(CheckError::MemberRedeclared {
                    ty: Type {
                        id,
                        name: name.to_string(),
                    },
                    member: field.name.clone(),
                    existing: MemberKind::Field,
                });
            }
            let ty = self.resolve_type(&field.ty);
            self.group_fields[id].push(FieldSignature {
                name: &field.name,
                ty,
            });
        }
    }

    /// The `id`s of the types that the type with `id` holds directly: a
    /// group's fields, or the values of a choice's options.
    fn contained_types(&self, id: usize) -> impl Iterator<Item = usize> + '_ {
        let options = self.choice_options.get(&id).into_iter().flatten();
        self.group_fields[id]
            .iter()
            .map(|field| &field.ty)
            .chain(options.map(|option| &option.ty))
            .filter_map(|ty| ty.as_ref().map(|ty| ty.id))
    }

    /// Reports an error if the group or choice with `id` contains itself,
    /// which would make it infinitely large.
    fn check_not_recursive(&mut self, id: usize, name: &str) {
        let mut seen = HashSet::new();
        let mut unvisited: Vec<_> = self.contained_types(id).collect();
        while let Some(next) = unvisited.pop() {
            if next == id {
                self.error(CheckError::RecursiveType(name.to_string()));
                return;
            }
            if seen.insert(next) {
                unvisited.extend(self.contained_types(next));
            }
        }
    }

    /// Checks `stmts` in a new scope, so their declarations aren't visible
    /// afterwards, returning what they compile to.
    fn block(&mut self, stmts: &'a [Stmt]) -> Vec<ir::Stmt> {
        self.scopes.push(HashMap::new());
        let lowered = stmts.iter().filter_map(|stmt| self.stmt(stmt)).collect();
        self.scopes.pop();
        lowered
    }

    /// Checks a statement inside a function, returning what it compiles to,
    /// or `None` if it declares a function or group, which compile
    /// separately, or if it has an error.
    fn stmt(&mut self, stmt: &'a Stmt) -> Option<ir::Stmt> {
        match stmt {
            Stmt::Use(_) | Stmt::Impl { .. } | Stmt::Primitive { .. } => {
                unreachable!(
                    "the parser only allows `use`, `impl` and `primitive` at the top level"
                )
            }
            Stmt::Decl {
                mutability,
                name,
                ty: type_name,
                value,
            } => {
                let Typed { ty, expr } = self.decl_type(name, type_name.as_ref(), value);
                let local = self.declare_local(name, *mutability, ty);
                Some(ir::Stmt::Assign {
                    local,
                    value: expr?,
                })
            }
            Stmt::Assign { name, value } => {
                let found = self.expr(value);
                let target = Name::new(name);
                match self.lookup(&target) {
                    Ok(Binding::Value {
                        mutability: Mutability::Var,
                        ty: expected,
                        local,
                    }) => {
                        self.check_assignable(name, expected, found.ty);
                        return Some(ir::Stmt::Assign {
                            local,
                            value: found.expr?,
                        });
                    }
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
                None
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond = self.expr(cond);
                if let Some(found) = &cond.ty
                    && *found != self.std_type("Bool")
                {
                    self.error(CheckError::ConditionNotBool(found.clone()));
                }

                let then_block = self.block(then_block);
                let else_block = match else_block {
                    Some(else_block) => self.block(else_block),
                    None => vec![],
                };
                Some(ir::Stmt::If {
                    cond: cond.expr?,
                    then_block,
                    else_block,
                })
            }
            // Inside functions, functions and groups are only visible after
            // they're declared.
            Stmt::Fn(function) => {
                // Declared before the body is checked, so it can call itself.
                let index = self.declare_fn(&function.name, &function.params, &function.ret);
                self.fn_body(index, &function.name, &function.body, FnKind::Function);
                None
            }
            Stmt::Group { name, .. } | Stmt::Choice { name, .. } => {
                let id = self.declare_type(name);
                self.define_type(id, stmt);
                self.check_not_recursive(id, name);
                None
            }
            Stmt::Match { value, options } => self.match_(value, options),
            Stmt::Return(value) => {
                let found = value.as_ref().map(|value| self.expr(value));
                let FnContext { name, returns, .. } = self
                    .current_fn
                    .clone()
                    .expect("statements are only checked inside functions");

                // A bare `return` returns `Void`.
                let function = name.to_string();
                match (returns, found.as_ref().map(|found| &found.ty)) {
                    (Some(expected), None) if expected != self.std_type("Void") => {
                        self.error(CheckError::MissingReturnValue { function, expected });
                    }
                    (Some(expected), Some(Some(found))) if expected != *found => {
                        self.error(CheckError::ReturnTypeMismatch {
                            function,
                            expected,
                            found: found.clone(),
                        });
                    }
                    _ => {}
                }
                match found {
                    Some(found) => found.expr.map(ir::Stmt::Return),
                    None => Some(ir::Stmt::Return(self.void())),
                }
            }
            Stmt::Expr(expr) => {
                let found = match expr {
                    // A call's return value can be ignored.
                    Expr::Call { callee, args } => self.call(callee, args),
                    _ => self.expr(expr),
                };
                found.expr.map(ir::Stmt::Expr)
            }
        }
    }

    /// Checks a `match` on `value`, which must be a choice whose every option
    /// is matched exactly once by `options`, returning what it compiles to.
    fn match_(&mut self, value: &Expr, options: &'a [MatchOption]) -> Option<ir::Stmt> {
        let Typed { ty, expr } = self.expr(value);
        let choice = match ty {
            Some(ty) if self.choice_options.contains_key(&ty.id) => Some(ty),
            Some(found) => {
                self.error(CheckError::MatchNotChoice(found));
                None
            }
            None => None,
        };

        let mut matched = HashSet::new();
        let mut lowered = vec![];
        for option in options {
            // Options are still checked when the choice is unknown, so the
            // errors in their blocks are found.
            let found = choice
                .as_ref()
                .and_then(|choice| self.matched_option(choice, option, &mut matched));

            // The binding is only visible in its option's block.
            self.scopes.push(HashMap::new());
            let binding = option.binding.as_ref().map(|name| {
                let ty = found.as_ref().and_then(|(_, ty)| ty.clone());
                self.declare_local(name, Mutability::Let, ty)
            });
            let body = option
                .body
                .iter()
                .filter_map(|stmt| self.stmt(stmt))
                .collect();
            self.scopes.pop();

            if let Some((index, _)) = found {
                lowered.push(ir::MatchOption {
                    index,
                    binding,
                    body,
                });
            }
        }

        let choice = choice?;
        let missing: Vec<_> = self.choice_options[&choice.id]
            .iter()
            .filter(|option| !matched.contains(option.name))
            .map(|option| format!("{choice}.{}", option.name))
            .collect();
        if !missing.is_empty() {
            self.error(CheckError::MissingOptions(missing));
        }

        Some(ir::Stmt::Match {
            value: expr?,
            choice: choice.id,
            options: lowered,
        })
    }

    /// Finds the option of `choice` that `option` from a `match` names,
    /// returning its index and the type of its value, as `Checker::option`
    /// does, or `None` if it has an error. `matched` holds the names of the
    /// options matched before it.
    fn matched_option(
        &mut self,
        choice: &Type,
        option: &'a MatchOption,
        matched: &mut HashSet<&'a str>,
    ) -> Option<(usize, Option<Type>)> {
        let ty = self.resolve_type(&option.ty)?;
        if ty != *choice {
            self.error(CheckError::WrongPatternType {
                expected: choice.clone(),
                found: ty,
            });
            return None;
        }
        let Some(found) = self.option(&ty, &option.name) else {
            let option = option.name.clone();
            self.error(CheckError::UnknownOption { ty, option });
            return None;
        };
        if !matched.insert(&option.name) {
            self.error(CheckError::DuplicateOption(format!("{ty}.{}", option.name)));
            return None;
        }
        Some(found)
    }

    /// Checks `expr`, returning its type, or `None` if it can't be determined
    /// because of an error that has already been reported, along with what it
    /// compiles to.
    fn expr(&mut self, expr: &Expr) -> Typed {
        match expr {
            Expr::Int(value) => self.literal("Int", ir::Expr::Int(*value)),
            Expr::Bool(value) => self.literal("Bool", ir::Expr::Bool(*value)),
            Expr::Var(name) => match self.lookup(name) {
                Ok(Binding::Value { ty, local, .. }) => Typed {
                    ty,
                    expr: Some(ir::Expr::Local(local)),
                },
                Ok(Binding::Global(index)) => Typed {
                    ty: self.global_type(index),
                    expr: Some(ir::Expr::Global(index)),
                },
                Ok(Binding::Function(_)) => {
                    self.error(CheckError::NotAValue(name.to_string()));
                    Typed::UNKNOWN
                }
                Ok(Binding::Type(_)) => {
                    self.error(CheckError::TypeAsValue(name.to_string()));
                    Typed::UNKNOWN
                }
                Err(error) => {
                    self.lookup_error(name, error);
                    Typed::UNKNOWN
                }
            },
            // Operators always produce their result type, even if an operand
            // is invalid, so one bad operand doesn't cause further errors.
            Expr::Unary(op, operand) => {
                let ty = self.std_type(unary_type(op));
                let operand = self.operand(op.symbol(), operand, ty.clone());
                let expr = operand.map(|operand| ir::Expr::Unary(op.clone(), Box::new(operand)));
                Typed { ty: Some(ty), expr }
            }
            Expr::Binary(lhs, op @ (BinOp::Eq | BinOp::Ne), rhs) => self.comparison(lhs, op, rhs),
            Expr::Binary(lhs, op, rhs) => {
                let ty = self.std_type(binary_type(op));
                let lhs = self.operand(op.symbol(), lhs, ty.clone());
                let rhs = self.operand(op.symbol(), rhs, ty.clone());
                let expr = lhs
                    .zip(rhs)
                    .map(|(lhs, rhs)| ir::Expr::Binary(Box::new(lhs), op.clone(), Box::new(rhs)));
                Typed { ty: Some(ty), expr }
            }
            Expr::Call { callee, args } => self.call(callee, args),
            Expr::Construct { ty, fields } => self.construct(ty, fields),
            Expr::MethodCall {
                receiver,
                method,
                args,
            } => self.method_call(receiver, method, args),
            Expr::Field { receiver, field } => self.field(receiver, field),
            Expr::SelfValue => {
                let name = Name::new("self");
                match self.lookup(&name) {
                    Ok(Binding::Value { ty, local, .. }) => Typed {
                        ty,
                        expr: Some(ir::Expr::Local(local)),
                    },
                    Ok(_) => unreachable!("only methods declare `self`, as a value"),
                    Err(LookupError::Undeclared) => {
                        let in_static = self.current_fn.as_ref().is_some_and(|f| f.is_static);
                        self.error(if in_static {
                            CheckError::SelfInStatic
                        } else {
                            CheckError::SelfOutsideImpl
                        });
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

    /// Checks a comparison of two `Int`s or two `Bool`s, which produces a
    /// `Bool`. The right operand must have the same type as the left.
    fn comparison(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Typed {
        let Typed { ty, expr: lhs } = self.expr(lhs);
        let rhs = match ty {
            Some(ty) if ty == self.std_type("Int") || ty == self.std_type("Bool") => {
                self.operand(op.symbol(), rhs, ty)
            }
            // The right operand is still checked for its own errors, but
            // there's nothing it could be compared to.
            Some(found) => {
                let op = op.symbol();
                self.error(CheckError::IncomparableOperand { op, found });
                self.expr(rhs);
                None
            }
            None => self.expr(rhs).expr,
        };

        let expr = lhs
            .zip(rhs)
            .map(|(lhs, rhs)| ir::Expr::Binary(Box::new(lhs), op.clone(), Box::new(rhs)));
        Typed {
            ty: Some(self.std_type("Bool")),
            expr,
        }
    }

    /// A literal of the standard library type called `name`, such as `Int`.
    fn literal(&self, name: &str, value: ir::Expr) -> Typed {
        Typed {
            ty: Some(self.std_type(name)),
            expr: Some(value),
        }
    }

    /// Checks the receiver of a field access or method call, which is followed
    /// by `.member`. A type's name is the type itself, for its static
    /// functions. A module's name gets its own error, since `::` was probably
    /// meant.
    fn receiver(&mut self, receiver: &Expr, member: &str) -> Option<Receiver> {
        if let Expr::Var(name) = receiver {
            match self.lookup(name) {
                Ok(Binding::Type(ty)) => return Some(Receiver::Type(ty)),
                Err(LookupError::Undeclared)
                    if name.module.is_none()
                        && self
                            .visible_modules()
                            .any(|module| self.modules[module].name == name.name) =>
                {
                    self.error(CheckError::ModuleNotValue {
                        module: name.name.clone(),
                        member: member.to_string(),
                    });
                    return None;
                }
                _ => {}
            }
        }
        let Typed { ty, expr } = self.expr(receiver);
        Some(Receiver::Value(ty?, expr))
    }

    /// Finds the field or method of `ty` called `name`.
    fn member(&self, ty: &Type, name: &str) -> Option<Member> {
        if let Some(index) = self.group_fields[ty.id]
            .iter()
            .position(|field| field.name == name)
        {
            let field_ty = self.group_fields[ty.id][index].ty.clone();
            return Some(Member::Field(index, field_ty));
        }
        self.methods
            .get(&ty.id)
            .and_then(|methods| methods.get(name))
            .map(|method| Member::Method(*method))
    }

    /// Finds the static function of `ty` called `name`.
    fn static_function(&self, ty: &Type, name: &str) -> Option<Method> {
        self.statics
            .get(&ty.id)
            .and_then(|statics| statics.get(name))
            .copied()
    }

    /// Finds the option of `ty` called `name`, if `ty` is a choice, returning
    /// its index in the order they're declared, and the type of the value it
    /// holds, which is `None` if it's unknown.
    fn option(&self, ty: &Type, name: &str) -> Option<(usize, Option<Type>)> {
        let options = self.choice_options.get(&ty.id)?;
        let index = options.iter().position(|option| option.name == name)?;
        Some((index, options[index].ty.clone()))
    }

    /// The error for `name` not being a field or method of `ty`, which points
    /// out an option or static function with that name.
    fn no_member(&self, ty: Type, name: &str) -> CheckError {
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
    fn no_static_function(&self, ty: Type, name: &str) -> CheckError {
        let kind = match self.member(&ty, name) {
            Some(Member::Field(..)) => MemberKind::Field,
            Some(Member::Method(_)) => MemberKind::Method,
            None if self.choice_options.contains_key(&ty.id) => {
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
    fn field(&mut self, receiver: &Expr, field: &str) -> Typed {
        let Some(receiver) = self.receiver(receiver, field) else {
            return Typed::UNKNOWN;
        };
        let error = match receiver {
            Receiver::Value(ty, receiver) => match self.member(&ty, field) {
                Some(Member::Field(index, field_ty)) => {
                    return Typed {
                        ty: field_ty,
                        expr: receiver.map(|receiver| ir::Expr::Field {
                            receiver: Box::new(receiver),
                            index,
                        }),
                    };
                }
                Some(Member::Method(_)) => CheckError::MemberNotValue {
                    ty,
                    member: field.to_string(),
                    kind: MemberKind::Method,
                },
                None => self.no_member(ty, field),
            },
            // An option holding `Void` can be created by naming it.
            Receiver::Type(ty) if let Some((option, value_ty)) = self.option(&ty, field) => {
                let void = self.std_type("Void");
                match value_ty {
                    Some(value_ty) if value_ty == void => {
                        return Typed {
                            expr: Some(ir::Expr::Choose {
                                choice: ty.id,
                                option,
                                value: Box::new(self.void()),
                            }),
                            ty: Some(ty),
                        };
                    }
                    Some(expected) => CheckError::OptionNeedsValue {
                        ty: ty.clone(),
                        option: field.to_string(),
                        expected,
                    },
                    None => {
                        return Typed {
                            ty: Some(ty),
                            expr: None,
                        };
                    }
                }
            }
            Receiver::Type(ty) => match self.static_function(&ty, field) {
                Some(_) => CheckError::MemberNotValue {
                    ty,
                    member: field.to_string(),
                    kind: MemberKind::StaticFunction,
                },
                None => self.no_static_function(ty, field),
            },
        };
        self.error(error);
        Typed::UNKNOWN
    }

    /// Checks a call to a method on a value, or a static function on a type,
    /// returning its return type, or `None` if it can't be determined, along
    /// with what it compiles to.
    fn method_call(&mut self, receiver: &Expr, method: &str, args: &[Arg]) -> Typed {
        // The receiver and arguments are checked first, so their errors come
        // before the call's.
        let receiver = self.receiver(receiver, method);
        let (found, lowered_args) = split(args.iter().map(|arg| self.expr(&arg.value)).collect());

        let Some(receiver) = receiver else {
            return Typed::UNKNOWN;
        };
        if let Receiver::Type(ty) = &receiver
            && let Some(option) = self.option(ty, method)
        {
            return self.choose(ty, method, option, args, found, lowered_args);
        }
        let (ty, found_method, lowered_receiver) = match receiver {
            Receiver::Value(ty, lowered_receiver) => match self.member(&ty, method) {
                Some(Member::Method(found_method)) => (ty, found_method, Some(lowered_receiver)),
                Some(Member::Field(..)) => {
                    let field = method.to_string();
                    self.error(CheckError::FieldNotMethod { ty, field });
                    return Typed::UNKNOWN;
                }
                None => {
                    let error = self.no_member(ty, method);
                    self.error(error);
                    return Typed::UNKNOWN;
                }
            },
            Receiver::Type(ty) => match self.static_function(&ty, method) {
                Some(found_method) => (ty, found_method, None),
                None => {
                    let error = self.no_static_function(ty, method);
                    self.error(error);
                    return Typed::UNKNOWN;
                }
            },
        };
        let member = method.to_string();

        // Like functions, methods and static functions can only be used from
        // their own module or one that imports it.
        if !self
            .visible_modules()
            .any(|module| module == found_method.module)
        {
            self.error(CheckError::MemberNotImported {
                ty,
                member,
                module: self.modules[found_method.module].display.clone(),
            });
            return Typed::UNKNOWN;
        }

        let signature = self.functions[found_method.function].clone();
        let returns = self.check_args(format!("{ty}.{method}"), args, found, signature);

        // A method's receiver is its first argument.
        let lowered_args = match lowered_receiver {
            Some(lowered_receiver) => lowered_receiver
                .zip(lowered_args)
                .map(|(receiver, args)| iter::once(receiver).chain(args).collect()),
            None => lowered_args,
        };
        Typed {
            ty: returns,
            expr: lowered_args.map(|args| ir::Expr::Call {
                function: found_method.function,
                args,
            }),
        }
    }

    /// Checks the creation of the choice `ty` as the option called `name`,
    /// given its index and the type of its value, as found by `option`.
    /// Creating an option is like calling a function with one unlabelled
    /// parameter. The argument has the type `found`, and compiles to
    /// `lowered`.
    fn choose(
        &mut self,
        ty: &Type,
        name: &str,
        (option, value_ty): (usize, Option<Type>),
        args: &[Arg],
        found: Vec<Option<Type>>,
        lowered: Option<Vec<ir::Expr>>,
    ) -> Typed {
        let signature = Signature {
            name: format!("{ty}.{name}"),
            params: vec![ParamSignature {
                label: None,
                name: "value",
                ty: value_ty,
            }],
            returns: Some(ty.clone()),
        };
        let returns = self.check_args(signature.name.clone(), args, found, signature);
        Typed {
            ty: returns,
            expr: lowered
                .and_then(|args| <[_; 1]>::try_from(args).ok())
                .map(|[value]| ir::Expr::Choose {
                    choice: ty.id,
                    option,
                    value: Box::new(value),
                }),
        }
    }

    /// Checks the construction of a group, returning its type, or `None` if
    /// it can't be determined, along with what it compiles to.
    fn construct(&mut self, type_name: &Name, values: &[FieldValue]) -> Typed {
        // Values are checked first, so their errors come before the group's.
        let (found, lowered) = split(values.iter().map(|field| self.expr(&field.value)).collect());

        let Some(ty) = self.resolve_type(type_name) else {
            return Typed::UNKNOWN;
        };
        let with_error = |ty| Typed {
            ty: Some(ty),
            expr: None,
        };
        if self.choice_options.contains_key(&ty.id) {
            self.error(CheckError::ConstructedChoice(ty.clone()));
            return with_error(ty);
        }
        if self.primitives.contains_key(&ty.id) {
            self.error(CheckError::ConstructedPrimitive(ty.clone()));
            return with_error(ty);
        }
        // Only the type's own module can construct it, and only in the type's
        // `impl`s. Elsewhere, values come from its functions, such as
        // `Void.new()`. An unknown `impl` type isn't reported again.
        let declared_in = self.group_modules[ty.id];
        if declared_in != self.module {
            self.error(CheckError::ConstructedOutsideModule {
                ty: ty.clone(),
                module: self.modules[declared_in].display.clone(),
            });
            return with_error(ty);
        }
        match &self.impl_type {
            Some(Some(impl_type)) if *impl_type == ty => {}
            Some(None) => {}
            _ => {
                self.error(CheckError::ConstructedOutsideImpl(ty.clone()));
                return with_error(ty);
            }
        }
        let fields = self.group_fields[ty.id].clone();
        let name = type_name.to_string();

        // Every field must be given, in the order the group declares them.
        if values.len() != fields.len() {
            self.error(CheckError::WrongFieldCount {
                ty: name,
                expected: fields.len(),
                found: values.len(),
            });
            return with_error(ty);
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

        Typed {
            expr: lowered.map(|fields| ir::Expr::Construct {
                group: ty.id,
                fields,
            }),
            ty: Some(ty),
        }
    }

    /// Checks a call, returning the function's return type, or `None` if it
    /// can't be determined, along with what it compiles to.
    fn call(&mut self, callee: &Name, args: &[Arg]) -> Typed {
        // Arguments are checked first, so their errors come before the call's.
        let (found, lowered) = split(args.iter().map(|arg| self.expr(&arg.value)).collect());

        let index = match self.lookup(callee) {
            Ok(Binding::Function(index)) => index,
            Ok(Binding::Value { .. } | Binding::Global(_) | Binding::Type(_)) => {
                self.error(CheckError::NotAFunction(callee.to_string()));
                return Typed::UNKNOWN;
            }
            Err(error) => {
                self.lookup_error(callee, error);
                return Typed::UNKNOWN;
            }
        };
        let signature = self.functions[index].clone();
        Typed {
            ty: self.check_args(callee.to_string(), args, found, signature),
            expr: lowered.map(|args| ir::Expr::Call {
                function: index,
                args,
            }),
        }
    }

    /// Checks a call's arguments, whose types are `found`, against the
    /// signature of `function`, returning its return type.
    fn check_args(
        &mut self,
        function: String,
        args: &[Arg],
        found: Vec<Option<Type>>,
        signature: Signature,
    ) -> Option<Type> {
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

    /// Checks an operand of `op`, returning what it compiles to, or `None` if
    /// it has an error.
    fn operand(&mut self, op: &'static str, operand: &Expr, expected: Type) -> Option<ir::Expr> {
        let Typed { ty, expr } = self.expr(operand);
        if let Some(found) = ty
            && found != expected
        {
            self.error(CheckError::InvalidOperand {
                op,
                expected,
                found,
            });
        }
        expr
    }

    /// The program made of everything that's been checked, which must not have
    /// any errors.
    fn program(self) -> ir::Program {
        const NO_ERRORS: &str = "the program has no errors";

        let main = self.main();
        let type_id = |ty: &Option<Type>| ty.as_ref().expect(NO_ERRORS).id;
        let types = (0..self.group_fields.len())
            .map(|id| {
                let name = self.type_names[id].clone();
                if let Some(&primitive) = self.primitives.get(&id) {
                    ir::TypeDef::Primitive(primitive)
                } else if let Some(options) = self.choice_options.get(&id) {
                    let options = options.iter().map(|option| type_id(&option.ty));
                    ir::TypeDef::Choice {
                        name,
                        options: options.collect(),
                    }
                } else {
                    let fields = self.group_fields[id].iter().map(|field| type_id(&field.ty));
                    ir::TypeDef::Group {
                        name,
                        fields: fields.collect(),
                    }
                }
            })
            .collect();
        let globals = self
            .globals
            .into_iter()
            .map(|global| ir::Global {
                name: format!("{}::{}", self.modules[global.module].display, global.name),
                ty: match global.state {
                    GlobalState::Checked(Some(ty)) => ty.id,
                    _ => panic!("{NO_ERRORS}"),
                },
                value: global.lowered.expect(NO_ERRORS),
            })
            .collect();

        ir::Program {
            types,
            main,
            functions: self
                .lowered_functions
                .into_iter()
                .map(|function| function.expect(NO_ERRORS))
                .collect(),
            globals,
            global_order: self.global_order,
        }
    }

    /// Calls `fn main() -> Int` from `/main`, or `None` if there isn't one.
    fn main(&self) -> Option<ir::Expr> {
        let module = self
            .modules
            .iter()
            .find(|module| module.package.is_none() && module.path == ["main"])?;
        let Some(&Binding::Function(index)) = module.scope.get("main") else {
            return None;
        };
        let signature = &self.functions[index];
        if !signature.params.is_empty() || signature.returns != Some(self.std_type("Int")) {
            return None;
        }
        Some(ir::Expr::Call {
            function: index,
            args: vec![],
        })
    }
}

/// Whether `stmts` returns on every path. Both branches of an `if` must return
/// for it to count, so an `if` without an `else` never does. Every option of
/// a `match` must return, which is enough since every option is matched.
fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(_) => true,
        Stmt::Match { options, .. } => options.iter().all(|option| always_returns(&option.body)),
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
/// which is also its result type. Comparisons don't have one, since they can
/// compare more than one type.
fn binary_type(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => "Int",
        BinOp::And | BinOp::Or => "Bool",
        BinOp::Eq | BinOp::Ne => unreachable!("comparisons are checked by `comparison`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::parse;

    /// The standard library type called `name`.
    fn ty(name: &str) -> Type {
        let mut checker = Checker::default();
        checker.add_stdlib();
        checker.check_modules();
        checker.std_type(name)
    }

    /// Parses each `(path, src)` pair as a module, with paths like
    /// `/dir/other`.
    fn modules(files: &[(&str, &str)]) -> Vec<Module> {
        files
            .iter()
            .map(|(path, src)| Module {
                path: path[1..].split('/').map(str::to_string).collect(),
                program: parse(src).expect("source should parse"),
            })
            .collect()
    }

    /// Checks each `(path, src)` pair as a module, with paths like
    /// `/dir/other`.
    fn check_files(files: &[(&str, &str)]) -> Result<(), Vec<ModuleError>> {
        check(&modules(files)).map(|_| ())
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
    fn accepts_comparisons_of_integers_and_booleans() {
        let src = "
            let a: Bool = 1 == 2
            let b: Bool = 1 + 2 != 3
            let c: Bool = true == false
            let d: Bool = (a != b) == !c
            ";
        assert_eq!(check_body(src), Ok(()));
    }

    #[test]
    fn comparisons_produce_booleans() {
        assert_eq!(
            check_body("let x = (1 == 1) + 1"),
            Err(vec![invalid_operand("+")])
        );
    }

    #[test]
    fn rejects_comparing_different_types() {
        // The right operand must have the left operand's type.
        assert_eq!(
            check_body("let x = 1 == true"),
            Err(vec![invalid_operand("==")])
        );
        assert_eq!(
            check_body("let x = false != 0"),
            Err(vec![CheckError::InvalidOperand {
                op: "!=",
                expected: ty("Bool"),
                found: ty("Int"),
            }])
        );
    }

    #[test]
    fn rejects_comparing_other_types() {
        let incomparable = |op| CheckError::IncomparableOperand {
            op,
            found: ty("Void"),
        };
        assert_eq!(
            check_body("let x = Void.new() == Void.new()"),
            Err(vec![incomparable("==")])
        );
        assert_eq!(
            check_body("let x = Void.new() != 1"),
            Err(vec![incomparable("!=")])
        );
        assert_eq!(
            messages("let x = Void.new() == Void.new()"),
            vec!["operator `==` expects `Int` or `Bool`, found `Void`"]
        );
    }

    #[test]
    fn comparisons_with_unknown_operands_do_not_cascade() {
        assert_eq!(
            check_body("let x = y == 1"),
            Err(vec![CheckError::Undeclared("y".to_string())])
        );
        assert_eq!(
            check_body("let x = Void.new() == y"),
            Err(vec![
                CheckError::IncomparableOperand {
                    op: "==",
                    found: ty("Void"),
                },
                CheckError::Undeclared("y".to_string()),
            ])
        );
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
    fn own_names_come_before_std_names() {
        // `/main` and `stdlib/core` are different modules, so both can declare
        // `Int`, and in `/main` a bare `Int` means its own.
        assert_eq!(
            messages_top(
                "
                group Int {}
                let a: Int = 1
                let b: core::Int = 1
                fn f(x: main::Int) -> core::Bool {
                    return true
                }
                "
            ),
            ["cannot assign `Int` to `a`, which has type `Int`"]
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
    fn std_types_are_groups() {
        assert_eq!(
            check_body(
                "
                group Wrapper {
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
                    group Point {}
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
    fn groups_can_be_used_as_types() {
        assert_eq!(
            check_body(
                "
                group User {
                    let age: Int
                    var isAdmin: Bool
                }
                group Post {
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
    fn accepts_empty_and_void_groups() {
        assert_eq!(
            check_body(
                "
                group Empty {}
                group Nothing {
                    let value: Void
                    let empty: Empty
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn type_errors_use_group_names() {
        assert_eq!(
            messages(
                "
                group User {}
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
    fn different_groups_are_different_types() {
        assert_eq!(
            messages(
                "
                group A {
                    let x: Int
                }
                group B {
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
    fn groups_with_the_same_name_are_different_types() {
        // The inner `A` is a different group, even though it has the same
        // name and fields.
        assert_eq!(
            messages(
                "
                group A {}
                fn f(a: A) {
                    group A {}
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
                group User {
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
            messages(
                "
                group User {
                    let age: Int
                    var age: Bool
                }
                "
            ),
            ["`User` already has a field `age`"]
        );
    }

    #[test]
    fn fields_can_reuse_outer_names() {
        assert_eq!(
            check_body(
                "
                let age = 1
                group A {
                    let age: Int
                }
                group B {
                    let age: Int
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn top_level_groups_can_be_used_in_any_order() {
        assert_eq!(
            check_top(
                "
                fn f(user: User) -> Post {
                    return f(user: user)
                }
                group Post {
                    let author: User
                }
                group User {
                    let address: Address
                }
                group Address {}
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn nested_groups_must_be_declared_before_use() {
        assert_eq!(
            check_body(
                "
                fn outer() {
                    fn f(user: User) {}
                    group Post {
                        let author: User
                    }
                    group User {}
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
    fn groups_cannot_contain_themselves() {
        assert_eq!(
            check_body(
                "
                group Node {
                    let next: Node
                }
                fn f() {
                    group Inner {
                        let inner: Inner
                    }
                }
                "
            ),
            Err(vec![
                CheckError::RecursiveType("Node".to_string()),
                CheckError::RecursiveType("Inner".to_string()),
            ])
        );
        // Every group in a cycle is reported, but not one that only contains
        // a group in a cycle.
        assert_eq!(
            check_top(
                "
                group A {
                    let b: B
                }
                group B {
                    let c: C
                }
                group C {
                    let a: A
                }
                group D {
                    let a: A
                }
                "
            ),
            Err(vec![
                CheckError::RecursiveType("A".to_string()),
                CheckError::RecursiveType("B".to_string()),
                CheckError::RecursiveType("C".to_string()),
            ])
        );
    }

    #[test]
    fn groups_can_contain_the_same_group_twice() {
        assert_eq!(
            check_top(
                "
                group Line {
                    let start: Point
                    let end: Point
                }
                group Point {}
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_redeclared_groups() {
        assert_eq!(
            check_body("group A {}\ngroup A {}"),
            Err(vec![CheckError::Redeclared("A".to_string())])
        );
        // A function can declare its own `Int`, which is a different type,
        // but literals still have the standard library's `Int`.
        assert_eq!(
            messages(
                "
                fn f() {
                    group Int {}
                    let x: Int = 1
                }
                "
            ),
            ["cannot assign `Int` to `x`, which has type `Int`"]
        );
    }

    #[test]
    fn groups_share_names_with_values_and_functions() {
        for src in [
            "group User {}\nlet User = 1",
            "let User = 1\ngroup User {}",
            "group User {}\nfn User() {}",
            "fn User() {}\ngroup User {}",
            "fn f(User: Int) {\ngroup User {}\n}",
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
                group User {}
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
                    group S {
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
    fn groups_are_scoped_like_other_names() {
        assert_eq!(
            check_body(
                "
                if true {
                    group A {}
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
                    group A {}
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
                    group A {}
                } else {
                    group A {}
                }
                "
            ),
            Ok(())
        );
        // Blocks can't shadow, but functions can.
        assert_eq!(
            check_body(
                "
                group A {}
                if true {
                    group A {}
                }
                fn f() {
                    group A {}
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
        assert!(check(&[]).is_ok());
    }

    /// Whether checking `src` as `path` finds the program's `main`.
    fn has_main(path: &str, src: &str) -> bool {
        check(&modules(&[(path, src)]))
            .expect("source should check")
            .main
            .is_some()
    }

    #[test]
    fn main_must_return_int_from_the_main_module() {
        assert!(has_main("/main", "fn main() -> Int { return 0 }"));
        assert!(!has_main("/main", "fn main() {}"));
        assert!(!has_main(
            "/main",
            "fn main(code: Int) -> Int { return code }"
        ));
        assert!(!has_main("/main", "let main = 0"));
        assert!(!has_main("/other", "fn main() -> Int { return 0 }"));
        assert!(!has_main("/dir/main", "fn main() -> Int { return 0 }"));
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
                    group Point {}
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
                ("/main", "let a = x\nlet b = lib::x\nlet c = /lib::x"),
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
                    use stdlib/core
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
                    let y: Int = a::f()
                    let z: Bool = b::f()
                    let w: Bool = /dir/b::f()
                    ",
                ),
                ("/a", a),
                ("/dir/b", b),
            ]),
            ["/main: `f` is declared in `/a` and `/dir/b`, so it must be qualified"]
        );
    }

    #[test]
    fn own_names_come_before_imported_names() {
        let a = "fn f() -> Int {\nreturn 1\n}";
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /a
                    fn f() -> Bool {
                        return f()
                    }
                    let x: Bool = f()
                    let y: Int = a::f()
                    let z: Bool = main::f()
                    ",
                ),
                ("/a", a),
            ]),
            Ok(())
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
                    let x = other::value
                    let y: Int = /a/other::value
                    let z: Bool = ./b/other::value
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
                    "use /lib\nlet x = lib::missing\nlet y: lib::Missing = 1",
                ),
                ("/lib", ""),
            ]),
            [
                "/main: `lib::missing` is not declared",
                "/main: unknown type `lib::Missing`",
            ]
        );
    }

    #[test]
    fn std_names_can_be_qualified() {
        assert_eq!(
            check_top("let x: core::Int = 1\nlet y: Int = x\nlet z: stdlib/core::Bool = true"),
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
                    group A {
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
                    group B {}
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
    fn groups_cannot_contain_themselves_across_modules() {
        assert_eq!(
            file_messages(&[
                ("/a", "use /b\ngroup A {\nlet b: B\n}"),
                ("/b", "use /a\ngroup B {\nlet a: A\n}"),
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
                        return a + b + lib::b
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
                "/main: functions can't use `lib::b`, because it is declared outside the function",
                "/main: functions can't use `c`, because it is declared outside the function",
            ]
        );
    }

    #[test]
    fn rejects_redeclaration_within_a_module() {
        assert_eq!(
            check_top("group a {}\nfn a() {}\nlet a = 1"),
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
        group User {
            let age: Int
            var isAdmin: Bool
        }
        impl User {
            static fn new(age: Int, isAdmin: Bool) -> User {
                return User { age: age, isAdmin: isAdmin }
            }
        }
    ";

    #[test]
    fn constructs_groups() {
        assert_eq!(
            check_top(&format!(
                "
                {USER}
                group Team {{
                    let lead: User
                    let size: Int
                }}
                group Empty {{}}
                impl Team {{
                    static fn new() -> Team {{
                        return Team {{ lead: User.new(age: 30, isAdmin: false), size: 1 + 2 }}
                    }}
                    fn withLead(lead: User) -> Team {{
                        return Team {{ lead: lead, size: 1 }}
                    }}
                }}
                impl Empty {{
                    static fn new() -> Empty {{
                        let empty = Empty {{}}
                        return empty
                    }}
                }}

                let admin: User = User.new(age: 42, isAdmin: true)
                let team = Team.new().withLead(lead: admin)
                let empty = Empty.new()
                "
            )),
            Ok(())
        );
    }

    #[test]
    fn constructors_give_the_group_type() {
        assert_eq!(
            messages_top(&format!(
                "
                {USER}
                impl User {{
                    static fn make() {{
                        let x: Int = User {{ age: 1, isAdmin: true }}
                        let y = User {{ age: 1, isAdmin: true }} + 1
                    }}
                }}
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
            messages_top(&format!(
                "
                {USER}
                impl User {{
                    static fn make() {{
                        let a = User {{ age: 1 }}
                        let b = User {{ age: 1, isAdmin: true, extra: 2 }}
                        let c = User {{}}
                    }}
                }}
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
            messages_top(&format!(
                "
                {USER}
                impl User {{
                    static fn make() {{
                        let a = User {{ isAdmin: true, age: 1 }}
                        let b = User {{ age: 1, admin: true }}
                    }}
                }}
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
            messages_top(&format!(
                "
                {USER}
                impl User {{
                    static fn make() {{
                        let a = User {{ age: true, isAdmin: 1 }}
                        let b = User {{ age: -true, isAdmin: false }}
                    }}
                }}
                "
            )),
            [
                "field `age` of `User` expects `Int`, found `Bool`",
                "field `isAdmin` of `User` expects `Bool`, found `Int`",
                "operator `-` expects `Int`, found `Bool`",
            ]
        );
    }

    #[test]
    fn constructors_need_a_group() {
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
            messages_top(
                "
                group Box {
                    let value: Missing
                }
                impl Box {
                    static fn make() -> Box {
                        return Box { value: 1 }
                    }
                }
                "
            ),
            ["unknown type `Missing`"]
        );
    }

    #[test]
    fn groups_are_only_constructed_in_their_own_module() {
        // Even in an `impl` of the type, another module can't construct it,
        // but it can use the type's own functions.
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use ./users/user
                    impl User {
                        static fn admin(age: Int) -> User {
                            return User { age: age, isAdmin: true }
                        }
                        static fn guest() -> User {
                            return User.new(age: 3, isAdmin: false)
                        }
                    }
                    let a = user::User { age: 1, isAdmin: true }
                    let b: User = User.guest()
                    ",
                ),
                ("/users/user", USER),
            ]),
            [
                "/main: `User` can only be constructed in `/users/user`, which declares it",
                "/main: `User` can only be constructed in `/users/user`, which declares it",
            ]
        );
    }

    #[test]
    fn only_literals_create_ints_and_bools() {
        assert_eq!(
            check_body("let v: Void = Void.new()\nlet w: core::Void = core::Void.new()"),
            Ok(())
        );
        assert_eq!(
            messages(
                "
                let a = Int {}
                let b = core::Bool { value: c }
                let d = 1.value
                "
            ),
            [
                "`Int` is a primitive, so it can only be created by the compiler, such as with a literal",
                "`c` is not declared",
                "`Bool` is a primitive, so it can only be created by the compiler, such as with a literal",
                "`Int` has no field or method `value`",
            ]
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
                    impl User {{
                        static fn make() -> User {{
                            return User {{ age: age, isAdmin: true }}
                        }}
                    }}
                    // A value can't contain itself.
                    let b: User = User.new(age: b.age, isAdmin: true)
                    "
                ),
            )]),
            [
                "/main: functions can't use `age`, because it is declared outside the function",
                "/main: `b` depends on its own value",
            ]
        );
    }

    #[test]
    fn rejects_qualifying_with_identifiers() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /lib
                    group Point {}
                    let x = 1
                    let a = x::y
                    let b = Point::origin
                    let c = helper::y
                    let d = missing::y
                    fn f(user: Int) -> Int {
                        let local = 1
                        return local::y + user::y + x::y
                    }
                    ",
                ),
                ("/lib", "fn helper() {}"),
            ]),
            [
                "/main: `x` is not a module, so it can't be followed by `::`",
                "/main: `Point` is not a module, so it can't be followed by `::`",
                "/main: `helper` is not a module, so it can't be followed by `::`",
                "/main: `missing` is not an imported module",
                "/main: `local` is not a module, so it can't be followed by `::`",
                "/main: `user` is not a module, so it can't be followed by `::`",
                // Even though functions can't use `x`, it's still declared.
                "/main: `x` is not a module, so it can't be followed by `::`",
            ]
        );
    }

    #[test]
    fn modules_and_identifiers_can_share_names() {
        // `::` always follows a module, so `user::age` is the module even
        // where `user` is also a value.
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /user
                    let user = 1
                    fn f(user: Int) -> Int {
                        return user::age() + user
                    }
                    ",
                ),
                ("/user", "fn age() -> Int {\nreturn 2\n}"),
            ]),
            Ok(())
        );
    }

    const COUNTER: &str = "
        group Counter {}
        impl Counter {
            static fn new() -> Counter {
                return Counter {}
            }
            fn me() -> Counter {
                return self
            }
            fn once() -> Int {
                return 1
            }
            fn twice() -> Int {
                return self.once() + self.once()
            }
            fn add(amount: Int) -> Int {
                return self.once() + amount
            }
        }
    ";

    #[test]
    fn calls_methods() {
        assert_eq!(
            check_top(&format!(
                "
                {COUNTER}
                let counter = Counter.new()
                let a: Int = counter.twice() + counter.add(amount: 2)
                let b: Counter = Counter.new().me().me()
                fn f(counter: Counter) -> Int {{
                    counter.once()
                    return counter.me().twice()
                }}
                "
            )),
            Ok(())
        );
    }

    #[test]
    fn methods_call_each_other_through_self() {
        assert_eq!(
            messages_top(
                "
                group Counter {}
                impl Counter {
                    fn once() -> Int {
                        return 1
                    }
                    fn twice() -> Int {
                        return once() + once()
                    }
                }
                "
            ),
            ["`once` is not declared", "`once` is not declared"]
        );
        // A bare name finds a function outside the `impl` instead.
        assert_eq!(
            messages_top(
                "
                fn once() -> Bool {
                    return true
                }
                group Counter {}
                impl Counter {
                    fn once() -> Int {
                        return 1
                    }
                    fn twice() -> Int {
                        return once() + self.once()
                    }
                }
                "
            ),
            ["operator `+` expects `Int`, found `Bool`"]
        );
    }

    #[test]
    fn self_is_only_available_in_methods() {
        assert_eq!(
            messages_top(&format!(
                "
                {COUNTER}
                let x = self
                fn f() -> Int {{
                    return self.once()
                }}
                impl Counter {{
                    fn nested() {{
                        fn inner() -> Counter {{
                            return self
                        }}
                    }}
                }}
                "
            )),
            [
                "`self` can only be used inside an `impl`",
                "`self` can only be used inside an `impl`",
                "functions can't use `self`, because it is declared outside the function",
            ]
        );
    }

    #[test]
    fn rejects_unknown_methods() {
        assert_eq!(
            messages_top(&format!(
                "
                {COUNTER}
                let a = Counter.new().missing()
                let b = 1.double()
                // Unknown receivers aren't reported again.
                let c = missing.f(x: y)
                "
            )),
            [
                "`Counter` has no field or method `missing`",
                "`Int` has no field or method `double`",
                "`missing` is not declared",
                "`y` is not declared",
            ]
        );
    }

    #[test]
    fn checks_method_arguments() {
        assert_eq!(
            messages_top(&format!(
                "
                {COUNTER}
                let c = Counter.new()
                let a = c.add()
                let b = c.add(by: 1)
                let d = c.add(amount: true)
                let e: Bool = c.add(amount: 1)
                "
            )),
            [
                "`Counter.add` takes 1 argument, found 0",
                "argument to `Counter.add` should be labelled `amount`, found `by`",
                "argument `amount` of `Counter.add` expects `Int`, found `Bool`",
                "cannot assign `Int` to `e`, which has type `Bool`",
            ]
        );
    }

    #[test]
    fn checks_method_bodies() {
        assert_eq!(
            messages_top(
                "
                group Counter {}
                impl Counter {
                    fn a() -> Int {
                        return true
                    }
                    fn b() -> Int {}
                    fn c(x: Missing) {}
                }
                "
            ),
            [
                "unknown type `Missing`",
                "`a` must return `Int`, found `Bool`",
                "`b` must return a value of type `Int` on every path",
            ]
        );
    }

    #[test]
    fn impls_can_extend_std_types() {
        assert_eq!(
            check_top(
                "
                impl Int {
                    fn doubled() -> Int {
                        return self * 2
                    }
                }
                impl core::Bool {
                    fn flipped() -> Bool {
                        return !self
                    }
                }
                let x: Int = 21.doubled().doubled()
                let y: Bool = true.flipped()
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn types_can_have_several_impls() {
        assert_eq!(
            check_top(&format!(
                "
                {COUNTER}
                impl Counter {{
                    fn thrice() -> Int {{
                        return self.twice() + self.once()
                    }}
                }}
                let x = Counter.new().thrice()
                "
            )),
            Ok(())
        );
        assert_eq!(
            messages_top(&format!(
                "
                {COUNTER}
                impl Counter {{
                    fn once() -> Int {{
                        return 2
                    }}
                }}
                "
            )),
            ["`Counter` already has a method `once`"]
        );
    }

    #[test]
    fn methods_can_share_names_with_other_declarations() {
        assert_eq!(
            check_top(
                "
                group Counter {}
                fn count() {}
                let total = 1
                impl Counter {
                    fn count() {}
                    fn total() {}
                    fn Counter() {}
                }
                group Other {}
                impl Other {
                    fn count() {}
                }
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn impls_work_across_modules() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /users/user
                    use /users/score
                    let score: Int = User.new().score()
                    ",
                ),
                (
                    "/users/user",
                    "group User {}\nimpl User {\nstatic fn new() -> User {\nreturn User {}\n}\n}"
                ),
                (
                    "/users/score",
                    "
                    use ./user
                    impl User {
                        fn score() -> Int {
                            return 1
                        }
                    }
                    ",
                ),
            ]),
            Ok(())
        );
        assert_eq!(
            file_messages(&[
                ("/main", "group User {}\nimpl User {\nfn f() {}\n}"),
                ("/other", "use /main\nimpl main::User {\nfn f() {}\n}"),
            ]),
            ["/other: `User` already has a method `f`"]
        );
    }

    #[test]
    fn methods_need_their_module_imported() {
        let score = "
            use /user
            impl User {
                fn score() -> Int {
                    return 1
                }
            }
        ";
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /user
                    let a = User.new().score()
                    fn f(user: User) {
                        user.score()
                    }
                    ",
                ),
                (
                    "/user",
                    "group User {}\nimpl User {\nstatic fn new() -> User {\nreturn User {}\n}\n}"
                ),
                ("/score", score),
            ]),
            [
                "/main: `User.score` is declared in `/score`, which is not imported",
                "/main: `User.score` is declared in `/score`, which is not imported",
            ]
        );
        // Imports aren't transitive, even if the type's module imports it.
        assert_eq!(
            file_messages(&[
                ("/main", "use /user\nlet a = User.new().score()"),
                (
                    "/user",
                    "use /score\ngroup User {}\nimpl User {\nstatic fn new() -> User {\nreturn User {}\n}\n}\nlet b = User.new().score()"
                ),
                ("/score", score),
            ]),
            ["/main: `User.score` is declared in `/score`, which is not imported"]
        );
        // A method's own module can use it, even without importing anything.
        assert_eq!(
            check_files(&[
                (
                    "/user",
                    "group User {}\nimpl User {\nstatic fn new() -> User {\nreturn User {}\n}\n}"
                ),
                ("/score", &format!("{score}\nlet c = User.new().score()")),
            ]),
            Ok(())
        );
    }

    #[test]
    fn methods_on_std_types_need_their_module_imported() {
        // The type comes from the standard library, which is always imported,
        // but the method comes from `/ints`, which must be imported.
        assert_eq!(
            file_messages(&[
                ("/main", "use /ints\nlet a = 1.doubled()"),
                ("/other", "let b = 1.doubled()"),
                (
                    "/ints",
                    "
                    impl Int {
                        fn doubled() -> Int {
                            return self * 2
                        }
                    }
                    ",
                ),
            ]),
            ["/other: `Int.doubled` is declared in `/ints`, which is not imported"]
        );
    }

    #[test]
    fn functions_need_their_module_imported() {
        assert_eq!(
            file_messages(&[
                ("/main", "use /a\nlet x = f()\nlet y = b::f()"),
                ("/a", "use /b"),
                ("/b", "fn f() -> Int {\nreturn 1\n}"),
            ]),
            [
                "/main: `f` is not declared",
                "/main: `b` is not an imported module",
            ]
        );
    }

    #[test]
    fn impls_need_a_type() {
        assert_eq!(
            messages_top(
                "
                let x = 1
                impl x {}
                impl Missing {
                    // `self` has an unknown type, so this isn't reported.
                    fn f() -> Int {
                        return self
                    }
                }
                "
            ),
            ["`x` is not a type", "unknown type `Missing`"]
        );
    }

    #[test]
    fn methods_are_hoisted() {
        assert_eq!(
            check_top(
                "
                let x: Int = Counter.new().once()
                impl Counter {
                    static fn new() -> Counter {
                        return Counter {}
                    }
                    fn once() -> Int {
                        return 1
                    }
                }
                group Counter {}
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn self_is_immutable_and_not_a_name() {
        // `self` isn't declared like other names, so it doesn't clash with a
        // group's methods or the program's names.
        assert_eq!(
            check_top(
                "
                group Box {}
                impl Box {
                    fn same(as other: Box) -> Box {
                        let copy: Box = self
                        return other
                    }
                }
                "
            ),
            Ok(())
        );
    }

    const TEAM: &str = "
        group User {
            let age: Int
            var isAdmin: Bool
        }
        impl User {
            static fn new(age: Int, isAdmin: Bool) -> User {
                return User { age: age, isAdmin: isAdmin }
            }
        }
        group Team {
            let lead: User
            let size: Int
        }
        impl Team {
            static fn new(lead: User, size: Int) -> Team {
                return Team { lead: lead, size: size }
            }
        }
    ";

    #[test]
    fn reads_fields() {
        assert_eq!(
            check_top(&format!(
                "
                {TEAM}
                let user = User.new(age: 42, isAdmin: true)
                let team = Team.new(lead: user, size: 3)
                let age: Int = user.age
                let isAdmin: Bool = team.lead.isAdmin
                let total: Int = team.lead.age + team.size
                fn leadAge(of team: Team) -> Int {{
                    return team.lead.age
                }}
                "
            )),
            Ok(())
        );
    }

    #[test]
    fn fields_have_their_declared_types() {
        assert_eq!(
            messages_top(&format!(
                "
                {TEAM}
                let user = User.new(age: 42, isAdmin: true)
                let a: Bool = user.age
                let b = user.isAdmin + 1
                let c: Int = Team.new(lead: user, size: 1).lead
                "
            )),
            [
                "cannot assign `Int` to `a`, which has type `Bool`",
                "operator `+` expects `Int`, found `Bool`",
                "cannot assign `User` to `c`, which has type `Int`",
            ]
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        assert_eq!(
            messages_top(&format!(
                "
                {TEAM}
                let user = User.new(age: 42, isAdmin: true)
                let a = user.name
                let b = 1.value
                // Not reported again.
                let c = user.name.length
                let d = missing.field
                "
            )),
            [
                "`User` has no field or method `name`",
                "`Int` has no field or method `value`",
                "`User` has no field or method `name`",
                "`missing` is not declared",
            ]
        );
    }

    #[test]
    fn unknown_field_types_are_not_reported_again() {
        assert_eq!(
            messages_top(
                "
                group Box {
                    let value: Missing
                }
                fn f(box: Box) -> Int {
                    return box.value + true
                }
                "
            ),
            [
                "unknown type `Missing`",
                "operator `+` expects `Int`, found `Bool`",
            ]
        );
    }

    #[test]
    fn methods_read_fields_through_self() {
        assert_eq!(
            check_top(&format!(
                "
                {TEAM}
                impl User {{
                    fn nextAge() -> Int {{
                        return self.age + 1
                    }}
                }}
                let x: Int = User.new(age: 1, isAdmin: true).nextAge()
                "
            )),
            Ok(())
        );
    }

    #[test]
    fn fields_and_methods_share_names() {
        assert_eq!(
            messages_top(&format!(
                "
                {TEAM}
                impl User {{
                    fn age() -> Int {{
                        return 1
                    }}
                    fn nextAge() -> Int {{
                        return 2
                    }}
                }}
                impl Team {{
                    fn size() {{}}
                }}
                "
            )),
            [
                "`User` already has a field `age`",
                "`Team` already has a field `size`",
            ]
        );
    }

    #[test]
    fn brackets_decide_whether_members_are_called() {
        assert_eq!(
            messages_top(&format!(
                "
                {TEAM}
                impl User {{
                    fn nextAge() -> Int {{
                        return self.age() + self.nextAge
                    }}
                }}
                fn f(user: User) {{
                    // Arguments are still checked.
                    user.isAdmin(x: missing)
                }}
                "
            )),
            [
                "`User.age` is a field, not a method",
                "`User.nextAge` is a method, not a value",
                "`missing` is not declared",
                "`User.isAdmin` is a field, not a method",
            ]
        );
    }

    #[test]
    fn fields_follow_capture_rules() {
        assert_eq!(
            messages_top(&format!(
                "
                {TEAM}
                let admin = User.new(age: 42, isAdmin: true)
                let age = admin.age
                fn f() -> Int {{
                    return admin.age
                }}
                "
            )),
            ["functions can't use `admin`, because it is declared outside the function"]
        );
    }

    #[test]
    fn fields_need_no_import() {
        // Fields are part of their group, so a value's fields can be read
        // without importing the module that declares it.
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /make
                    let age: Int = make().age
                    ",
                ),
                (
                    "/make",
                    "
                    use /user
                    fn make() -> User {
                        return User.new(age: 1)
                    }
                    ",
                ),
                (
                    "/user",
                    "
                    group User {
                        let age: Int
                    }
                    impl User {
                        static fn new(age: Int) -> User {
                            return User { age: age }
                        }
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn suggests_colons_after_modules() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /user
                    let a = user.User
                    let b = user.make()
                    // A value with the module's name takes precedence.
                    fn f(user: Int) -> Int {
                        return user.age
                    }
                    ",
                ),
                ("/user", "group User {}\nfn make() {}"),
            ]),
            [
                "/main: `user` is a module, not a value, so use `user::User` to refer to a name in it",
                "/main: `user` is a module, not a value, so use `user::make` to refer to a name in it",
                "/main: `Int` has no field or method `age`",
            ]
        );
    }

    const POINT: &str = "
        group Point {
            let x: Int
        }
        impl Int {
            static fn zero() -> Int {
                return 0
            }
        }
        impl Point {
            static fn origin() -> Point {
                return Point { x: Int.zero() }
            }
            static fn at(x: Int) -> Point {
                return Point { x: x }
            }
            fn moved(by amount: Int) -> Point {
                return Point.at(x: self.x + amount)
            }
        }
    ";

    #[test]
    fn calls_static_functions_on_types() {
        assert_eq!(
            check_top(&format!(
                "
                {POINT}
                let zero: Int = Int.zero()
                let alsoZero: Int = core::Int.zero()
                let origin: Point = Point.origin()
                let moved: Point = Point.at(x: 1).moved(by: 2)
                fn f() -> Int {{
                    Point.origin()
                    return main::Point.origin().x
                }}
                "
            )),
            Ok(())
        );
    }

    #[test]
    fn static_functions_on_types_from_other_modules() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /geometry
                    let a: Point = geometry::Point.origin()
                    let b: Point = Point.origin()
                    ",
                ),
                (
                    "/geometry",
                    "
                    group Point {}
                    impl Point {
                        static fn origin() -> Point {
                            return Point {}
                        }
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn static_functions_have_no_self() {
        assert_eq!(
            messages_top(
                "
                impl Int {
                    static fn zero() -> Int {
                        return self
                    }
                    static fn one() -> Int {
                        fn nested() -> Int {
                            return self
                        }
                        return 1
                    }
                }
                "
            ),
            [
                "`self` can't be used in a static function",
                "`self` can only be used inside an `impl`",
            ]
        );
    }

    #[test]
    fn static_functions_are_used_on_types_not_values() {
        assert_eq!(
            messages_top(&format!(
                "
                {POINT}
                let a = 1.zero()
                let b = Point.origin().origin()
                let c = 1.zero
                "
            )),
            [
                "`Int.zero` is a static function, so it must be used on `Int` itself rather than a value",
                "`Point.origin` is a static function, so it must be used on `Point` itself rather than a value",
                "`Int.zero` is a static function, so it must be used on `Int` itself rather than a value",
            ]
        );
    }

    #[test]
    fn methods_and_fields_are_used_on_values_not_types() {
        assert_eq!(
            messages_top(&format!(
                "
                {POINT}
                let a = Point.moved(by: 1)
                let b = Point.x
                let c = Point.x()
                "
            )),
            [
                "`Point.moved` is a method, so it must be used on a value of `Point` rather than `Point` itself",
                "`Point.x` is a field, so it must be used on a value of `Point` rather than `Point` itself",
                "`Point.x` is a field, so it must be used on a value of `Point` rather than `Point` itself",
            ]
        );
    }

    #[test]
    fn static_functions_must_be_called() {
        assert_eq!(
            messages_top(&format!(
                "
                {POINT}
                let a = Int.zero
                // A type on its own is still not a value.
                let b = Int
                "
            )),
            [
                "`Int.zero` is a static function, not a value",
                "`Int` is a type, not a value",
            ]
        );
    }

    #[test]
    fn checks_static_function_calls() {
        assert_eq!(
            messages_top(&format!(
                "
                {POINT}
                let a = Int.zero(1)
                let b = Point.at(y: 1)
                let c = Point.at(x: true)
                let d: Bool = Int.zero()
                let e = Int.missing()
                "
            )),
            [
                "`Int.zero` takes 0 arguments, found 1",
                "argument to `Point.at` should be labelled `x`, found `y`",
                "argument `x` of `Point.at` expects `Int`, found `Bool`",
                "cannot assign `Int` to `d`, which has type `Bool`",
                "`Int` has no static function `missing`",
            ]
        );
        assert_eq!(
            messages_top(
                "
                impl Int {
                    static fn zero() -> Int {
                        return true
                    }
                }
                "
            ),
            ["`zero` must return `Int`, found `Bool`"]
        );
    }

    #[test]
    fn static_functions_have_their_own_names() {
        // Separate from the type's fields and methods, which share names with
        // each other.
        assert_eq!(
            check_top(
                "
                group Point {
                    let x: Int
                }
                impl Point {
                    static fn x() -> Bool {
                        return true
                    }
                    fn make() -> Int {
                        return 1
                    }
                    static fn make() -> Point {
                        return Point { x: 0 }
                    }
                }
                let a: Int = Point.make().x
                let b: Bool = Point.x()
                let c: Int = Point.make().make()
                "
            ),
            Ok(())
        );
        assert_eq!(
            messages_top(
                "
                group Point {}
                impl Point {
                    static fn make() {}
                }
                impl Point {
                    static fn make() {}
                    fn make() {}
                    fn make() {}
                }
                "
            ),
            [
                "`Point` already has a static function `make`",
                "`Point` already has a method `make`",
            ]
        );
    }

    #[test]
    fn static_functions_need_their_module_imported() {
        let ints = "
            impl Int {
                static fn zero() -> Int {
                    return 0
                }
            }
        ";
        assert_eq!(
            file_messages(&[
                ("/main", "use /ints\nlet a = Int.zero()"),
                ("/other", "let b = Int.zero()"),
                ("/ints", ints),
            ]),
            ["/other: `Int.zero` is declared in `/ints`, which is not imported"]
        );
    }

    #[test]
    fn static_functions_are_hoisted() {
        assert_eq!(
            check_top(
                "
                let p: Point = Point.origin()
                impl Point {
                    static fn origin() -> Point {
                        return Point {}
                    }
                }
                group Point {}
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn stdlib_is_a_package() {
        assert_eq!(
            file_messages(&[(
                "/main",
                "
                use stdlib
                use stdlib/core
                use stdlib/missing
                let a: stdlib::Int = 1
                let b: stdlib/core::Int = 1
                ",
            )]),
            [
                "/main: `stdlib` is a package, so use one of its modules, such as `stdlib/core`",
                "/main: cannot find module `stdlib/missing`",
                "/main: `stdlib` is a package, so use one of its modules, such as `stdlib/core`",
            ]
        );
    }

    #[test]
    fn only_the_stdlib_can_declare_primitives() {
        assert_eq!(
            check_top(
                "primitive Int
primitive Float"
            ),
            Err(vec![
                CheckError::PrimitiveOutsideStdlib("Int".to_string()),
                CheckError::PrimitiveOutsideStdlib("Float".to_string()),
            ])
        );
        assert_eq!(
            messages_top("primitive Int"),
            ["`Int` can't be a primitive, since only the standard library can declare them"]
        );
    }

    #[test]
    fn relative_paths_stay_in_their_package() {
        // `./core` from the project looks in the project, not the standard
        // library.
        assert_eq!(
            file_messages(&[("/main", "use ./core")]),
            ["/main: cannot find module `./core`"]
        );
        assert_eq!(
            check_files(&[("/main", "use ./core"), ("/core", "group Int {}")]),
            Ok(())
        );
    }

    #[test]
    fn project_modules_can_share_names_with_stdlib_modules() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /core
                    let a: Bool = flag
                    let b: Bool = /core::flag
                    let c: Int = stdlib/core::Int.zero()
                    let d = core::flag
                    ",
                ),
                (
                    "/core",
                    "
                    let flag = true
                    impl Int {
                        static fn zero() -> Int {
                            return 0
                        }
                    }
                    ",
                ),
            ]),
            ["/main: `core` could be `stdlib/core` or `/core`, so the full path must be used",]
        );
    }

    #[test]
    fn programs_import_core_without_a_use() {
        assert_eq!(
            check_top("let x: Void = Void.new()\nlet y: Bool = true\nlet z: core::Int = 1"),
            Ok(())
        );
    }

    #[test]
    fn imports_every_module_in_a_directory() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use ./users/*
                    let user: User = User.new(age: maxAge)
                    let admin: Admin = Admin.promote(user: user)
                    ",
                ),
                (
                    "/users/user",
                    "
                    group User {
                        let age: Int
                    }
                    impl User {
                        static fn new(age: Int) -> User {
                            return User { age: age }
                        }
                    }
                    let maxAge = 120
                    ",
                ),
                (
                    "/users/admin",
                    "
                    use ./user
                    group Admin {}
                    impl Admin {
                        static fn promote(user: User) -> Admin {
                            return Admin {}
                        }
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn directory_imports_include_subdirectories() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "use /users/*\nlet a = top\nlet b = nested\nlet c = deeper\nlet d = outside",
                ),
                ("/users/user", "let top = 1"),
                ("/users/admins/admin", "let nested = 2"),
                ("/users/admins/super/root", "let deeper = 3"),
                ("/other/thing", "let outside = 4"),
                // Only whole directory names match, not prefixes.
                ("/usersExtra/x", "let outside = 5"),
            ]),
            ["/main: `outside` is not declared"]
        );
    }

    #[test]
    fn root_directory_imports_cover_the_whole_project() {
        assert_eq!(
            check_files(&[
                ("/main", "use /*\nlet x = a + b + c"),
                ("/a", "let a = 1"),
                ("/dir/b", "let b = 2"),
                ("/dir/sub/c", "let c = 3"),
            ]),
            Ok(())
        );
        // `./*` from inside a directory doesn't reach outside it.
        assert_eq!(
            file_messages(&[
                ("/dir/main", "use ./*\nlet x = b + a"),
                ("/a", "let a = 1"),
                ("/dir/sub/b", "let b = 2"),
            ]),
            ["/dir/main: `a` is not declared"]
        );
    }

    #[test]
    fn directory_imports_are_relative_or_absolute() {
        assert_eq!(
            check_files(&[
                ("/dir/main", "use ./*\nuse /*\nlet x = sibling + root"),
                ("/dir/sibling", "let sibling = 1"),
                ("/root", "let root = 2"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn directory_imports_skip_the_importing_module() {
        // Only `/main` is in the root, so `/*` has nothing else to import.
        assert_eq!(
            file_messages(&[("/main", "use ./*\nuse /missing/*")]),
            [
                "/main: `./*` doesn't match any modules that can be imported",
                "/main: `/missing/*` doesn't match any modules that can be imported",
            ]
        );
    }

    #[test]
    fn directory_imports_can_import_the_stdlib() {
        // `stdlib/core` is imported anyway, so this changes nothing.
        assert_eq!(
            check_files(&[("/main", "use stdlib/*\nlet a: core::Int = 1")]),
            Ok(())
        );
    }

    #[test]
    fn names_from_directory_imports_can_be_ambiguous() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use ./shapes/*
                    let a = size
                    let b: Int = circle::size
                    let c: Bool = square::size
                    ",
                ),
                ("/shapes/circle", "let size = 1"),
                ("/shapes/square", "let size = true"),
            ]),
            [
                "/main: `size` is declared in `/shapes/circle` and `/shapes/square`, so it must be qualified",
            ]
        );
    }

    #[test]
    fn importing_a_directory_and_its_modules_is_harmless() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "use ./lib/*\nuse ./lib/a\nuse ./lib/*\nlet x = a + b"
                ),
                ("/lib/a", "let a = 1"),
                ("/lib/b", "let b = 2"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn creates_void_values() {
        assert_eq!(
            check_top(
                "
                let a: Void = Void.new()
                fn nothing() {}
                let b: Void = nothing()
                fn f() -> Void {
                    return core::Void.new()
                }
                "
            ),
            Ok(())
        );
        assert_eq!(
            messages_top("let a: Int = Void.new()\nlet b = Void.new().new()"),
            [
                "cannot assign `Void` to `a`, which has type `Int`",
                "`Void.new` is a static function, so it must be used on `Void` itself rather than a value",
            ]
        );
    }

    #[test]
    fn groups_are_only_constructed_in_their_impls() {
        assert_eq!(
            messages_top(
                "
                group Point {}
                group Other {}
                let a = Point {}
                fn f() -> Point {
                    return Point {}
                }
                impl Other {
                    static fn make() -> Point {
                        return Point {}
                    }
                    fn method() -> Point {
                        return Point {}
                    }
                }
                let b = Void {}
                "
            ),
            [
                "`Point` can only be constructed inside an `impl Point`",
                "`Point` can only be constructed inside an `impl Point`",
                "`Point` can only be constructed inside an `impl Point`",
                "`Point` can only be constructed inside an `impl Point`",
                "`Void` can only be constructed in `stdlib/core`, which declares it",
            ]
        );
    }

    #[test]
    fn methods_static_functions_and_nested_functions_can_construct() {
        assert_eq!(
            check_top(
                "
                group Point {}
                impl Point {
                    static fn origin() -> Point {
                        return Point {}
                    }
                    fn copy() -> Point {
                        fn make() -> Point {
                            return Point {}
                        }
                        return make()
                    }
                }
                let p: Point = Point.origin().copy()
                "
            ),
            Ok(())
        );
    }

    #[test]
    fn unknown_impl_types_do_not_report_constructors() {
        assert_eq!(
            messages_top(
                "
                group Point {}
                impl Missing {
                    static fn make() -> Point {
                        return Point {}
                    }
                }
                "
            ),
            ["unknown type `Missing`"]
        );
    }

    /// A choice for tests, with options holding values of different types.
    const SHAPE: &str = "
        choice Shape {
            circle: Int
            empty
            nothing: Void
        }
        ";

    #[test]
    fn accepts_choices() {
        let src = format!(
            "
            {SHAPE}
            let a: Shape = Shape.circle(3)
            let b: Shape = Shape.empty
            let c: Shape = Shape.empty(Void.new())
            let d: Shape = Shape.nothing
            fn f() -> Shape {{
                choice Inner {{
                    only
                }}
                let inner: Inner = Inner.only
                return Shape.circle(1 + 2)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn options_can_be_used_anywhere() {
        // Unlike groups, choices can be created outside their `impl` and
        // their module.
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /shapes
                    let a = Shape.circle(1)
                    fn f() -> Shape {
                        return Shape.empty
                    }
                    ",
                ),
                ("/shapes", SHAPE),
            ]),
            Ok(())
        );
    }

    #[test]
    fn choices_can_have_impls() {
        let src = format!(
            "
            {SHAPE}
            impl Shape {{
                static fn unit() -> Shape {{
                    return Shape.circle(1)
                }}

                // Methods are used on values, so they can share names with
                // options.
                fn circle() -> Int {{
                    return 1
                }}
            }}
            let x: Int = Shape.unit().circle()
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn rejects_wrong_option_values() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let a = Shape.circle(true)
                let b = Shape.circle(value: 1)
                let c = Shape.circle()
                let d = Shape.circle(1, 2)
                let e = Shape.empty(1)
                let f = Shape.empty()
                "
            )),
            [
                "argument `value` of `Shape.circle` expects `Int`, found `Bool`",
                "argument to `Shape.circle` should not have a label, found `value`",
                "`Shape.circle` takes 1 argument, found 0",
                "`Shape.circle` takes 1 argument, found 2",
                "argument `value` of `Shape.empty` expects `Void`, found `Int`",
                "`Shape.empty` takes 1 argument, found 0",
            ]
        );
    }

    #[test]
    fn options_holding_values_need_them() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let a: Shape = Shape.circle
                let b: Int = a
                "
            )),
            [
                "`Shape.circle` holds a value of type `Int`, so it must be given one, as in `Shape.circle(value)`",
                // The value still has the choice's type.
                "cannot assign `Shape` to `b`, which has type `Int`",
            ]
        );
    }

    #[test]
    fn rejects_unknown_options() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let a = Shape.square(1)
                let b = Shape.square
                "
            )),
            [
                "`Shape` has no option or static function `square`",
                "`Shape` has no option or static function `square`",
            ]
        );
    }

    #[test]
    fn options_are_used_on_types_not_values() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let shape = Shape.empty
                let a = shape.empty
                let b = shape.circle(1)
                "
            )),
            [
                "`Shape.empty` is an option, so it must be used on `Shape` itself rather than a value",
                "`Shape.circle` is an option, so it must be used on `Shape` itself rather than a value",
            ]
        );
    }

    #[test]
    fn rejects_constructing_choices_like_groups() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                impl Shape {{
                    static fn new() -> Shape {{
                        return Shape {{}}
                    }}
                }}
                "
            )),
            ["`Shape` is a choice, so it can only be created from one of its options"]
        );
    }

    #[test]
    fn option_names_are_unique() {
        assert_eq!(
            messages_top(
                "
                choice Shape {
                    circle: Int
                    circle: Bool
                }
                impl Shape {
                    static fn circle() -> Shape {
                        return Shape.circle(1)
                    }
                }
                "
            ),
            [
                "`Shape` already has an option `circle`",
                "`Shape` already has an option `circle`",
            ]
        );
    }

    #[test]
    fn rejects_recursive_choices() {
        assert_eq!(
            check_top(
                "
                choice List {
                    empty
                    node: List
                }
                group Node {
                    let next: Link
                }
                choice Link {
                    none
                    some: Node
                }
                "
            ),
            Err(vec![
                CheckError::RecursiveType("List".to_string()),
                CheckError::RecursiveType("Node".to_string()),
                CheckError::RecursiveType("Link".to_string()),
            ])
        );
    }

    #[test]
    fn unknown_option_types_do_not_cascade() {
        assert_eq!(
            check_top(
                "
                choice Shape {
                    circle: Circle
                }
                let a = Shape.circle(1)
                "
            ),
            Err(vec![CheckError::UnknownType("Circle".to_string())])
        );
    }

    #[test]
    fn choices_are_not_comparable() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let a = Shape.empty == Shape.empty
                "
            )),
            ["operator `==` expects `Int` or `Bool`, found `Shape`"]
        );
    }

    #[test]
    fn accepts_match() {
        let src = format!(
            "
            {SHAPE}
            fn area(of shape: Shape) -> Int {{
                var area = 0
                match shape {{
                    Shape.circle(let radius) => {{
                        area = radius * radius * 3
                    }}
                    // The value doesn't have to be given a name.
                    Shape.empty => {{}}
                    Shape.nothing(let nothing) => {{
                        let void: Void = nothing
                    }}
                }}
                return area
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn match_bindings_have_their_options_types() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f(shape: Shape) {{
                    match shape {{
                        Shape.circle(let value) => {{
                            let wrong: Bool = value
                        }}
                        Shape.empty(let value) => {{
                            let wrong: Int = value
                        }}
                        Shape.nothing => {{}}
                    }}
                }}
                "
            )),
            [
                "cannot assign `Int` to `wrong`, which has type `Bool`",
                "cannot assign `Void` to `wrong`, which has type `Int`",
            ]
        );
    }

    #[test]
    fn match_bindings_are_constants_in_their_own_option() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f(shape: Shape, taken: Int) {{
                    match shape {{
                        Shape.circle(let radius) => {{
                            radius = 1
                        }}
                        Shape.empty(let taken) => {{}}
                        Shape.nothing => {{
                            let x = radius
                        }}
                    }}
                    let y = radius
                }}
                "
            )),
            [
                "cannot assign to `radius` because it was declared with `let`",
                "`taken` is already declared",
                "`radius` is not declared",
                "`radius` is not declared",
            ]
        );
    }

    #[test]
    fn match_needs_a_choice() {
        assert_eq!(
            check_body("match 1 {}"),
            Err(vec![CheckError::MatchNotChoice(ty("Int"))])
        );
    }

    #[test]
    fn rejects_wrong_options_in_match() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                choice Other {{
                    circle: Int
                }}
                fn f(shape: Shape) {{
                    match shape {{
                        Other.circle => {{}}
                        Shape.square => {{}}
                        Missing.circle => {{}}
                        Shape.circle => {{}}
                        Shape.circle(let radius) => {{}}
                        Shape.empty => {{}}
                        Shape.nothing => {{}}
                    }}
                }}
                "
            )),
            [
                "`match` is on `Shape`, so it can't match an option of `Other`",
                "`Shape` has no option `square`",
                "unknown type `Missing`",
                "`Shape.circle` is matched more than once",
            ]
        );
    }

    #[test]
    fn match_needs_every_option() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f(shape: Shape) {{
                    match shape {{
                        Shape.empty => {{}}
                    }}
                    match shape {{
                        Shape.empty => {{}}
                        Shape.circle => {{}}
                    }}
                }}
                "
            )),
            [
                "`match` is missing the options `Shape.circle` and `Shape.nothing`",
                "`match` is missing the option `Shape.nothing`",
            ]
        );
    }

    #[test]
    fn match_returns_if_every_option_does() {
        let src = format!(
            "
            {SHAPE}
            fn f(shape: Shape) -> Int {{
                match shape {{
                    Shape.circle(let radius) => {{
                        return radius
                    }}
                    Shape.empty => {{
                        if true {{
                            return 1
                        }} else {{
                            return 2
                        }}
                    }}
                    Shape.nothing => {{
                        return 3
                    }}
                }}
            }}
            fn absurd(_ never: Never) -> Int {{
                match never {{}}
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f(shape: Shape) -> Int {{
                    match shape {{
                        Shape.circle(let radius) => {{
                            return radius
                        }}
                        Shape.empty => {{}}
                        Shape.nothing => {{
                            return 3
                        }}
                    }}
                }}
                "
            )),
            ["`f` must return a value of type `Int` on every path"]
        );
    }

    #[test]
    fn match_options_are_checked_when_the_value_has_an_error() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f() {{
                    match missing {{
                        Shape.circle(let radius) => {{
                            let x: Bool = 1
                        }}
                    }}
                }}
                "
            )),
            [
                "`missing` is not declared",
                "cannot assign `Int` to `x`, which has type `Bool`",
            ]
        );
    }

    #[test]
    fn never_cannot_be_created() {
        // It's from the standard library, and a `match` on it needs no
        // options, since it doesn't have any.
        let src = "
            fn absurd(_ never: Never) -> Int {
                match never {}
            }
            ";
        assert_eq!(check_top(src), Ok(()));
        assert_eq!(
            messages_top(
                "
                let a = Never.anything
                let b = Never.anything(1)
                impl Never {
                    static fn new() -> Never {
                        return Never {}
                    }
                }
                "
            ),
            [
                "`Never` has no option or static function `anything`",
                "`Never` has no option or static function `anything`",
                "`Never` is a choice, so it can only be created from one of its options",
            ]
        );
    }
}
