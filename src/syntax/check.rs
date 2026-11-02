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
//! must not clash with each other. When they do, a name is qualified by the
//! last part of its module's path, as in `other::f`, or by the alias given in
//! the module's `use`, as in `use /dir/other as renamed`. Only a `use` can
//! contain a path.
//!
//! The standard library is a package called `stdlib`, whose files are modules
//! named from the package, such as `stdlib/core`. Every program imports
//! `stdlib/core` without needing a `use`, and can import the standard
//! library's other modules like any other module. Only the standard library
//! can declare
//! primitive types, such as `Int`, which the compiler represents itself, and
//! whose values only come from literals and operators, and primitive
//! functions, such as `exit`, which the compiler implements itself.
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
        self as ast, Arg, BinOp, ChoiceOption, Expr, ExprKind, Field, FieldValue, ImplAlias,
        ImplFunction, Import, MatchArm, ModulePath, Mutability, Name, Param, Pattern, PatternKind,
        Program, Span, Stmt, StmtKind, TraitFunction, TypeName, UnOp,
    },
    parser::parse,
};
use crate::ir;

/// The package holding the standard library.
const STDLIB_PACKAGE: &str = "stdlib";

/// The source of each file of the standard library, by its path within it.
const STDLIB_FILES: [(&str, &str); 1] = [("core", include_str!("../stdlib/core.atom"))];

/// The source of the standard library's module shown as `module`, such as
/// `stdlib/core`, for showing where its errors are.
pub fn stdlib_source(module: &str) -> Option<&'static str> {
    let path = module.strip_prefix(STDLIB_PACKAGE)?.strip_prefix('/')?;
    let (_, src) = STDLIB_FILES.iter().find(|(name, _)| *name == path)?;
    Some(src)
}

/// The files of the standard library, by their paths within it.
static STDLIB: LazyLock<Vec<Module>> = LazyLock::new(|| {
    STDLIB_FILES
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

/// Every type is a group, a choice or a primitive. Types in different scopes
/// can share a name, and aliases give a type more names, so they're told
/// apart by `id`, and `name` is only for messages. It's the name the type was
/// found by, so a type found through an alias is named after the alias.
#[derive(Debug, Clone)]
pub struct Type {
    id: usize,
    name: String,
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Type {}

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
    /// Where it is in the module's source.
    pub span: Span,
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
    /// A `use` gave a module an alias that an earlier `use` gave another module.
    AliasRedeclared { alias: String, module: String },
    /// A `use` of a directory found no modules that could be imported.
    EmptyDirectory(String),
    /// A package was used where one of its modules was needed.
    PackageNotModule(String),
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
    /// they need different names, using `as`.
    AmbiguousModule { name: String, modules: Vec<String> },
    /// A top-level value's initialiser depends on the value itself.
    CyclicValue(String),
    /// An alias is an alias of itself, directly or through other aliases.
    CyclicAlias(String),
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
    /// An arithmetic, bitwise or ordering operator was used on something
    /// that isn't an integer.
    NotInteger { op: &'static str, found: Type },
    /// `-` was used on something that isn't a signed integer.
    NotSignedInteger { op: &'static str, found: Type },
    /// A shift was used on something that isn't an unsigned integer.
    NotUnsignedInteger { op: &'static str, found: Type },
    /// An integer literal doesn't fit in its type, which holds `min` to
    /// `max`.
    IntOutOfRange {
        value: i128,
        ty: Type,
        min: i128,
        max: i128,
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
    /// A function was assigned to.
    AssignToFunction(String),
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
    /// A choice was constructed like a group, rather than from an option.
    ConstructedChoice(Type),
    /// A pattern can't match the type of value it's matched against.
    PatternMismatch { pattern: String, ty: Type },
    /// A pattern names an option of a different type to the matched value's.
    WrongPatternType { expected: Type, found: Type },
    /// A pattern names an option that the choice doesn't have.
    UnknownOption { ty: Type, option: String },
    /// A pattern that's assigned a value doesn't match every value, such as
    /// `missing`.
    RefutablePattern { pattern: String, missing: String },
    /// A pattern in a `match` can't be reached, since the ones before it
    /// match everything it does.
    UnreachablePattern(String),
    /// A `match` doesn't match some values, shown as these patterns, with
    /// `more` if there are more than are shown.
    MissingPatterns { patterns: Vec<String>, more: bool },
    /// A value was called whose type doesn't implement `Call`.
    NotCallable { name: String, ty: Type },
    /// A value was called with an input of a different type to its `Input`.
    WrongCallInput {
        name: String,
        expected: Type,
        found: Type,
    },
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
    /// `Self` was used outside an `impl`.
    SelfTypeOutsideImpl,
    /// `self` was used in a static function, which isn't called on a value.
    SelfInStatic,
    /// No trait has this name.
    UnknownTrait(String),
    /// An `impl` is `with` something that isn't a trait.
    NotATrait(String),
    /// A trait's name was used as a value.
    TraitAsValue(String),
    /// A trait declares more than one alias or function with the same name.
    TraitMemberRedeclared { trait_name: String, member: String },
    /// An `impl` with a trait has an alias or function the trait doesn't
    /// declare.
    NotInTrait { trait_name: String, member: String },
    /// An `impl` without a trait has an alias, which only a trait can
    /// declare.
    ImplAliasWithoutTrait(String),
    /// A generic type was used without its arguments.
    GenericArgsNeeded(String),
    /// A type that isn't generic was given generic arguments.
    NotGeneric(String),
    /// A generic type was given the wrong number of arguments.
    WrongGenericCount {
        ty: String,
        expected: usize,
        found: usize,
    },
    /// A generic type's name was used without arguments where they can't be
    /// inferred.
    CannotInferGeneric(String),
    /// A generic type that's only for choices was given something else.
    NotAChoice { generic: String, found: Type },
    /// `Self.T` was used outside the `impl` of a generic type.
    SelfParamOutsideGeneric(String),
    /// `Self.T` names a parameter the `impl`'s generic type doesn't have.
    UnknownSelfParam { ty: Type, param: String },
    /// A generic type was given arguments nested too deeply, which means a
    /// type uses itself with ever larger arguments.
    GenericTooDeep(String),
    /// A generic type was declared inside a function.
    GenericInFunction(String),
    /// `Self` or one of a trait's aliases was used in a tuple type in the
    /// trait's signatures.
    TraitTypeInTuple(String),
    /// An alias that's inferred from an `impl`'s signatures was used in them,
    /// or in another of its aliases, without being given.
    InferredAliasUsed(String),
    /// An `impl` doesn't give one of its trait's aliases, which can't be
    /// inferred since none of the `impl`'s functions use it.
    UninferableAlias {
        ty: String,
        trait_name: String,
        alias: String,
    },
    /// An `impl` with a trait implements one of its functions with a
    /// different signature to the trait's, which is `expected`.
    TraitFnMismatch {
        trait_name: String,
        expected: String,
    },
    /// `&`, `|`, `!`, `==` or `!=` was used on a type that doesn't implement
    /// the standard library's `And`, `Or`, `Not` or `Equal` trait, which
    /// they call.
    NotImplemented {
        op: &'static str,
        trait_name: &'static str,
        found: Type,
    },
    /// An `impl` with a trait doesn't implement one of its functions, which
    /// is `expected`, as the trait writes it.
    MissingTraitMember {
        ty: String,
        trait_name: String,
        expected: String,
    },
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
            Self::AliasRedeclared { alias, module } => {
                write!(f, "`{alias}` is already an alias of `{module}`")
            }
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
                "`{name}` could be {}, so use `as` to give them different names",
                quoted_list(modules, "or")
            ),
            Self::CyclicValue(name) => write!(f, "`{name}` depends on its own value"),
            Self::CyclicAlias(name) => write!(f, "`{name}` is an alias of itself"),
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
            Self::NotInteger { op, found } => {
                write!(f, "operator `{op}` expects an integer, found `{found}`")
            }
            Self::NotSignedInteger { op, found } => {
                write!(
                    f,
                    "operator `{op}` expects a signed integer, found `{found}`"
                )
            }
            Self::NotUnsignedInteger { op, found } => {
                write!(
                    f,
                    "operator `{op}` expects an unsigned integer, found `{found}`"
                )
            }
            Self::IntOutOfRange {
                value,
                ty,
                min,
                max,
            } => write!(
                f,
                "`{value}` doesn't fit in `{ty}`, which holds `{min}` to `{max}`"
            ),
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
            Self::AssignToFunction(name) => {
                write!(f, "cannot assign to `{name}`, since it's a function")
            }
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
            Self::PatternMismatch { pattern, ty } => {
                write!(f, "pattern `{pattern}` can't match `{ty}`")
            }
            Self::WrongPatternType { expected, found } => write!(
                f,
                "`match` is on `{expected}`, so it can't match an option of `{found}`"
            ),
            Self::UnknownOption { ty, option } => {
                write!(f, "`{ty}` has no option `{option}`")
            }
            Self::RefutablePattern { pattern, missing } => write!(
                f,
                "pattern `{pattern}` doesn't match every value, such as `{missing}`, so it can only be used in a `match`"
            ),
            Self::UnreachablePattern(pattern) => write!(
                f,
                "pattern `{pattern}` can't be reached, since the patterns before it match everything it does"
            ),
            Self::MissingPatterns { patterns, more } => {
                let s = if patterns.len() == 1 && !more {
                    ""
                } else {
                    "s"
                };
                let patterns = if *more {
                    let quoted: Vec<_> = patterns
                        .iter()
                        .map(|pattern| format!("`{pattern}`"))
                        .collect();
                    format!("{} and others", quoted.join(", "))
                } else {
                    quoted_list(patterns, "and")
                };
                write!(f, "`match` is missing the pattern{s} {patterns}")
            }
            Self::ConstructedChoice(ty) => write!(
                f,
                "`{ty}` is a choice, so it can only be created from one of its options"
            ),
            Self::NoMember { ty, member } => {
                write!(f, "`{ty}` has no field or method `{member}`")
            }
            Self::NotCallable { name, ty } => write!(
                f,
                "`{name}` is `{ty}`, which doesn't implement `Call`, so it can't be called"
            ),
            Self::WrongCallInput {
                name,
                expected,
                found,
            } => write!(
                f,
                "`{name}` is called with `{expected}`, but was given `{found}`"
            ),
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
            Self::SelfTypeOutsideImpl => {
                write!(f, "`Self` can only be used inside an `impl` or a `trait`")
            }
            Self::SelfInStatic => write!(f, "`self` can't be used in a static function"),
            Self::UnknownTrait(name) => write!(f, "unknown trait `{name}`"),
            Self::NotATrait(name) => write!(f, "`{name}` is not a trait"),
            Self::TraitAsValue(name) => write!(f, "`{name}` is a trait, not a value"),
            Self::TraitMemberRedeclared { trait_name, member } => {
                write!(f, "trait `{trait_name}` already declares `{member}`")
            }
            Self::NotInTrait { trait_name, member } => {
                write!(f, "`{member}` is not declared by trait `{trait_name}`")
            }
            Self::ImplAliasWithoutTrait(name) => write!(
                f,
                "`alias {name}` can only be given in an `impl` with a trait that declares it"
            ),
            Self::GenericArgsNeeded(name) => write!(
                f,
                "`{name}` is generic, so it needs arguments, as in `{name}<...>`"
            ),
            Self::NotGeneric(name) => {
                write!(f, "`{name}` isn't generic, so it can't be given arguments")
            }
            Self::WrongGenericCount {
                ty,
                expected,
                found,
            } => {
                let s = if *expected == 1 { "" } else { "s" };
                write!(
                    f,
                    "`{ty}` has {expected} generic parameter{s}, but was given {found}"
                )
            }
            Self::CannotInferGeneric(name) => write!(
                f,
                "the arguments of `{name}` can't be inferred here, so they must be given, as in `{name}<...>`"
            ),
            Self::NotAChoice { generic, found } => {
                write!(
                    f,
                    "`{generic}` can only be used with a choice, found `{found}`"
                )
            }
            Self::SelfParamOutsideGeneric(param) => write!(
                f,
                "`Self.{param}` can only be used in the `impl` of a generic type"
            ),
            Self::UnknownSelfParam { ty, param } => {
                write!(f, "`{ty}` has no generic parameter `{param}`")
            }
            Self::GenericTooDeep(ty) => write!(
                f,
                "`{ty}` nests generic types too deeply, which usually means a generic type uses itself with different arguments"
            ),
            Self::GenericInFunction(name) => write!(
                f,
                "`{name}` can't be generic, since only types at the top level can be"
            ),
            Self::TraitTypeInTuple(name) => write!(
                f,
                "`{name}` can't be used inside another type in a trait's signatures, since it's a different type in each `impl`"
            ),
            Self::InferredAliasUsed(name) => write!(
                f,
                "`{name}` is inferred from this `impl`'s signatures, so they can't use it unless it's given with `alias {name} = ...`"
            ),
            Self::UninferableAlias {
                ty,
                trait_name,
                alias,
            } => write!(
                f,
                "`impl {ty} with {trait_name}` must give `alias {alias}`, since it can't be inferred from the `impl`'s functions"
            ),
            Self::TraitFnMismatch {
                trait_name,
                expected,
            } => write!(
                f,
                "trait `{trait_name}` declares this function as `{expected}`"
            ),
            Self::MissingTraitMember {
                ty,
                trait_name,
                expected,
            } => write!(f, "`impl {ty} with {trait_name}` is missing `{expected}`"),
            Self::NotImplemented {
                op,
                trait_name,
                found,
            } => write!(
                f,
                "operator `{op}` expects a type that implements `{trait_name}`, found `{found}`"
            ),
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
    // Each generic type is checked once on its own, so the types made from it
    // shouldn't have errors of their own. If one does anyway, it's reported,
    // rather than being lost.
    if checker.errors.is_empty()
        && let Some(error) = checker.quietened.first()
    {
        checker.errors.push(error.clone());
    }

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
    span: Span,
    /// `None` if the parameter's type is unknown.
    ty: Option<Type>,
}

/// An element of a tuple type as `Checker::tuple_ids` tells them apart: its
/// label, name and type `id`.
type TupleKey = (Option<String>, Option<String>, usize);

/// An element of a tuple type, which is built with its label, and read by
/// its name.
#[derive(Debug, Clone)]
struct TupleElementType {
    label: Option<String>,
    name: Option<String>,
    ty: Type,
}

impl fmt::Display for TupleElementType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let element = ast::TupleElement {
            label: self.label.clone(),
            name: self.name.clone(),
            ty: TypeName::new(&self.ty.name),
        };
        write!(f, "{element}")
    }
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
    /// An index into `Checker::aliases`. `Checker::lookup` finds aliases as
    /// the types they're aliases of.
    Alias(usize),
    /// An index into `Checker::traits`.
    Trait(usize),
    /// A generic group, choice or primitive, as an index into
    /// `Checker::templates`, which is a different type for each set of
    /// arguments it's given.
    Generic(usize),
}

/// A generic group, choice or primitive, which types are made from by giving
/// its parameters arguments, as in `Optional<Int>`.
struct Template<'a> {
    name: &'a str,
    module: usize,
    params: &'a [String],
    /// The group, choice or primitive declaring it.
    stmt: &'a Stmt,
    /// Its `impl`s, with the modules they're in, whose functions are
    /// declared for each type made from it.
    impls: Vec<(usize, &'a Stmt)>,
}

/// A trait, whose functions an `impl` with it must implement exactly.
struct TraitInfo<'a> {
    name: &'a str,
    span: Span,
    module: usize,
    /// The names of its aliases, such as `Rhs`, whose types each
    /// implementation gives.
    aliases: &'a [String],
    functions: &'a [TraitFunction],
    /// For each of `functions`, its parameter types and return type, which
    /// are `None` if they're unknown. Worked out once types are defined.
    signatures: Vec<(Vec<Option<TraitType>>, Option<TraitType>)>,
}

/// Which values a pattern matches, for checking that a `match`'s patterns
/// match every value, and that each matches something new.
#[derive(Debug, Clone)]
enum Space {
    Any,
    /// The values made with `Ctor`, whose parts match each of these in turn.
    Ctor(Ctor, Vec<Space>),
}

/// A way of making a value that patterns can tell apart from the others.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ctor {
    Int(i128),
    Bool(bool),
    /// A tuple, whose parts are its elements.
    Tuple,
    /// The option of a choice at this index, whose part is its value.
    Option(usize),
}

/// `value`, the checked value for the option at `option` of the choice with
/// id `choice`, as the choice.
fn chosen(choice: usize, option: usize, value: Typed) -> Typed {
    Typed {
        ty: value.ty,
        expr: value.expr.map(|value| ir::Expr::Choose {
            choice,
            option,
            value: Box::new(value),
        }),
    }
}

/// The names of the values that `pattern` binds, in order.
fn bindings(pattern: &Pattern) -> Vec<(&str, Span)> {
    match &pattern.kind {
        PatternKind::Binding(name) => vec![(name, pattern.span)],
        PatternKind::Tuple(elements) => elements
            .iter()
            .flat_map(|element| bindings(&element.pattern))
            .collect(),
        PatternKind::Option {
            value: Some(value), ..
        } => bindings(value),
        PatternKind::Any
        | PatternKind::Int { .. }
        | PatternKind::Bool(_)
        | PatternKind::Option { .. } => vec![],
    }
}

/// The parameters of a static function or option of a generic type, as they're
/// written, for working out the type's arguments from a call's.
struct DeclaredParams<'a> {
    /// The index in `Checker::modules` of the module they're written in.
    module: usize,
    types: Vec<&'a TypeName>,
    /// Whether they're in an `impl`, where a generic parameter is written
    /// `Self.T`, rather than a choice's options, where it's written `T`.
    in_impl: bool,
}

/// An `impl ty with trait_name`, whose functions are checked against the
/// trait at `index` in `Checker::traits`.
struct Implementation<'b, 'a> {
    ty: &'b Name,
    /// The type `ty` refers to.
    self_ty: &'b Type,
    trait_name: &'b Name,
    index: usize,
    aliases: &'a [ImplAlias],
    /// The index in `Checker::impl_aliases` of the types of `aliases`, if
    /// there are any.
    alias_types: Option<usize>,
}

/// The type of an alias in an `impl`.
#[derive(Debug, Clone)]
enum ImplAliasType {
    /// Given, or already inferred. `None` if it's unknown.
    Known(Option<Type>),
    /// Not given, so it's being inferred from the `impl`'s signatures, which
    /// can't use it.
    Inferring,
}

/// A type in a trait's function signature.
#[derive(Debug, Clone)]
enum TraitType {
    /// `Self`, which is whichever type implements the trait.
    SelfType,
    /// One of the trait's aliases, as an index into `TraitInfo::aliases`,
    /// whose type each implementation gives.
    Alias(usize),
    Type(Type),
}

/// A value declared at the top level of a module. Its type is worked out when
/// it's first needed, since top-level values can be used in any order.
struct Global<'a> {
    module: usize,
    name: &'a str,
    span: Span,
    ty: Option<&'a TypeName>,
    value: &'a Expr,
    state: LazyType,
    /// What its initialiser compiles to, once it's been checked, or `None` if
    /// it has an error.
    lowered: Option<ir::Expr>,
}

/// The state of something whose type is worked out when it's first needed.
enum LazyType {
    Unchecked,
    /// Its type is being worked out, so needing it again means it depends on
    /// itself.
    Checking,
    Checked(Option<Type>),
}

/// A type alias, as in `alias ID = Int`. Top-level aliases are worked out
/// when they're first needed, since they can be used in any order.
struct Alias<'a> {
    module: usize,
    name: &'a str,
    span: Span,
    /// The type it's an alias of.
    target: &'a TypeName,
    state: LazyType,
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

/// What's called by `Checker::call_value`.
enum Callee {
    /// A value, which compiles to this if it has no errors.
    Value(Option<ir::Expr>),
    /// A function by its name, as an index into `Checker::functions`.
    Function(usize),
}

struct ModuleInfo<'a> {
    /// The module's path, such as `/dir/other` or `stdlib/core`.
    display: String,
    /// The package the module is in, or `None` if it's in the project.
    package: Option<&'a str>,
    /// The module's path from the root of its package or the project.
    path: &'a [String],
    /// The name used to qualify the module's names, such as `other` in
    /// `other::f`, unless a `use` gives it an alias.
    name: &'a str,
    program: &'a Program,
    /// The names declared at the module's top level.
    scope: HashMap<&'a str, Binding>,
    /// The other modules whose top-level names this one can use.
    imports: Vec<usize>,
    /// The names that can qualify a name in this module, with the modules they
    /// refer to, which are this one and its imports.
    qualifiers: Vec<(&'a str, usize)>,
}

/// Why `Checker::lookup` couldn't find a name.
enum LookupError {
    Undeclared,
    Captured,
    /// The name was found in each of these modules.
    Ambiguous(Vec<String>),
    /// The name's module couldn't be found.
    Module(Box<CheckError>),
    /// The name is an alias whose type couldn't be worked out, or `Self` in
    /// an `impl` whose type is unknown, which has already been reported.
    Reported,
    /// The name is `Self`, outside an `impl`.
    SelfTypeOutsideImpl,
    /// The name is an `impl`'s alias that's still being inferred.
    InferredAlias,
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
        /// Its signature.
        span: Span,
        body: &'a [Stmt],
        kind: FnKind,
        /// The `impl` aliases its body can use, as an index into
        /// `Checker::impl_aliases`, if it's in an `impl` that gives any.
        aliases: Option<usize>,
        /// Whether its errors aren't reported, as for the functions of the
        /// types made from a generic type, whose `impl` is checked once on
        /// its own.
        quiet: bool,
        /// The module of the trait, if it's a default implementation.
        written_in: Option<usize>,
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
    /// For each function used as a value, by its index in `functions`, the
    /// index of the function that a pointer to it really points to, which
    /// takes its `Input` as a single parameter.
    function_values: HashMap<usize, usize>,
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
    /// The aliases of each `impl` with a trait, by name, whether they're
    /// given, such as `alias Rhs = Self`, or inferred.
    impl_aliases: Vec<HashMap<&'a str, ImplAliasType>>,
    /// The index in `impl_aliases` of the aliases that the code being
    /// checked can use, if it's in an `impl` that gives any, including inside
    /// functions nested in its methods.
    current_impl_aliases: Option<usize>,
    functions: Vec<Signature<'a>>,
    traits: Vec<TraitInfo<'a>>,
    templates: Vec<Template<'a>>,
    /// For each type made from a generic type, by `id`, the index in
    /// `templates` of what it's made from, and its arguments.
    instances: HashMap<usize, (usize, Vec<Type>)>,
    /// The `id` of each type made from a generic type, by the index in
    /// `templates` of what it's made from and its arguments' `id`s.
    instance_ids: HashMap<(usize, Vec<usize>), usize>,
    /// The types made from generic types with stand-ins for their parameters,
    /// which check them once, so their errors are reported.
    checked_instances: HashSet<usize>,
    /// While a type made from a generic type is defined, the index in
    /// `templates` of what it's made from, and its arguments, which its
    /// parameters stand for.
    type_params: Option<(usize, Vec<Type>)>,
    /// Whether every `impl` has been found, so the functions of a type made
    /// from a generic type can be declared once it's made.
    methods_ready: bool,
    /// The types made from generic types before `methods_ready`, whose
    /// functions are declared once it is.
    undeclared_instances: Vec<usize>,
    /// The values and function bodies still to check, with their modules.
    pending: Vec<(usize, Pending<'a>)>,
    /// The errors that weren't reported, from checking the types made from
    /// generic types, which are checked once on their own.
    quietened: Vec<ModuleError>,
    /// Which types implement which traits, as a type `id` and an index into
    /// `traits`.
    implementations: HashSet<(usize, usize)>,
    /// For each of `functions`, what it compiles to, once its body has been
    /// checked, or `None` if it has an error.
    lowered_functions: Vec<Option<ir::Function>>,
    /// The statements that evaluate a `Never`, which never finish, so count
    /// as returning.
    never_returns: HashSet<*const Stmt>,
    /// Whether a `Never` has been evaluated since `evaluating` started
    /// checking a statement's expressions.
    evaluated_never: bool,
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
    aliases: Vec<Alias<'a>>,
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
    /// For each tuple type, by `id`, its elements. Other types aren't in it.
    tuples: HashMap<usize, Vec<TupleElementType>>,
    /// The `id` of each tuple type, by its elements' labels, names and type
    /// `id`s. Tuples are the same type only if all of those are the same.
    tuple_ids: HashMap<Vec<TupleKey>, usize>,
    errors: Vec<ModuleError>,
    /// The module holding the code being checked, if that's not `module`, as
    /// a trait's default implementation is checked as if it's in an `impl`.
    /// Errors are reported where the code is written.
    written_in: Option<usize>,
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
            name: module.path.last().expect("module paths aren't empty"),
            program: &module.program,
            scope: HashMap::new(),
            imports: vec![],
            qualifiers: vec![],
        });
        self.modules.len() - 1
    }

    /// Adds the standard library's modules.
    fn add_stdlib(&mut self) {
        for module in STDLIB.iter() {
            let index = self.add_module(Some(STDLIB_PACKAGE), module);
            if module.path == ["core"] {
                self.core = index;
            }
        }
    }

    fn error(&mut self, span: Span, error: CheckError) {
        self.error_in(self.written_in.unwrap_or(self.module), span, error);
    }

    fn error_in(&mut self, module: usize, span: Span, error: CheckError) {
        let module = self.modules[module].display.clone();
        self.errors.push(ModuleError {
            module,
            span,
            error,
        });
    }

    /// Checks every module. Each kind of top-level declaration is declared in
    /// every module before the next kind, and before anything is checked, so
    /// top-level names can be used in any order.
    fn check_modules(&mut self) {
        for module in 0..self.modules.len() {
            self.module = module;
            self.resolve_imports();
        }

        // Every type and alias is declared before any are resolved, so types
        // can contain each other, and aliases can refer to each other, in any
        // order.
        let mut types = vec![];
        for module in 0..self.modules.len() {
            self.module = module;
            for stmt in self.modules[module].program {
                match &stmt.kind {
                    StmtKind::Group { name, params, .. }
                    | StmtKind::Choice { name, params, .. }
                    | StmtKind::Primitive { name, params }
                        if !params.is_empty() =>
                    {
                        self.declare_template(name, params, stmt);
                    }
                    StmtKind::Group { name, .. }
                    | StmtKind::Choice { name, .. }
                    | StmtKind::Primitive { name, .. } => {
                        let id = self.declare_type(name, stmt.span);
                        // It can be used as a choice before its options are
                        // defined.
                        if matches!(stmt.kind, StmtKind::Choice { .. }) {
                            self.choice_options.insert(id, vec![]);
                        }
                        types.push((module, id, stmt));
                    }
                    StmtKind::Alias { name, ty } => {
                        self.declare_alias(name, stmt.span, ty, LazyType::Unchecked);
                    }
                    StmtKind::Trait {
                        name,
                        aliases,
                        functions,
                    } => self.declare_trait(name, stmt.span, aliases, functions),
                    _ => {}
                }
            }
        }
        for index in 0..self.aliases.len() {
            self.alias_type(index);
        }
        for &(module, id, stmt) in &types {
            self.module = module;
            self.define_type(id, stmt);
        }
        // Each generic type is checked once, with stand-ins for its
        // parameters, so its errors are reported once.
        let mut checked = vec![];
        for template in 0..self.templates.len() {
            self.module = self.templates[template].module;
            checked.extend(self.check_template(template));
        }
        for index in 0..self.traits.len() {
            self.define_trait(index);
        }
        for &(module, id, stmt) in &types {
            self.module = module;
            if let StmtKind::Group { name, .. } | StmtKind::Choice { name, .. } = &stmt.kind {
                self.check_not_recursive(id, name, stmt.span);
            }
        }
        for (module, ty, span) in checked {
            self.module = module;
            self.check_not_recursive(ty.id, &ty.name, span);
        }

        // Functions, methods and values come after groups, since their types
        // can be groups.
        for module in 0..self.modules.len() {
            self.module = module;
            for stmt in self.modules[module].program {
                match &stmt.kind {
                    StmtKind::Fn(function) => {
                        let index = self.declare_fn(
                            &function.name,
                            function.span,
                            &function.params,
                            &function.ret,
                        );
                        self.pending.push((
                            module,
                            Pending::Fn {
                                index,
                                name: &function.name,
                                span: function.span,
                                body: &function.body,
                                kind: FnKind::Function,
                                aliases: None,
                                quiet: false,
                                written_in: None,
                            },
                        ));
                    }
                    StmtKind::PrimitiveFn { name, params, ret } => {
                        let index = self.declare_fn(name, stmt.span, params, ret);
                        self.define_primitive_fn(index, name, stmt.span, FnKind::Function);
                    }
                    StmtKind::Impl {
                        ty,
                        with,
                        aliases,
                        functions,
                    } => {
                        // A generic type's `impl` is for every type made from
                        // it, so its functions are declared for each of them.
                        if let Ok(Binding::Generic(template)) = self.lookup(ty) {
                            self.templates[template].impls.push((module, stmt));
                            continue;
                        }
                        let self_ty = self.resolve_named_type(ty);
                        let declared = self.declare_impl(
                            ty,
                            self_ty,
                            with.as_ref(),
                            aliases,
                            functions,
                            false,
                        );
                        self.pending
                            .extend(declared.into_iter().map(|pending| (module, pending)));
                    }
                    StmtKind::Decl {
                        name, ty, value, ..
                    } => {
                        self.globals.push(Global {
                            module,
                            name,
                            span: stmt.span,
                            ty: ty.as_ref(),
                            value,
                            state: LazyType::Unchecked,
                            lowered: None,
                        });
                        let index = self.globals.len() - 1;
                        self.declare(name, stmt.span, Binding::Global(index));
                        self.pending.push((module, Pending::Global(index)));
                    }
                    StmtKind::Use(_)
                    | StmtKind::Group { .. }
                    | StmtKind::Choice { .. }
                    | StmtKind::Primitive { .. }
                    | StmtKind::Alias { .. }
                    | StmtKind::Trait { .. } => {}
                    _ => unreachable!("the parser only allows declarations at the top level"),
                }
            }
        }

        // The types made from generic types so far can now have their
        // functions declared, and later ones have them declared straight away.
        self.methods_ready = true;
        for id in std::mem::take(&mut self.undeclared_instances) {
            self.declare_instance_methods(id);
        }

        // Checking a body can make more types from generic types, whose
        // functions' bodies are then checked too.
        loop {
            let pending = std::mem::take(&mut self.pending);
            if pending.is_empty() {
                break;
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
                        span,
                        body,
                        kind,
                        aliases,
                        quiet,
                        written_in,
                    } => {
                        let errors = self.errors.len();
                        self.written_in = written_in;
                        self.fn_body(index, name, span, body, kind, aliases);
                        self.written_in = None;
                        if quiet {
                            self.quieten(errors);
                        }
                    }
                }
            }
        }
    }

    /// Declares the functions of `impl ty`, whose type is `self_ty`, `with`
    /// a trait if given, returning their bodies to check. Its errors are
    /// reported unless it's `quiet`, which it is for the types made from a
    /// generic type, whose `impl` is checked once on its own.
    fn declare_impl(
        &mut self,
        ty: &'a Name,
        self_ty: Option<Type>,
        with: Option<&'a Name>,
        aliases: &'a [ImplAlias],
        functions: &'a [ImplFunction],
        quiet: bool,
    ) -> Vec<Pending<'a>> {
        let errors = self.errors.len();
        // A type made from a generic type is named with its arguments, so
        // each one's functions have their own names.
        let type_name = match &self_ty {
            Some(self_ty) if self.instances.contains_key(&self_ty.id) => self_ty.name.clone(),
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
        // Signatures can use `Self` and the `impl`'s aliases,
        // just like bodies.
        let outer_impl_type = self.impl_type.replace(self_ty.clone());
        let impl_aliases = self.define_impl_aliases(aliases, trait_index);
        let mut indexes = vec![];
        for ImplFunction {
            is_static,
            is_primitive,
            function,
        } in functions
        {
            let name = format!("{type_name}.{}", function.name);
            let full_name = self.full_name(&name);
            let index = self.declare_method(
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
                self.define_primitive_fn(index, &name, function.span, kind);
                continue;
            }
            pending.push(Pending::Fn {
                index,
                name: &function.name,
                span: function.span,
                body: &function.body,
                kind,
                aliases: impl_aliases,
                quiet,
                written_in: None,
            });
        }
        self.impl_type = outer_impl_type;
        self.current_impl_aliases = None;
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
                quiet,
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
        if quiet {
            self.quieten(errors);
        }
        pending
    }

    /// Finds the modules imported by the current module. Modules in the
    /// project always import `stdlib/core`.
    fn resolve_imports(&mut self) {
        let mut imports = vec![];
        let mut qualifiers = vec![(self.modules[self.module].name, self.module)];
        if self.modules[self.module].package.is_none() {
            imports.push(self.core);
            qualifiers.push((self.modules[self.core].name, self.core));
        }
        let mut aliases = HashMap::new();

        for stmt in self.modules[self.module].program {
            let StmtKind::Use(import) = &stmt.kind else {
                continue;
            };
            let (path, alias) = match import {
                Import::Module { path, alias } => (path, alias),
                Import::Directory(dir) => {
                    let found = self.resolve_directory(dir);
                    if found.is_empty() {
                        self.error(stmt.span, CheckError::EmptyDirectory(import.to_string()));
                    }
                    for module in found {
                        if !imports.contains(&module) {
                            imports.push(module);
                        }
                        let qualifier = (self.modules[module].name, module);
                        if !qualifiers.contains(&qualifier) {
                            qualifiers.push(qualifier);
                        }
                    }
                    continue;
                }
            };
            let Some(module) = self.resolve_path(path) else {
                self.error(stmt.span, self.unknown_module(path));
                continue;
            };
            // Importing a module twice, or importing itself, is harmless.
            if module != self.module && !imports.contains(&module) {
                imports.push(module);
            }
            if let Some(alias) = alias
                && let first = *aliases.entry(alias.as_str()).or_insert(module)
                && first != module
            {
                let module = self.modules[first].display.clone();
                let alias = alias.clone();
                self.error(stmt.span, CheckError::AliasRedeclared { alias, module });
                continue;
            }
            let name = alias.as_deref().unwrap_or(self.modules[module].name);
            if !qualifiers.contains(&(name, module)) {
                qualifiers.push((name, module));
            }
        }

        self.modules[self.module].imports = imports;
        self.modules[self.module].qualifiers = qualifiers;
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
    /// subdirectories, other than the current module.
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

    /// Finds the module that the qualifier `name` refers to. It must be the
    /// current module or one it imports.
    fn resolve_qualifier(&self, name: &str) -> Result<usize, CheckError> {
        let matches: Vec<_> = self.modules[self.module]
            .qualifiers
            .iter()
            .filter(|(qualifier, _)| *qualifier == name)
            .map(|&(_, module)| module)
            .collect();
        match matches[..] {
            // Modules aren't identifiers, so a name can be both. If it's only
            // an identifier, that's probably the mistake.
            [] if self.is_declared(name) => Err(CheckError::NotAModule(name.to_string())),
            [] if name == STDLIB_PACKAGE => Err(CheckError::PackageNotModule(name.to_string())),
            [] => Err(CheckError::NotImported(name.to_string())),
            [module] => Ok(module),
            _ => Err(CheckError::AmbiguousModule {
                name: name.to_string(),
                modules: matches
                    .iter()
                    .map(|&module| self.modules[module].display.clone())
                    .collect(),
            }),
        }
    }

    /// Whether `name` is declared anywhere the current code could see it,
    /// even if it's a value that a function can't use.
    fn is_declared(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains_key(name))
            || self
                .visible_modules()
                .any(|module| self.modules[module].scope.contains_key(name))
    }

    /// A generic type from `stdlib/core`, such as `Fn`, as an index into
    /// `templates`.
    fn std_template(&self, name: &str) -> usize {
        match self.modules[self.core].scope.get(name) {
            Some(Binding::Generic(template)) => *template,
            _ => panic!("the standard library should declare `{name}`"),
        }
    }

    /// A type from `stdlib/core`, such as `Int` for integer literals. Found in
    /// the standard library itself, so it can't be shadowed.
    fn std_type(&self, name: &str) -> Type {
        let ty = match self.modules[self.core].scope.get(name) {
            Some(Binding::Type(ty)) => Some(ty),
            // The standard library's aliases are worked out before any code
            // that needs them is checked.
            Some(Binding::Alias(index)) => match &self.aliases[*index].state {
                LazyType::Checked(ty) => ty.as_ref(),
                _ => None,
            },
            _ => None,
        };
        let ty = ty.unwrap_or_else(|| panic!("the standard library should declare `{name}`"));
        Type {
            id: ty.id,
            name: name.to_string(),
        }
    }

    /// Finds what `name` refers to. An alias is found as the type it's an
    /// alias of, which is worked out if it hasn't been yet.
    fn lookup(&mut self, name: &Name) -> Result<Binding, LookupError> {
        // `Self` is a keyword, so it's never declared, and always refers to
        // the type of the `impl` it's in.
        if name.is_self() {
            return match &self.impl_type {
                Some(Some(ty)) => Ok(Binding::Type(ty.clone())),
                Some(None) => Err(LookupError::Reported),
                None => Err(LookupError::SelfTypeOutsideImpl),
            };
        }
        match self.lookup_binding(name)? {
            Binding::Alias(index) => {
                let name = self.aliases[index].name.to_string();
                let ty = self.alias_type(index).ok_or(LookupError::Reported)?;
                Ok(Binding::Type(Type { name, ..ty }))
            }
            binding => Ok(binding),
        }
    }

    /// Finds what `name` refers to, without working out aliases.
    fn lookup_binding(&self, name: &Name) -> Result<Binding, LookupError> {
        let binding = match &name.module {
            Some(path) => {
                let module = self
                    .resolve_qualifier(path)
                    .map_err(|error| LookupError::Module(Box::new(error)))?;
                self.modules[module]
                    .scope
                    .get(name.name.as_str())
                    .ok_or(LookupError::Undeclared)?
            }
            None => {
                // While a type made from a generic type is defined, its
                // parameters stand for its arguments.
                if let Some((template, args)) = &self.type_params
                    && let Some(index) = self.templates[*template]
                        .params
                        .iter()
                        .position(|param| *param == name.name)
                {
                    return Ok(Binding::Type(args[index].clone()));
                }
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

                // Then the aliases of the `impl` it's in.
                if let Some(aliases) = self.current_impl_aliases
                    && let Some(ty) = self.impl_aliases[aliases].get(name.name.as_str())
                {
                    return match ty {
                        ImplAliasType::Known(ty) => {
                            ty.clone().map(Binding::Type).ok_or(LookupError::Reported)
                        }
                        ImplAliasType::Inferring => Err(LookupError::InferredAlias),
                    };
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
            LookupError::Module(error) => *error,
            LookupError::Reported => return,
            LookupError::SelfTypeOutsideImpl => CheckError::SelfTypeOutsideImpl,
            LookupError::InferredAlias => CheckError::InferredAliasUsed(name_string),
        };
        self.error(name.span, error);
    }

    fn declare(&mut self, name: &'a str, span: Span, binding: Binding) {
        // Names can't be redeclared within a function, even to shadow an
        // outer block. A function's own scope can reuse names from outside it.
        // At the top level, names only need to be unique within their module.
        let scope = match self.scopes.last_mut() {
            Some(_) => {
                if self.scopes[self.fn_scope_start..]
                    .iter()
                    .any(|scope| scope.contains_key(name))
                {
                    self.error(span, CheckError::Redeclared(name.to_string()));
                }
                self.scopes.last_mut().expect("checked above")
            }
            None => {
                if self.modules[self.module].scope.contains_key(name) {
                    self.error(span, CheckError::Redeclared(name.to_string()));
                }
                &mut self.modules[self.module].scope
            }
        };
        scope.insert(name, binding);
    }

    /// Finds the type `ty`, without reporting an error if there isn't one.
    fn find_type(&mut self, ty: &TypeName) -> Option<Type> {
        match ty {
            TypeName::Named(name) => self.find_named_type(name),
            TypeName::Generic { name, args } => self.generic_type(name, args, false),
            TypeName::SelfParam(param, span) => self.self_param(param, *span, false),
            TypeName::Tuple(elements) => {
                let elements = elements
                    .iter()
                    .map(|element| {
                        Some(TupleElementType {
                            label: element.label.clone(),
                            name: element.name.clone(),
                            ty: self.find_type(&element.ty)?,
                        })
                    })
                    .collect::<Option<_>>()?;
                Some(self.tuple_type(elements))
            }
        }
    }

    /// Finds the type called `name`, without reporting an error if there
    /// isn't one.
    fn find_named_type(&mut self, name: &Name) -> Option<Type> {
        match self.lookup(name) {
            Ok(Binding::Type(ty)) => Some(ty),
            _ => None,
        }
    }

    /// Finds the type `ty`, reporting an error if it can't be found.
    fn resolve_type(&mut self, ty: &TypeName) -> Option<Type> {
        match ty {
            TypeName::Named(name) => self.resolve_named_type(name),
            TypeName::Generic { name, args } => self.generic_type(name, args, true),
            TypeName::SelfParam(param, span) => self.self_param(param, *span, true),
            TypeName::Tuple(elements) => {
                // Every element is resolved, so each one's errors are found.
                let elements: Vec<_> = elements
                    .iter()
                    .map(|element| {
                        Some(TupleElementType {
                            label: element.label.clone(),
                            name: element.name.clone(),
                            ty: self.resolve_type(&element.ty)?,
                        })
                    })
                    .collect();
                let elements = elements.into_iter().collect::<Option<_>>()?;
                Some(self.tuple_type(elements))
            }
        }
    }

    /// The tuple type with `elements`, which is created the first time it's
    /// needed.
    fn tuple_type(&mut self, elements: Vec<TupleElementType>) -> Type {
        let shown: Vec<_> = elements.iter().map(ToString::to_string).collect();
        let name = format!("({})", shown.join(", "));
        let key: Vec<_> = elements
            .iter()
            .map(|element| (element.label.clone(), element.name.clone(), element.ty.id))
            .collect();
        let id = match self.tuple_ids.get(&key) {
            Some(&id) => id,
            None => {
                let id = self.group_fields.len();
                self.group_fields.push(vec![]);
                self.group_modules.push(self.module);
                self.type_names.push(name.clone());
                self.tuple_ids.insert(key, id);
                self.tuples.insert(id, elements);
                id
            }
        };
        Type { id, name }
    }

    /// The type made from the generic type called `name` with `args`,
    /// reporting errors if `report` is set.
    fn generic_type(&mut self, name: &Name, args: &[TypeName], report: bool) -> Option<Type> {
        let template = match self.lookup(name) {
            Ok(Binding::Generic(template)) => template,
            Ok(Binding::Type(_)) if report => {
                self.error(name.span, CheckError::NotGeneric(name.to_string()));
                return None;
            }
            _ if !report => return None,
            _ => {
                self.resolve_named_type(name);
                return None;
            }
        };
        let expected = self.templates[template].params.len();
        if args.len() != expected {
            if report {
                self.error(
                    name.span,
                    CheckError::WrongGenericCount {
                        ty: name.to_string(),
                        expected,
                        found: args.len(),
                    },
                );
            }
            return None;
        }
        // Every argument is resolved, so each one's errors are found.
        let args: Vec<_> = args
            .iter()
            .map(|arg| {
                if report {
                    self.resolve_type(arg)
                } else {
                    self.find_type(arg)
                }
            })
            .collect();
        let args = args.into_iter().collect::<Option<Vec<_>>>()?;
        self.named_instance(template, args, name, report)
    }

    /// The type made from the generic type called `name`, at `template` in
    /// `templates`, with `args`, reporting errors if `report` is set.
    fn named_instance(
        &mut self,
        template: usize,
        args: Vec<Type>,
        name: &Name,
        report: bool,
    ) -> Option<Type> {
        if self.is_only_for_choices(template) && !self.choice_options.contains_key(&args[0].id) {
            if report {
                let error = CheckError::NotAChoice {
                    generic: name.to_string(),
                    found: args[0].clone(),
                };
                self.error(name.span, error);
            }
            return None;
        }
        self.instance(template, args, name.span, true)
    }

    /// Whether `ty` is made from the generic type at `template` in
    /// `templates`.
    fn is_made_from(&self, ty: &Type, template: usize) -> bool {
        self.instances
            .get(&ty.id)
            .is_some_and(|(found, _)| *found == template)
    }

    /// Whether the generic type at `template` in `templates` has values only
    /// when its first argument is a choice.
    fn is_only_for_choices(&self, template: usize) -> bool {
        template == self.std_template("ChoiceTag") || template == self.std_template("ChoiceOption")
    }

    /// What `Self.param` stands for in the `impl` of a type made from a
    /// generic type, reporting errors if `report` is set.
    fn self_param(&mut self, param: &str, span: Span, report: bool) -> Option<Type> {
        let impl_type = match &self.impl_type {
            Some(Some(ty)) => ty.clone(),
            // The `impl`'s type is unknown, which has been reported.
            Some(None) => return None,
            None => {
                if report {
                    self.error(span, CheckError::SelfParamOutsideGeneric(param.to_string()));
                }
                return None;
            }
        };
        let Some((template, args)) = self.instances.get(&impl_type.id) else {
            if report {
                self.error(span, CheckError::SelfParamOutsideGeneric(param.to_string()));
            }
            return None;
        };
        match self.templates[*template]
            .params
            .iter()
            .position(|found| found == param)
        {
            Some(index) => Some(args[index].clone()),
            None => {
                if report {
                    self.error(
                        span,
                        CheckError::UnknownSelfParam {
                            ty: impl_type,
                            param: param.to_string(),
                        },
                    );
                }
                None
            }
        }
    }

    /// The type `type_name` refers to where a value of one of `contexts` is
    /// expected or matched, reporting errors if `report` is set. A generic
    /// type's name without arguments refers to the first of `contexts` that's
    /// made from it, since that's what its arguments must be.
    fn type_in_context(
        &mut self,
        type_name: &TypeName,
        contexts: &[Option<&Type>],
        report: bool,
    ) -> Option<Type> {
        if let TypeName::Named(name) = type_name
            && let Ok(Binding::Generic(template)) = self.lookup(name)
        {
            let found = contexts
                .iter()
                .flatten()
                .find(|ty| self.is_made_from(ty, template));
            if found.is_none() && report {
                self.error(name.span, CheckError::CannotInferGeneric(name.to_string()));
            }
            return found.map(|&ty| ty.clone());
        }
        if report {
            self.resolve_type(type_name)
        } else {
            self.find_type(type_name)
        }
    }

    /// Declares the generic group, choice or primitive called `name`, with
    /// `params`, which `stmt` declares.
    fn declare_template(&mut self, name: &'a str, params: &'a [String], stmt: &'a Stmt) {
        let mut seen = HashSet::new();
        for param in params {
            if !seen.insert(param) {
                self.error(stmt.span, CheckError::Redeclared(param.clone()));
            }
        }
        self.templates.push(Template {
            name,
            module: self.module,
            params,
            stmt,
            impls: vec![],
        });
        self.declare(name, stmt.span, Binding::Generic(self.templates.len() - 1));
    }

    /// Checks the generic type at `template` in `templates` once, by making
    /// a type from it with a stand-in for each parameter, which can't be
    /// used as anything else. Returns that type, with the module it's in and
    /// where it's declared.
    fn check_template(&mut self, template: usize) -> Option<(usize, Type, Span)> {
        let Template {
            module,
            params,
            stmt,
            ..
        } = self.templates[template];
        let stand_ins = params
            .iter()
            .map(|param| {
                let id = self.group_fields.len();
                self.group_fields.push(vec![]);
                self.group_modules.push(module);
                self.type_names.push(param.clone());
                Type {
                    id,
                    name: param.clone(),
                }
            })
            .collect::<Vec<_>>();
        // Its first parameter stands for a choice, like its arguments.
        if self.is_only_for_choices(template) {
            self.choice_options.insert(stand_ins[0].id, vec![]);
        }
        let ty = self.instance(template, stand_ins, stmt.span, false)?;
        self.checked_instances.insert(ty.id);
        Some((module, ty, stmt.span))
    }

    /// The type made from the generic type at `template` in `templates` with
    /// `args`, which is made the first time it's needed, for the code at
    /// `span`. Its errors aren't reported if it's `quiet`, since each generic
    /// type is checked once on its own.
    fn instance(
        &mut self,
        template: usize,
        args: Vec<Type>,
        span: Span,
        quiet: bool,
    ) -> Option<Type> {
        // Arguments nested deeper than this almost certainly mean a type uses
        // itself with ever larger arguments, which would never end.
        const MAX_DEPTH: usize = 32;

        let shown: Vec<_> = args.iter().map(|arg| arg.name.clone()).collect();
        let name = format!("{}<{}>", self.templates[template].name, shown.join(", "));
        let key = (template, args.iter().map(|arg| arg.id).collect());
        if let Some(&id) = self.instance_ids.get(&key) {
            return Some(Type { id, name });
        }
        let depth = 1 + args
            .iter()
            .map(|arg| self.type_depth(arg.id))
            .max()
            .unwrap_or(0);
        if depth > MAX_DEPTH {
            self.error(span, CheckError::GenericTooDeep(name));
            return None;
        }

        let Template { module, stmt, .. } = self.templates[template];
        let id = self.group_fields.len();
        self.group_fields.push(vec![]);
        self.group_modules.push(module);
        self.type_names
            .push(format!("{}::{name}", self.modules[module].display));
        self.instance_ids.insert(key, id);
        self.instances.insert(id, (template, args.clone()));

        // Defined like the generic type, in its module, with its parameters
        // standing for `args`.
        let errors = self.errors.len();
        self.at_top_level(module, Some((template, args)), |checker| match &stmt.kind {
            StmtKind::Group { fields, .. } => checker.define_group(id, &name, fields),
            StmtKind::Choice { options, .. } => checker.define_choice(id, &name, options),
            StmtKind::Primitive { name, .. } => checker.define_primitive(id, name, stmt.span),
            _ => unreachable!("only groups, choices and primitives are generic"),
        });
        if quiet {
            self.quieten(errors);
        }
        if self.methods_ready {
            self.declare_instance_methods(id);
        } else {
            self.undeclared_instances.push(id);
        }
        Some(Type { id, name })
    }

    /// How deeply the type with `id` nests types made from generic types and
    /// tuples, which is 0 for other types.
    fn type_depth(&self, id: usize) -> usize {
        let parts: Vec<_> = match (self.instances.get(&id), self.tuples.get(&id)) {
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

    /// Declares the functions of the generic type's `impl`s for the type
    /// with `id` made from it, queueing their bodies to be checked.
    fn declare_instance_methods(&mut self, id: usize) {
        let (template, args) = &self.instances[&id];
        let shown: Vec<_> = args.iter().map(|arg| arg.name.clone()).collect();
        let template = *template;
        let ty = Type {
            id,
            name: format!("{}<{}>", self.templates[template].name, shown.join(", ")),
        };
        let quiet = !self.checked_instances.contains(&id);
        for (module, stmt) in self.templates[template].impls.clone() {
            let StmtKind::Impl {
                ty: type_name,
                with,
                aliases,
                functions,
            } = &stmt.kind
            else {
                unreachable!("only `impl`s are recorded");
            };
            let declared = self.at_top_level(module, None, |checker| {
                checker.declare_impl(
                    type_name,
                    Some(ty.clone()),
                    with.as_ref(),
                    aliases,
                    functions,
                    quiet,
                )
            });
            self.pending
                .extend(declared.into_iter().map(|pending| (module, pending)));
        }
    }

    /// Runs `check` as if it were at the top level of `module`, outside any
    /// function or `impl`, with `type_params` as `Checker::type_params`.
    /// Types made from generic types are defined like this, wherever they're
    /// first needed.
    fn at_top_level<T>(
        &mut self,
        module: usize,
        type_params: Option<(usize, Vec<Type>)>,
        check: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let module = std::mem::replace(&mut self.module, module);
        let scopes = std::mem::take(&mut self.scopes);
        let fn_scope_start = std::mem::replace(&mut self.fn_scope_start, 0);
        let current_fn = self.current_fn.take();
        let impl_type = self.impl_type.take();
        let impl_aliases = self.current_impl_aliases.take();
        let type_params = std::mem::replace(&mut self.type_params, type_params);
        let written_in = self.written_in.take();
        let checked = check(self);
        self.written_in = written_in;
        self.module = module;
        self.scopes = scopes;
        self.fn_scope_start = fn_scope_start;
        self.current_fn = current_fn;
        self.impl_type = impl_type;
        self.current_impl_aliases = impl_aliases;
        self.type_params = type_params;
        checked
    }

    /// Stops reporting the errors after the first `errors`, keeping them in
    /// `quietened`. A generic type nested too deeply is still reported, since
    /// it's only found while making the types nested in it, but only the
    /// first time, since the types around it are too deep for the same reason.
    fn quieten(&mut self, errors: usize) {
        for error in self.errors.split_off(errors) {
            if !matches!(error.error, CheckError::GenericTooDeep(_)) {
                self.quietened.push(error);
            } else if !self
                .errors
                .iter()
                .any(|found| matches!(found.error, CheckError::GenericTooDeep(_)))
            {
                self.errors.push(error);
            }
        }
    }

    /// Finds the type called `name`, reporting an error if it can't be found.
    fn resolve_named_type(&mut self, name: &Name) -> Option<Type> {
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
    fn alias_type(&mut self, index: usize) -> Option<Type> {
        let alias = &self.aliases[index];
        match &alias.state {
            LazyType::Checked(ty) => return ty.clone(),
            LazyType::Checking => {
                let error = CheckError::CyclicAlias(alias.name.to_string());
                self.error_in(alias.module, alias.span, error);
                return None;
            }
            LazyType::Unchecked => {}
        }

        // Worked out in its own module, even when another module needs it
        // first. Only top-level aliases are worked out later, and they're all
        // worked out before any functions are checked, so there are no local
        // scopes to set aside.
        let (module, target) = (alias.module, alias.target);
        self.aliases[index].state = LazyType::Checking;
        let outer_module = std::mem::replace(&mut self.module, module);
        let ty = self.resolve_type(target);
        self.module = outer_module;

        self.aliases[index].state = LazyType::Checked(ty.clone());
        ty
    }

    /// The type of the top-level value at `index` in `globals`, checking its
    /// declaration if that hasn't been done yet.
    fn global_type(&mut self, index: usize) -> Option<Type> {
        let global = &self.globals[index];
        match &global.state {
            LazyType::Checked(ty) => return ty.clone(),
            LazyType::Checking => {
                let error = CheckError::CyclicValue(global.name.to_string());
                self.error_in(global.module, global.span, error);
                return None;
            }
            LazyType::Unchecked => {}
        }

        // Checked as part of its own module, even when another module needs
        // it first. Top-level values are only used at the top level, so
        // there are no local scopes to set aside.
        let (module, name, type_name, value) =
            (global.module, global.name, global.ty, global.value);
        self.globals[index].state = LazyType::Checking;
        let outer_module = std::mem::replace(&mut self.module, module);
        let Typed { ty, expr } = self.decl_type(name, type_name, value);
        self.module = outer_module;

        self.globals[index].state = LazyType::Checked(ty.clone());
        self.globals[index].lowered = expr;
        self.global_order.push(index);
        ty
    }

    /// Checks a `let` or `var` declaration's value, returning the type it's
    /// declared with, along with what the value compiles to.
    fn decl_type(&mut self, name: &str, type_name: Option<&TypeName>, value: &Expr) -> Typed {
        // Checked first so that `let x = x` refers to an earlier `x`. The
        // explicit type is found first so the value can take its type from
        // it, but its errors are reported after the value's.
        let expected = type_name.and_then(|type_name| self.find_type(type_name));
        let Typed { ty: found, expr } = self.expr_as(value, expected.as_ref());

        // An explicit type takes precedence over the value's type, so later
        // uses are checked against what the programmer wrote.
        let ty = match type_name {
            Some(type_name) => {
                let expected = self.resolve_type(type_name);
                self.check_assignable(name, value.span, expected.clone(), found);
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
    fn declare_fn(
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
    fn declare_method(
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
            // Methods must be unique across every module, even ones that
            // can't see each other, since a call can't say which module's
            // method it means.
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
                let found = Method {
                    module: self.module,
                    function: index,
                };
                let table = if is_static {
                    &mut self.statics
                } else {
                    &mut self.methods
                };
                table.entry(self_ty.id).or_default().insert(name, found);
            }
        }
        index
    }

    /// Resolves the parameter and return types of the function called
    /// `full_name`, returning its index into `functions`.
    fn signature(&mut self, params: &'a [Param], ret: &TypeName, full_name: String) -> usize {
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

        self.functions.push(Signature {
            name: full_name,
            params,
            returns,
        });
        self.lowered_functions.push(None);
        self.functions.len() - 1
    }

    /// Checks the body of the function at `index` in `functions`.
    /// `aliases` are the aliases of the `impl` a method or static function is
    /// in, as an index into `impl_aliases`. Other functions keep the aliases
    /// of the code they're nested in.
    fn fn_body(
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
        let outer_impl_aliases = match &kind {
            FnKind::Method(_) | FnKind::Static(_) => {
                std::mem::replace(&mut self.current_impl_aliases, aliases)
            }
            FnKind::Function => self.current_impl_aliases,
        };

        // `self` is a keyword, so it can't clash with a parameter. Like any
        // other value, functions nested in the method can't use it.
        if let FnKind::Method(ty) = kind {
            self.declare_local("self", span, Mutability::Let, ty);
        }
        for param in params {
            self.declare_local(param.name, param.span, Mutability::Let, param.ty);
        }
        let param_count = self.current_fn.as_ref().expect("set above").locals.len();
        let mut lowered: Vec<_> = body.iter().filter_map(|stmt| self.stmt(stmt)).collect();

        self.scopes.pop();
        let locals = std::mem::replace(&mut self.current_fn, outer_fn)
            .expect("set above")
            .locals;
        self.fn_scope_start = outer_scope_start;
        self.impl_type = outer_impl_type;
        self.current_impl_aliases = outer_impl_aliases;

        // A `Void` function can finish without an explicit `return`.
        if !self.always_returns(body) {
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

    /// Whether `stmts`, which have been checked, return on every path. A
    /// statement that evaluates a `Never` counts, since it never finishes.
    /// Both branches of an `if` must return for it to count, so an `if`
    /// without an `else` never does. Every option of a `match` must return,
    /// which is enough since every option is matched.
    fn always_returns(&self, stmts: &[Stmt]) -> bool {
        stmts.iter().any(|stmt| {
            self.never_returns.contains(&(stmt as *const Stmt))
                || match &stmt.kind {
                    StmtKind::Return(_) => true,
                    StmtKind::Match { arms, .. } => {
                        arms.iter().all(|arm| self.always_returns(&arm.body))
                    }
                    StmtKind::If {
                        then_block,
                        else_block: Some(else_block),
                        ..
                    } => self.always_returns(then_block) && self.always_returns(else_block),
                    _ => false,
                }
        })
    }

    /// Declares the trait called `name` in the current module. Its signatures
    /// are worked out by `define_trait`, once types are defined.
    fn declare_trait(
        &mut self,
        name: &'a str,
        span: Span,
        aliases: &'a [String],
        functions: &'a [TraitFunction],
    ) {
        self.traits.push(TraitInfo {
            name,
            span,
            module: self.module,
            aliases,
            functions,
            signatures: vec![],
        });
        self.declare(name, span, Binding::Trait(self.traits.len() - 1));
    }

    /// Works out the types in the signatures of the trait at `index` in
    /// `traits`, where `Self` stands for whichever type implements it, and
    /// its aliases for whichever types the implementation gives.
    fn define_trait(&mut self, index: usize) {
        let TraitInfo {
            name,
            span,
            module,
            aliases,
            functions,
            ..
        } = self.traits[index];
        self.module = module;
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

        let mut signatures = vec![];
        for function in functions {
            let params = function
                .params
                .iter()
                .map(|param| self.trait_type(aliases, &param.ty))
                .collect();
            signatures.push((params, self.trait_type(aliases, &function.ret)));
        }
        self.traits[index].signatures = signatures;
    }

    /// Resolves the type `ty` in the signature of a trait with `aliases`.
    fn trait_type(&mut self, aliases: &[String], ty: &TypeName) -> Option<TraitType> {
        let TypeName::Named(name) = ty else {
            // `Self` and the aliases are a different type in each `impl`, so
            // they can't be in a tuple, whose type is always the same.
            if let Some(name) = trait_names(ty, aliases).next() {
                self.error(name.span, CheckError::TraitTypeInTuple(name.name.clone()));
                return None;
            }
            return self.resolve_type(ty).map(TraitType::Type);
        };
        if name.is_self() {
            return Some(TraitType::SelfType);
        }
        if name.module.is_none()
            && let Some(index) = aliases.iter().position(|alias| *alias == name.name)
        {
            return Some(TraitType::Alias(index));
        }
        self.resolve_named_type(name).map(TraitType::Type)
    }

    /// Works out the types of an `impl`'s `aliases`, which can use `Self`
    /// and the aliases before them, returning their index in `impl_aliases`
    /// if there are any, or it has a trait, whose index in `traits` is
    /// `trait_index`. While its functions are declared, they can be used.
    ///
    /// The trait's aliases that aren't given are inferred by
    /// `infer_impl_aliases` once its functions are declared, so until then,
    /// they can't be used.
    fn define_impl_aliases(
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
        self.current_impl_aliases = Some(index);
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
    fn infer_impl_aliases(
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
        for (alias, name) in trait_.aliases.iter().enumerate() {
            if !matches!(
                self.impl_aliases[aliases].get(name.as_str()),
                Some(ImplAliasType::Inferring)
            ) {
                continue;
            }
            let is_alias = |expected: &Option<TraitType>| matches!(expected, Some(TraitType::Alias(found)) if *found == alias);
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
    /// `functions`, returning their bodies to check. They're checked as if
    /// they were written in the `impl`, so `Self`, which is `self_ty`, and
    /// the aliases, at `aliases` in `impl_aliases`, are the `impl`'s, and
    /// they can be used like its own functions.
    fn declare_defaults(
        &mut self,
        type_name: &str,
        self_ty: Option<&Type>,
        trait_index: usize,
        functions: &'a [ImplFunction],
        aliases: Option<usize>,
        quiet: bool,
    ) -> Vec<Pending<'a>> {
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

        let outer_impl_type = self.impl_type.replace(self_ty.cloned());
        self.current_impl_aliases = aliases;
        let written_in = self.traits[trait_index].module;
        let outer_written_in = self.written_in.replace(written_in);
        let mut pending = vec![];
        for declared in declared.iter().filter(|declared| !is_implemented(declared)) {
            let Some(body) = &declared.body else {
                continue;
            };
            let name = format!("{type_name}.{}", declared.name);
            let index = self.declare_method(
                self_ty,
                (&declared.name, &declared.params, &declared.ret),
                declared.span,
                declared.is_static,
                self.full_name(&name),
            );
            let kind = if declared.is_static {
                FnKind::Static(self_ty.cloned())
            } else {
                FnKind::Method(self_ty.cloned())
            };
            if complete {
                pending.push(Pending::Fn {
                    index,
                    name: &declared.name,
                    span: declared.span,
                    body,
                    kind,
                    aliases,
                    quiet,
                    written_in: Some(written_in),
                });
            }
        }
        self.written_in = outer_written_in;
        self.impl_type = outer_impl_type;
        self.current_impl_aliases = None;
        pending
    }

    /// Finds the trait called `name`, returning its index into `traits`.
    fn resolve_trait(&mut self, name: &Name) -> Option<usize> {
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
    fn check_implements(
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
        let alias_type = |alias: usize| {
            let types = &self.impl_aliases[alias_types?];
            match types.get(trait_.aliases[alias].as_str())? {
                ImplAliasType::Known(ty) => ty.as_ref(),
                ImplAliasType::Inferring => None,
            }
        };
        let matches = |found: &Option<Type>, expected: &Option<TraitType>| match (found, expected) {
            (Some(found), Some(TraitType::SelfType)) => found == self_ty,
            (Some(found), Some(TraitType::Alias(alias))) => {
                alias_type(*alias).is_none_or(|expected| found == expected)
            }
            (Some(found), Some(TraitType::Type(expected))) => found == expected,
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

    /// Records the primitive function called `name`, such as `exit` or
    /// `Bool.and`, at `index` in `functions`, which must be declared in the
    /// standard library.
    fn define_primitive_fn(&mut self, index: usize, name: &str, span: Span, kind: FnKind) {
        if self.modules[self.module].package != Some(STDLIB_PACKAGE) {
            self.error(span, CheckError::PrimitiveOutsideStdlib(name.to_string()));
            return;
        }
        let primitive = match name {
            "exit" => ir::PrimitiveFn::Exit,
            "Bool.and" => ir::PrimitiveFn::BoolAnd,
            "Bool.or" => ir::PrimitiveFn::BoolOr,
            "Bool.not" => ir::PrimitiveFn::BoolNot,
            "IntU8.isEqual" | "IntU16.isEqual" | "IntU32.isEqual" | "IntU64.isEqual"
            | "IntS8.isEqual" | "IntS16.isEqual" | "IntS32.isEqual" | "IntS64.isEqual"
            | "Bool.isEqual" => ir::PrimitiveFn::Equal,
            "IntU8.isNotEqual" | "IntU16.isNotEqual" | "IntU32.isNotEqual"
            | "IntU64.isNotEqual" | "IntS8.isNotEqual" | "IntS16.isNotEqual"
            | "IntS32.isNotEqual" | "IntS64.isNotEqual" | "Bool.isNotEqual" => {
                ir::PrimitiveFn::NotEqual
            }
            // Every type made from a generic type has its own functions.
            _ if name.starts_with("Fn<") && name.ends_with(">.call") => ir::PrimitiveFn::CallFn,
            _ if name.starts_with("ChoiceTag<") && name.ends_with(">.new") => {
                ir::PrimitiveFn::ChoiceTag
            }
            _ if name.starts_with("ChoiceTag<") && name.ends_with(">.isEqual") => {
                ir::PrimitiveFn::Equal
            }
            _ if name.starts_with("ChoiceTag<") && name.ends_with(">.isNotEqual") => {
                ir::PrimitiveFn::NotEqual
            }
            _ if name.starts_with("ChoiceOption<") && name.ends_with(">.tag") => {
                ir::PrimitiveFn::OptionTag
            }
            _ if name.starts_with("ChoiceOption<") && name.ends_with(">.call") => {
                ir::PrimitiveFn::CallOption
            }
            _ => panic!("the compiler doesn't know how to implement `{name}`"),
        };
        let Signature {
            name: full_name,
            params,
            returns,
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

    /// Declares a value in the current function, returning its index among
    /// the function's locals.
    fn declare_local(
        &mut self,
        name: &'a str,
        span: Span,
        mutability: Mutability,
        ty: Option<Type>,
    ) -> usize {
        let locals = &mut self
            .current_fn
            .as_mut()
            .expect("values are only declared inside functions")
            .locals;
        locals.push(ty.clone());
        let local = locals.len() - 1;
        self.declare(
            name,
            span,
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

    /// Declares an alias called `name` of the type called `target`, whose
    /// type has been worked out if `state` says so.
    fn declare_alias(&mut self, name: &'a str, span: Span, target: &'a TypeName, state: LazyType) {
        self.aliases.push(Alias {
            module: self.module,
            name,
            span,
            target,
            state,
        });
        self.declare(name, span, Binding::Alias(self.aliases.len() - 1));
    }

    /// Declares a type without its fields or options, returning its `id`.
    fn declare_type(&mut self, name: &'a str, span: Span) -> usize {
        let id = self.group_fields.len();
        self.group_fields.push(vec![]);
        self.group_modules.push(self.module);
        self.type_names.push(self.full_name(name));
        let ty = Type {
            id,
            name: name.to_string(),
        };
        self.declare(name, span, Binding::Type(ty));
        id
    }

    /// Resolves the fields or options of `stmt`, which declares the type with
    /// `id`.
    fn define_type(&mut self, id: usize, stmt: &'a Stmt) {
        match &stmt.kind {
            StmtKind::Group { name, fields, .. } => self.define_group(id, name, fields),
            StmtKind::Choice { name, options, .. } => self.define_choice(id, name, options),
            StmtKind::Primitive { name, .. } => self.define_primitive(id, name, stmt.span),
            _ => unreachable!("only groups, choices and primitives declare types"),
        }
    }

    /// Records the primitive type called `name` with `id`, which must be
    /// declared in the standard library.
    fn define_primitive(&mut self, id: usize, name: &str, span: Span) {
        if self.modules[self.module].package != Some(STDLIB_PACKAGE) {
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
        self.primitives.insert(id, primitive);
    }

    /// Resolves the options of the choice called `name` with `id`.
    fn define_choice(&mut self, id: usize, name: &str, options: &'a [ChoiceOption]) {
        // Its options can use it as a choice.
        self.choice_options.insert(id, vec![]);
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
                        },
                        member: option.name.clone(),
                        existing: MemberKind::Option,
                    },
                );
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
                self.error(
                    field.span,
                    CheckError::MemberRedeclared {
                        ty: Type {
                            id,
                            name: name.to_string(),
                        },
                        member: field.name.clone(),
                        existing: MemberKind::Field,
                    },
                );
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
        let elements = self.tuples.get(&id).into_iter().flatten();
        self.group_fields[id]
            .iter()
            .map(|field| &field.ty)
            .chain(options.map(|option| &option.ty))
            .filter_map(|ty| ty.as_ref().map(|ty| ty.id))
            .chain(elements.map(|element| element.ty.id))
    }

    /// Reports an error if the group or choice with `id` contains itself,
    /// which would make it infinitely large.
    fn check_not_recursive(&mut self, id: usize, name: &str, span: Span) {
        let mut seen = HashSet::new();
        let mut unvisited: Vec<_> = self.contained_types(id).collect();
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
        match &stmt.kind {
            StmtKind::Use(_)
            | StmtKind::Impl { .. }
            | StmtKind::Trait { .. }
            | StmtKind::Primitive { .. }
            | StmtKind::PrimitiveFn { .. } => {
                unreachable!(
                    "the parser only allows `use`, `impl`, `trait` and `primitive` at the top level"
                )
            }
            StmtKind::Decl {
                mutability,
                name,
                ty: type_name,
                value,
            } => {
                let Typed { ty, expr } = self.evaluating(stmt, |checker| {
                    checker.decl_type(name, type_name.as_ref(), value)
                });
                let local = self.declare_local(name, stmt.span, *mutability, ty);
                Some(ir::Stmt::Assign {
                    local,
                    value: expr?,
                })
            }
            StmtKind::Assign { name, value } => {
                // The target is found first so the value can take its type
                // from it, but its errors are reported after the value's.
                let target = Name {
                    span: stmt.span,
                    ..Name::new(name)
                };
                let binding = self.lookup(&target);
                let expected = match &binding {
                    Ok(Binding::Value { ty, .. }) => ty.clone(),
                    _ => None,
                };
                let found =
                    self.evaluating(stmt, |checker| checker.expr_as(value, expected.as_ref()));
                match binding {
                    Ok(Binding::Value {
                        mutability: Mutability::Var,
                        ty: expected,
                        local,
                    }) => {
                        self.check_assignable(name, value.span, expected, found.ty);
                        return Some(ir::Stmt::Assign {
                            local,
                            value: found.expr?,
                        });
                    }
                    Ok(Binding::Value {
                        mutability: Mutability::Let,
                        ..
                    }) => {
                        self.error(stmt.span, CheckError::AssignToConstant(name.clone()));
                    }
                    Ok(Binding::Function(_)) => {
                        self.error(stmt.span, CheckError::AssignToFunction(name.clone()));
                    }
                    Ok(Binding::Type(_) | Binding::Alias(_) | Binding::Generic(_)) => {
                        self.error(stmt.span, CheckError::TypeAsValue(name.clone()));
                    }
                    Ok(Binding::Trait(_)) => {
                        self.error(stmt.span, CheckError::TraitAsValue(name.clone()));
                    }
                    // `lookup` never finds top-level values from inside a
                    // function, which is the only place assignments can be.
                    Ok(Binding::Global(_)) => self.lookup_error(&target, LookupError::Captured),
                    Err(error) => self.lookup_error(&target, error),
                }
                None
            }
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond_span = cond.span;
                let cond = self.evaluating(stmt, |checker| checker.expr(cond));
                if let Some(found) = &cond.ty
                    && *found != self.std_type("Bool")
                {
                    self.error(cond_span, CheckError::ConditionNotBool(found.clone()));
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
            StmtKind::Fn(function) => {
                // Declared before the body is checked, so it can call itself.
                let index = self.declare_fn(
                    &function.name,
                    function.span,
                    &function.params,
                    &function.ret,
                );
                self.fn_body(
                    index,
                    &function.name,
                    function.span,
                    &function.body,
                    FnKind::Function,
                    None,
                );
                None
            }
            StmtKind::Group { name, params, .. } | StmtKind::Choice { name, params, .. }
                if !params.is_empty() =>
            {
                self.error(stmt.span, CheckError::GenericInFunction(name.clone()));
                None
            }
            StmtKind::Group { name, .. } | StmtKind::Choice { name, .. } => {
                let id = self.declare_type(name, stmt.span);
                self.define_type(id, stmt);
                self.check_not_recursive(id, name, stmt.span);
                None
            }
            // Like other names inside functions, an alias is only visible
            // after it's declared, so it's worked out straight away.
            StmtKind::Alias { name, ty } => {
                let target = self.resolve_type(ty);
                self.declare_alias(name, stmt.span, ty, LazyType::Checked(target));
                None
            }
            StmtKind::Match { value, arms } => self.match_(stmt, value, arms),
            StmtKind::Destructure { pattern, value } => self.destructure(stmt, pattern, value),
            StmtKind::Return(value) => {
                let FnContext { name, returns, .. } = self
                    .current_fn
                    .clone()
                    .expect("statements are only checked inside functions");
                let found = value.as_ref().map(|value| {
                    self.evaluating(stmt, |checker| checker.expr_as(value, returns.as_ref()))
                });

                // A bare `return` returns `Void`.
                let function = name.to_string();
                match (returns, found.as_ref().map(|found| &found.ty)) {
                    (Some(expected), None) if expected != self.std_type("Void") => {
                        self.error(
                            stmt.span,
                            CheckError::MissingReturnValue { function, expected },
                        );
                    }
                    (Some(expected), Some(Some(found))) if expected != *found => {
                        self.error(
                            value.as_ref().map_or(stmt.span, |value| value.span),
                            CheckError::ReturnTypeMismatch {
                                function,
                                expected,
                                found: found.clone(),
                            },
                        );
                    }
                    _ => {}
                }
                match found {
                    Some(found) => found.expr.map(ir::Stmt::Return),
                    None => Some(ir::Stmt::Return(self.void())),
                }
            }
            StmtKind::Expr(expr) => {
                // A call's return value can be ignored.
                let found = self.evaluating(stmt, |checker| checker.expr(expr));
                found.expr.map(ir::Stmt::Expr)
            }
        }
    }

    /// Checks the expressions of `stmt` with `check`. If they evaluate a
    /// `Never`, `stmt` never finishes, so it counts as returning.
    fn evaluating<T>(&mut self, stmt: &'a Stmt, check: impl FnOnce(&mut Self) -> T) -> T {
        self.evaluated_never = false;
        let checked = check(self);
        if self.evaluated_never {
            self.never_returns.insert(stmt);
        }
        checked
    }

    /// Checks a `match` on `value`, whose arms' patterns must together match
    /// every value it could be, with each matching something the ones before
    /// it don't, returning what it compiles to.
    fn match_(&mut self, stmt: &'a Stmt, value: &Expr, arms: &'a [MatchArm]) -> Option<ir::Stmt> {
        let Typed { ty, expr } = self.evaluating(stmt, |checker| checker.expr(value));
        let mut lowered = vec![];
        let mut spaces = vec![];
        for arm in arms {
            // A pattern's bindings are only visible in its arm's block. Arms
            // are still checked when the type is unknown, so the errors in
            // their blocks are found.
            self.scopes.push(HashMap::new());
            let (pattern, space) = self.pattern(&arm.pattern, ty.as_ref());
            let body = arm.body.iter().filter_map(|stmt| self.stmt(stmt)).collect();
            self.scopes.pop();
            spaces.push(space);
            if let Some(pattern) = pattern {
                lowered.push(ir::MatchArm { pattern, body });
            }
        }

        // Patterns with errors have already been reported, and can't be told
        // apart from patterns that match anything.
        let ty = ty?;
        if lowered.len() < arms.len() {
            return None;
        }
        self.check_exhaustive(&ty, stmt.span, arms, spaces);
        Some(ir::Stmt::Match {
            value: expr?,
            ty: ty.id,
            arms: lowered,
        })
    }

    /// Checks `pattern = value`, which declares the values that `pattern`
    /// binds in the current scope. With nothing to fall back on, `pattern`
    /// must match every value it could be given. It compiles to a `match`
    /// with one arm, whose block is empty.
    fn destructure(
        &mut self,
        stmt: &'a Stmt,
        pattern: &'a Pattern,
        value: &Expr,
    ) -> Option<ir::Stmt> {
        let Typed { ty, expr } = self.evaluating(stmt, |checker| checker.expr(value));
        let (lowered, space) = self.pattern(pattern, ty.as_ref());
        let (ty, lowered) = (ty?, lowered?);
        let tys = std::slice::from_ref(&ty);
        if let Some(values) = self
            .unmatched(&[vec![space]], &[Space::Any], tys, 1)
            .first()
        {
            self.error(
                pattern.span,
                CheckError::RefutablePattern {
                    pattern: pattern.to_string(),
                    missing: self.show_space(&values[0], &ty),
                },
            );
            return None;
        }
        Some(ir::Stmt::Match {
            value: expr?,
            ty: ty.id,
            arms: vec![ir::MatchArm {
                pattern: lowered,
                body: vec![],
            }],
        })
    }

    /// Checks `pattern` against a value of type `ty`, declaring the values it
    /// binds in the current scope. Returns what it compiles to, or `None` if
    /// it has an error or `ty` is unknown, along with which values it matches.
    fn pattern(&mut self, pattern: &'a Pattern, ty: Option<&Type>) -> (Option<ir::Pattern>, Space) {
        let Some(ty) = ty else {
            // The values it binds are still declared, so their uses aren't
            // reported as undeclared.
            for (binding, span) in bindings(pattern) {
                self.declare_local(binding, span, Mutability::Let, None);
            }
            return (None, Space::Any);
        };
        let mismatch = |checker: &mut Self| {
            checker.error(
                pattern.span,
                CheckError::PatternMismatch {
                    pattern: pattern.to_string(),
                    ty: ty.clone(),
                },
            );
            for (binding, span) in bindings(pattern) {
                checker.declare_local(binding, span, Mutability::Let, None);
            }
            (None, Space::Any)
        };
        match &pattern.kind {
            PatternKind::Any => (Some(ir::Pattern::Any), Space::Any),
            PatternKind::Binding(name) => {
                let local =
                    self.declare_local(name, pattern.span, Mutability::Let, Some(ty.clone()));
                (Some(ir::Pattern::Binding(local)), Space::Any)
            }
            PatternKind::Int { value, negative } => {
                let Some((bits, signed)) = self.int_type(ty) else {
                    return mismatch(self);
                };
                let value = if *negative {
                    -i128::from(*value)
                } else {
                    i128::from(*value)
                };
                let (min, max) = int_range(bits, signed);
                if !(min..=max).contains(&value) {
                    self.error(
                        pattern.span,
                        CheckError::IntOutOfRange {
                            value,
                            ty: ty.clone(),
                            min,
                            max,
                        },
                    );
                    return (None, Space::Any);
                }
                // Stored as its bits, like an integer literal.
                let bits = (value as u128 & (u128::MAX >> (128 - bits))) as u64;
                (
                    Some(ir::Pattern::Int(bits)),
                    Space::Ctor(Ctor::Int(value), vec![]),
                )
            }
            PatternKind::Bool(value) if *ty == self.std_type("Bool") => (
                Some(ir::Pattern::Bool(*value)),
                Space::Ctor(Ctor::Bool(*value), vec![]),
            ),
            PatternKind::Bool(_) => mismatch(self),
            PatternKind::Tuple(elements) => {
                // The labels must be the ones the tuple is built with.
                let types = self.tuples.get(&ty.id).cloned().filter(|types| {
                    types.len() == elements.len()
                        && types
                            .iter()
                            .zip(elements)
                            .all(|(ty, element)| ty.label == element.label)
                });
                let Some(types) = types else {
                    return mismatch(self);
                };
                let mut lowered = vec![];
                let mut spaces = vec![];
                for (element, element_ty) in elements.iter().zip(&types) {
                    let (pattern, space) = self.pattern(&element.pattern, Some(&element_ty.ty));
                    lowered.push(pattern);
                    spaces.push(space);
                }
                let lowered = lowered.into_iter().collect::<Option<_>>();
                (
                    lowered.map(ir::Pattern::Tuple),
                    Space::Ctor(Ctor::Tuple, spaces),
                )
            }
            PatternKind::Option {
                ty: type_name,
                option,
                value,
            } => {
                let found = self.option_pattern(ty, type_name, option, pattern.span);
                let value_ty = found.as_ref().and_then(|(_, value_ty)| value_ty.as_ref());
                let (value, space) = match value {
                    Some(value) => self.pattern(value, value_ty),
                    None => (Some(ir::Pattern::Any), Space::Any),
                };
                let Some((index, _)) = found else {
                    return (None, Space::Any);
                };
                (
                    value.map(|value| ir::Pattern::Option {
                        index,
                        value: Box::new(value),
                    }),
                    Space::Ctor(Ctor::Option(index), vec![space]),
                )
            }
        }
    }

    /// Finds the option called `option` of `type_name`, which must be `ty`,
    /// returning its index and the type of its value, as `Checker::option`
    /// does, or `None` if it has an error.
    fn option_pattern(
        &mut self,
        ty: &Type,
        type_name: &TypeName,
        option: &str,
        span: Span,
    ) -> Option<(usize, Option<Type>)> {
        // A generic type's arguments can be left out, since they're the
        // matched value's.
        let found = self.type_in_context(type_name, &[Some(ty)], true)?;
        if found != *ty {
            self.error(
                span,
                CheckError::WrongPatternType {
                    expected: ty.clone(),
                    found,
                },
            );
            return None;
        }
        let Some(option) = self.option(&found, option) else {
            let option = option.to_string();
            self.error(span, CheckError::UnknownOption { ty: found, option });
            return None;
        };
        Some(option)
    }

    /// Reports the patterns of `arms`, which match `spaces`, that can't be
    /// reached, since the ones before them match everything they do, and the
    /// values of type `ty` that none of them match, for the `match` at `span`.
    fn check_exhaustive(&mut self, ty: &Type, span: Span, arms: &[MatchArm], spaces: Vec<Space>) {
        // A few missing values are shown, and one more shows there are more.
        const SHOWN: usize = 3;

        let mut rows: Vec<Vec<Space>> = vec![];
        let mut errors = vec![];
        let tys = std::slice::from_ref(ty);
        for (arm, space) in arms.iter().zip(spaces) {
            if self
                .unmatched(&rows, std::slice::from_ref(&space), tys, 1)
                .is_empty()
            {
                errors.push((
                    arm.pattern.span,
                    CheckError::UnreachablePattern(arm.pattern.to_string()),
                ));
            }
            rows.push(vec![space]);
        }
        let missing = self.unmatched(&rows, &[Space::Any], tys, SHOWN + 1);
        if !missing.is_empty() {
            let shown = missing.iter().take(SHOWN);
            errors.push((
                span,
                CheckError::MissingPatterns {
                    patterns: shown
                        .map(|values| self.show_space(&values[0], ty))
                        .collect(),
                    more: missing.len() > SHOWN,
                },
            ));
        }
        for (span, error) in errors {
            self.error(span, error);
        }
    }

    /// Up to `limit` values that `values` matches but none of `rows` do,
    /// where `values` and each row match a value of each of `tys` in turn.
    ///
    /// This is the usual way of checking patterns: values are split up by
    /// how they're made, as far as patterns can tell, such as which option
    /// of a choice they are, and each way is checked against the rows that
    /// match it.
    fn unmatched(
        &self,
        rows: &[Vec<Space>],
        values: &[Space],
        tys: &[Type],
        limit: usize,
    ) -> Vec<Vec<Space>> {
        let Some((first, rest)) = values.split_first() else {
            // Nothing is left to match, so the values are unmatched only if
            // there are no rows.
            return if rows.is_empty() {
                vec![vec![]]
            } else {
                vec![]
            };
        };
        let (ty, rest_tys) = (&tys[0], &tys[1..]);
        if let Space::Ctor(ctor, parts) = first {
            return self.unmatched_ctor(rows, ctor, parts.clone(), rest, ty, rest_tys, limit);
        }

        let ctors = self.ctors(ty);
        // When no row is left, every value is unmatched, which is shown as
        // `_` rather than each way of making it, unless there are no values,
        // as for `Never`.
        if rows.is_empty() && ctors.as_ref().is_none_or(|ctors| !ctors.is_empty()) {
            return vec![vec![Space::Any; values.len()]];
        }

        let mut used: Vec<&Ctor> = vec![];
        for row in rows {
            if let Space::Ctor(ctor, _) = &row[0]
                && !used.contains(&ctor)
            {
                used.push(ctor);
            }
        }
        // Each way of making a value that a row uses is checked against the
        // rows that match it, in the order they're declared if they're known.
        let checked: Vec<&Ctor> = match &ctors {
            Some(ctors) => ctors.iter().filter(|ctor| used.contains(ctor)).collect(),
            None => used.clone(),
        };
        let mut found = vec![];
        for ctor in checked {
            let parts = vec![Space::Any; self.ctor_types(ty, ctor).len()];
            let left = limit - found.len();
            found.extend(self.unmatched_ctor(rows, ctor, parts, rest, ty, rest_tys, left));
            if found.len() >= limit {
                return found;
            }
        }

        // A value made any other way is only matched by the rows that match
        // anything first. It's shown as each way that no row uses, if they're
        // known, and `_` otherwise.
        let unused: Vec<Space> = match &ctors {
            Some(ctors) => ctors
                .iter()
                .filter(|ctor| !used.contains(ctor))
                .map(|ctor| {
                    let parts = vec![Space::Any; self.ctor_types(ty, ctor).len()];
                    Space::Ctor(ctor.clone(), parts)
                })
                .collect(),
            None => vec![Space::Any],
        };
        if unused.is_empty() {
            return found;
        }
        let defaults: Vec<_> = rows
            .iter()
            .filter(|row| matches!(row[0], Space::Any))
            .map(|row| row[1..].to_vec())
            .collect();
        let others = self.unmatched(&defaults, rest, rest_tys, limit - found.len());
        for first in unused {
            for other in &others {
                found.push(
                    iter::once(first.clone())
                        .chain(other.iter().cloned())
                        .collect(),
                );
                if found.len() >= limit {
                    return found;
                }
            }
        }
        found
    }

    /// Like `unmatched`, for values made with `ctor` from `parts`, followed
    /// by `rest`.
    #[allow(clippy::too_many_arguments)]
    fn unmatched_ctor(
        &self,
        rows: &[Vec<Space>],
        ctor: &Ctor,
        parts: Vec<Space>,
        rest: &[Space],
        ty: &Type,
        rest_tys: &[Type],
        limit: usize,
    ) -> Vec<Vec<Space>> {
        let part_tys = self.ctor_types(ty, ctor);
        let arity = part_tys.len();
        // The rows that match values made this way, with their parts.
        let specialised: Vec<Vec<Space>> = rows
            .iter()
            .filter_map(|row| {
                let parts = match &row[0] {
                    Space::Ctor(found, parts) if found == ctor => parts.clone(),
                    Space::Ctor(..) => return None,
                    Space::Any => vec![Space::Any; arity],
                };
                Some(parts.into_iter().chain(row[1..].iter().cloned()).collect())
            })
            .collect();
        let values: Vec<_> = parts.into_iter().chain(rest.iter().cloned()).collect();
        let tys: Vec<_> = part_tys
            .into_iter()
            .chain(rest_tys.iter().cloned())
            .collect();
        self.unmatched(&specialised, &values, &tys, limit)
            .into_iter()
            .map(|mut found| {
                let rest = found.split_off(arity);
                iter::once(Space::Ctor(ctor.clone(), found))
                    .chain(rest)
                    .collect()
            })
            .collect()
    }

    /// Every way of making a value of `ty` that patterns can tell apart, or
    /// `None` if there are too many to list, such as for integers, or
    /// patterns can't tell them apart, such as for groups.
    fn ctors(&self, ty: &Type) -> Option<Vec<Ctor>> {
        if *ty == self.std_type("Bool") {
            Some(vec![Ctor::Bool(false), Ctor::Bool(true)])
        } else if self.tuples.contains_key(&ty.id) {
            Some(vec![Ctor::Tuple])
        } else {
            let options = self.choice_options.get(&ty.id)?;
            Some((0..options.len()).map(Ctor::Option).collect())
        }
    }

    /// The types of the parts of a value of `ty` made with `ctor`.
    fn ctor_types(&self, ty: &Type, ctor: &Ctor) -> Vec<Type> {
        match ctor {
            Ctor::Int(_) | Ctor::Bool(_) => vec![],
            Ctor::Tuple => self.tuples[&ty.id]
                .iter()
                .map(|element| element.ty.clone())
                .collect(),
            // An option whose type is unknown has already been reported.
            Ctor::Option(index) => vec![
                self.choice_options[&ty.id][*index]
                    .ty
                    .clone()
                    .unwrap_or_else(|| self.std_type("Void")),
            ],
        }
    }

    /// How values of `ty` matching `space` are written as a pattern.
    fn show_space(&self, space: &Space, ty: &Type) -> String {
        let Space::Ctor(ctor, parts) = space else {
            return "_".to_string();
        };
        let part_tys = self.ctor_types(ty, ctor);
        match ctor {
            Ctor::Int(value) => value.to_string(),
            Ctor::Bool(value) => value.to_string(),
            Ctor::Tuple => {
                let elements: Vec<_> = self.tuples[&ty.id]
                    .iter()
                    .zip(parts.iter().zip(&part_tys))
                    .map(|(element, (part, part_ty))| {
                        let part = self.show_space(part, part_ty);
                        match &element.label {
                            Some(label) => format!("{label}: {part}"),
                            None => part,
                        }
                    })
                    .collect();
                format!("({})", elements.join(", "))
            }
            Ctor::Option(index) => {
                let option = self.choice_options[&ty.id][*index].name;
                match &parts[0] {
                    Space::Any => format!("{ty}.{option}"),
                    value => format!("{ty}.{option}({})", self.show_space(value, &part_tys[0])),
                }
            }
        }
    }

    /// Checks `expr`, where no particular type is expected.
    fn expr(&mut self, expr: &Expr) -> Typed {
        self.expr_as(expr, None)
    }

    /// Checks `expr`, returning its type, or `None` if it can't be determined
    /// because of an error that has already been reported, along with what it
    /// compiles to. `expected` is the type it's used as, if that's known,
    /// which integer literals take as their type. It doesn't have to match:
    /// whatever uses `expr` checks that.
    fn expr_as(&mut self, expr: &Expr, expected: Option<&Type>) -> Typed {
        let typed = self.expr_kind_as(expr, expected);
        if typed.ty == Some(self.std_type("Never")) {
            self.evaluated_never = true;
        }
        typed
    }

    /// Checks `expr` like `expr_as`, by what kind of expression it is.
    fn expr_kind_as(&mut self, expr: &Expr, expected: Option<&Type>) -> Typed {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Int(value) => self.int_literal(i128::from(*value), span, expected),
            // A negated literal is checked as one value, so the smallest
            // signed integers can be written.
            ExprKind::Unary(UnOp::Neg, operand) if let ExprKind::Int(value) = &operand.kind => {
                self.int_literal(-i128::from(*value), span, expected)
            }
            ExprKind::Bool(value) => Typed {
                ty: Some(self.std_type("Bool")),
                expr: Some(ir::Expr::Bool(*value)),
            },
            ExprKind::Var(name) => match self.lookup(name) {
                Ok(Binding::Value { ty, local, .. }) => Typed {
                    ty,
                    expr: Some(ir::Expr::Local(local)),
                },
                Ok(Binding::Global(index)) => Typed {
                    ty: self.global_type(index),
                    expr: Some(ir::Expr::Global(index)),
                },
                Ok(Binding::Function(index)) => self.function_value(index, span),
                Ok(Binding::Type(_) | Binding::Alias(_) | Binding::Generic(_)) => {
                    self.error(span, CheckError::TypeAsValue(name.to_string()));
                    Typed::UNKNOWN
                }
                Ok(Binding::Trait(_)) => {
                    self.error(span, CheckError::TraitAsValue(name.to_string()));
                    Typed::UNKNOWN
                }
                Err(error) => {
                    self.lookup_error(name, error);
                    Typed::UNKNOWN
                }
            },
            // Operators always produce their result type, even if an operand
            // is invalid, so one bad operand doesn't cause further errors.
            ExprKind::Unary(op @ (UnOp::Neg | UnOp::BitNot), operand) => {
                self.integer_unary(op, operand, expected)
            }
            ExprKind::Unary(UnOp::Not, operand) => self.not(operand),
            ExprKind::Binary(lhs, op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), rhs) => {
                self.comparison(lhs, op, rhs)
            }
            ExprKind::Binary(lhs, op @ (BinOp::And | BinOp::Or | BinOp::Eq | BinOp::Ne), rhs) => {
                self.trait_operator(lhs, op, rhs)
            }
            ExprKind::Binary(lhs, op @ (BinOp::ShiftLeft | BinOp::ShiftRight), rhs) => {
                self.shift(lhs, op, rhs, expected)
            }
            ExprKind::Binary(lhs, op, rhs) => self.arithmetic(lhs, op, rhs, expected),
            ExprKind::Call { callee, args } => self.call(callee, args, span),
            ExprKind::Construct { ty, fields } => self.construct(ty, span, fields, expected),
            ExprKind::Paren(value) => match expected.and_then(|ty| self.tuples.get(&ty.id)) {
                // A tuple with one unlabelled element is built from a value in
                // brackets, as in `(1)`.
                Some(elements) if matches!(&elements[..], [element] if element.label.is_none()) => {
                    let arg = Arg {
                        label: None,
                        value: (**value).clone(),
                        span,
                    };
                    self.tuple(&[arg], expected)
                }
                _ => self.expr_as(value, expected),
            },
            ExprKind::Tuple(values) => self.tuple(values, expected),
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => self.method_call(receiver, method, args, span, expected),
            ExprKind::Field { receiver, field } => self.field(receiver, field, span, expected),
            ExprKind::GenericType { name, .. } => {
                self.error(span, CheckError::TypeAsValue(name.to_string()));
                Typed::UNKNOWN
            }
            ExprKind::SelfValue => {
                let name = Name {
                    span,
                    ..Name::new("self")
                };
                match self.lookup(&name) {
                    Ok(Binding::Value { ty, local, .. }) => Typed {
                        ty,
                        expr: Some(ir::Expr::Local(local)),
                    },
                    Ok(_) => unreachable!("only methods declare `self`, as a value"),
                    Err(LookupError::Undeclared) => {
                        let in_static = self.current_fn.as_ref().is_some_and(|f| f.is_static);
                        self.error(
                            span,
                            if in_static {
                                CheckError::SelfInStatic
                            } else {
                                CheckError::SelfOutsideImpl
                            },
                        );
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

    /// The width and signedness of `ty`, if it's an integer type.
    fn int_type(&self, ty: &Type) -> Option<(u32, bool)> {
        match self.primitives.get(&ty.id) {
            Some(&ir::Primitive::Int { bits, signed }) => Some((bits, signed)),
            _ => None,
        }
    }

    /// The type of an integer that's used as `expected`, which is `expected`
    /// itself if it's an integer type, and `Int` otherwise.
    fn int_type_for(&self, expected: Option<&Type>) -> Type {
        expected
            .filter(|ty| self.int_type(ty).is_some())
            .cloned()
            .unwrap_or_else(|| self.std_type("Int"))
    }

    /// Checks an integer literal of `value`, which may be negated, used as
    /// `expected`. It must fit in its type.
    fn int_literal(&mut self, value: i128, span: Span, expected: Option<&Type>) -> Typed {
        let ty = self.int_type_for(expected);
        let (bits, signed) = self.int_type(&ty).expect("literals have integer types");
        let (min, max) = int_range(bits, signed);
        if !(min..=max).contains(&value) {
            self.error(
                span,
                CheckError::IntOutOfRange {
                    value,
                    ty: ty.clone(),
                    min,
                    max,
                },
            );
            return Typed {
                ty: Some(ty),
                expr: None,
            };
        }

        // Stored as its bits, so a negative value is in two's complement.
        let bits = (value as u128 & (u128::MAX >> (128 - bits))) as u64;
        Typed {
            expr: Some(ir::Expr::Int {
                value: bits,
                ty: ty.id,
            }),
            ty: Some(ty),
        }
    }

    /// Checks the operands of a binary operator, where `expected` is the
    /// type the operator is used as. Each operand is used as the other's
    /// type, so `x + 1` takes the type of `x`. An operand made only of
    /// integer literals has no type of its own, so it's checked second.
    fn operands(&mut self, lhs: &Expr, rhs: &Expr, expected: Option<&Type>) -> (Typed, Typed) {
        if has_own_type(lhs) || !has_own_type(rhs) {
            let lhs = self.expr_as(lhs, expected);
            let rhs = self.expr_as(rhs, lhs.ty.as_ref().or(expected));
            (lhs, rhs)
        } else {
            let rhs = self.expr_as(rhs, expected);
            let lhs = self.expr_as(lhs, rhs.ty.as_ref().or(expected));
            (lhs, rhs)
        }
    }

    /// Checks `-operand`, which negates a signed integer, or `.!operand`,
    /// which flips the bits of any integer. Both produce another integer of
    /// the same type.
    fn integer_unary(&mut self, op: &UnOp, operand: &Expr, expected: Option<&Type>) -> Typed {
        let span = operand.span;
        let Typed { ty, expr } = self.expr_as(operand, expected);
        let needs_signed = *op == UnOp::Neg;
        match ty {
            Some(ty)
                if self
                    .int_type(&ty)
                    .is_some_and(|(_, signed)| signed || !needs_signed) =>
            {
                Typed {
                    expr: expr.map(|operand| ir::Expr::Unary(op.clone(), Box::new(operand))),
                    ty: Some(ty),
                }
            }
            found => {
                if let Some(found) = found {
                    let op = op.symbol();
                    self.error(
                        span,
                        if needs_signed {
                            CheckError::NotSignedInteger { op, found }
                        } else {
                            CheckError::NotInteger { op, found }
                        },
                    );
                }
                Typed {
                    ty: Some(self.int_type_for(expected)),
                    expr: None,
                }
            }
        }
    }

    /// Checks `lhs op rhs`, where `op` is `+`, `-`, `*`, `/`, `.&`, `.|` or
    /// `.^`, whose operands are integers of the same type, which it produces. The first operand
    /// that's an integer decides the type.
    fn arithmetic(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr, expected: Option<&Type>) -> Typed {
        let spans = [lhs.span, rhs.span];
        let (lhs, rhs) = self.operands(lhs, rhs, expected);
        let ty = [&lhs.ty, &rhs.ty]
            .into_iter()
            .flatten()
            .find(|ty| self.int_type(ty).is_some())
            .cloned()
            .unwrap_or_else(|| self.int_type_for(expected));
        let op_symbol = op.symbol();
        for (found, span) in [&lhs.ty, &rhs.ty].into_iter().zip(spans) {
            let Some(found) = found else {
                continue;
            };
            if self.int_type(found).is_none() {
                let found = found.clone();
                self.error(
                    span,
                    CheckError::NotInteger {
                        op: op_symbol,
                        found,
                    },
                );
            } else if *found != ty {
                self.error(
                    span,
                    CheckError::InvalidOperand {
                        op: op_symbol,
                        expected: ty.clone(),
                        found: found.clone(),
                    },
                );
            }
        }

        let (_, signed) = self.int_type(&ty).expect("found as an integer type");
        let expr = lhs.expr.zip(rhs.expr).map(|(lhs, rhs)| ir::Expr::Binary {
            lhs: Box::new(lhs),
            op: op.clone(),
            rhs: Box::new(rhs),
            signed,
        });
        Typed { ty: Some(ty), expr }
    }

    /// Checks `!operand`, which calls `operand.not()` from the standard
    /// library's `Not` trait, which the operand's type must implement. The
    /// result is its implementation's `Output`.
    fn not(&mut self, operand: &Expr) -> Typed {
        let span = operand.span;
        let Typed { ty, expr } = self.expr(operand);
        let function = ty
            .as_ref()
            .and_then(|ty| self.operator_method("!", "Not", "not", ty, span));
        let expr = function
            .zip(expr)
            .map(|(function, operand)| ir::Expr::Call {
                function,
                args: vec![operand],
            });
        // The result's type comes from `not`, so it's unknown without it.
        Typed {
            ty: function.and_then(|function| self.functions[function].returns.clone()),
            expr,
        }
    }

    /// Finds `method` of `ty`, which the operator `op` calls, as an index
    /// into `functions`, as `trait_method` does, for the code at `span`.
    fn operator_method(
        &mut self,
        op: &'static str,
        trait_name: &'static str,
        method: &str,
        ty: &Type,
        span: Span,
    ) -> Option<usize> {
        let not_implemented = CheckError::NotImplemented {
            op,
            trait_name,
            found: ty.clone(),
        };
        self.trait_method(trait_name, method, ty, span, not_implemented)
    }

    /// Finds `method` of `ty`, as an index into `functions`, which the
    /// compiler calls for some syntax, such as an operator. `ty` must
    /// implement the standard library's trait called `trait_name`, or
    /// `not_implemented` is reported at `span`. Like calling the method
    /// directly, the module implementing it must be imported.
    fn trait_method(
        &mut self,
        trait_name: &str,
        method: &str,
        ty: &Type,
        span: Span,
        not_implemented: CheckError,
    ) -> Option<usize> {
        let trait_index = match self.modules[self.core].scope.get(trait_name) {
            Some(Binding::Trait(index)) => *index,
            _ => panic!("the standard library should declare `{trait_name}`"),
        };
        let found = match self.member(ty, method) {
            Some(Member::Method(found)) if self.implementations.contains(&(ty.id, trait_index)) => {
                found
            }
            _ => {
                self.error(span, not_implemented);
                return None;
            }
        };
        self.check_imported(ty, method, &found, span)
            .then_some(found.function)
    }

    /// Checks `lhs & rhs`, `lhs | rhs`, `lhs == rhs` or `lhs != rhs`, which
    /// call `lhs.and(rhs)`, `lhs.or(rhs)`, `lhs.isEqual(to: rhs)` or
    /// `lhs.isNotEqual(to: rhs)` from the standard library's `And`, `Or` or
    /// `Equal` trait, which the type of `lhs` must implement. `rhs` must be
    /// its implementation's `Rhs`, and the result is the method's.
    fn trait_operator(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Typed {
        let (trait_name, method) = match op {
            BinOp::And => ("And", "and"),
            BinOp::Or => ("Or", "or"),
            BinOp::Eq => ("Equal", "isEqual"),
            BinOp::Ne => ("Equal", "isNotEqual"),
            _ => unreachable!("only `&`, `|`, `==` and `!=` call trait functions"),
        };
        let op = op.symbol();
        let (lhs_span, rhs_span) = (lhs.span, rhs.span);
        // An operand made only of integer literals has no type of its own, so
        // in `0 == byte`, the `0` takes its type from `byte`.
        let (lhs, rhs_first) = if !has_own_type(lhs) && has_own_type(rhs) {
            let rhs = self.expr(rhs);
            (self.expr_as(lhs, rhs.ty.as_ref()), Some(rhs))
        } else {
            (self.expr(lhs), None)
        };
        let signature = lhs
            .ty
            .as_ref()
            .and_then(|ty| self.operator_method(op, trait_name, method, ty, lhs_span))
            .map(|function| (function, self.functions[function].clone()));

        // The right operand is the method's argument, so it's used as its
        // parameter's type, like any other argument.
        let param = signature
            .as_ref()
            .and_then(|(_, signature)| signature.params.first()?.ty.clone());
        let rhs = match rhs_first {
            Some(rhs) => rhs,
            None => self.expr_as(rhs, param.as_ref()),
        };
        let mut function = signature.as_ref().map(|&(function, _)| function);
        if let Some((param, found)) = param.zip(rhs.ty)
            && param != found
        {
            // Named as the left operand is when it's the same type, such as
            // `Int` rather than the `IntS64` its `impl` is for.
            let expected = lhs.ty.clone().filter(|ty| *ty == param).unwrap_or(param);
            self.error(
                rhs_span,
                CheckError::InvalidOperand {
                    op,
                    expected,
                    found,
                },
            );
            function = None;
        }

        let expr = function
            .zip(lhs.expr.zip(rhs.expr))
            .map(|(function, (lhs, rhs))| ir::Expr::Call {
                function,
                args: vec![lhs, rhs],
            });
        Typed {
            ty: signature.and_then(|(_, signature)| signature.returns),
            expr,
        }
    }

    /// Checks a tuple built from `values`, labelled like the elements of its
    /// type. If it has the labels of `expected`, its values take their types
    /// from its elements, and it's that type if they're the same. Otherwise,
    /// its elements are named after their labels, so `(x: 1)` is `(x: Int)`,
    /// and the elements without labels can't be read.
    fn tuple(&mut self, values: &[Arg], expected: Option<&Type>) -> Typed {
        let expected_elements = expected
            .and_then(|ty| self.tuples.get(&ty.id))
            .filter(|elements| {
                elements.len() == values.len()
                    && elements
                        .iter()
                        .zip(values)
                        .all(|(element, value)| element.label == value.label)
            })
            .cloned();
        let typed: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let element = expected_elements
                    .as_ref()
                    .map(|elements| &elements[index].ty);
                self.expr_as(&value.value, element)
            })
            .collect();
        let (found, lowered) = split(typed);

        // An element whose type is unknown has already been reported.
        let is_expected = expected_elements.is_some_and(|elements| {
            elements
                .iter()
                .zip(&found)
                .all(|(element, found)| found.as_ref().is_none_or(|found| *found == element.ty))
        });
        let ty = if is_expected {
            expected.cloned()
        } else {
            let elements = values.iter().zip(found).map(|(value, ty)| {
                Some(TupleElementType {
                    label: value.label.clone(),
                    name: value.label.clone(),
                    ty: ty?,
                })
            });
            let elements = elements.collect::<Option<_>>();
            elements.map(|elements| self.tuple_type(elements))
        };
        let expr = ty
            .as_ref()
            .zip(lowered)
            .map(|(ty, fields)| ir::Expr::Construct {
                group: ty.id,
                fields,
            });
        Typed { ty, expr }
    }

    /// Checks `lhs .< rhs` or `lhs .> rhs`, which shift the bits of an
    /// unsigned integer, producing another of the same type. The amount to
    /// shift by can be any integer type.
    fn shift(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr, expected: Option<&Type>) -> Typed {
        let op_symbol = op.symbol();
        let (lhs_span, rhs_span) = (lhs.span, rhs.span);
        let lhs = self.expr_as(lhs, expected);
        let rhs = self.expr(rhs);
        let ty = match lhs.ty {
            Some(ty) if self.int_type(&ty).is_some_and(|(_, signed)| !signed) => Some(ty),
            Some(found) => {
                self.error(
                    lhs_span,
                    CheckError::NotUnsignedInteger {
                        op: op_symbol,
                        found,
                    },
                );
                None
            }
            None => None,
        };
        let rhs_expr = match rhs.ty {
            Some(ty) if self.int_type(&ty).is_some() => rhs.expr,
            Some(found) => {
                self.error(
                    rhs_span,
                    CheckError::NotInteger {
                        op: op_symbol,
                        found,
                    },
                );
                None
            }
            None => None,
        };

        let expr = ty
            .as_ref()
            .and(lhs.expr.zip(rhs_expr))
            .map(|(lhs, rhs)| ir::Expr::Binary {
                lhs: Box::new(lhs),
                op: op.clone(),
                rhs: Box::new(rhs),
                signed: false,
            });
        Typed {
            ty: Some(ty.unwrap_or_else(|| self.int_type_for(expected))),
            expr,
        }
    }

    /// Checks `<`, `<=`, `>` or `>=`, which order two integers of the same
    /// type, producing a `Bool`. The first operand whose type is known
    /// decides what's compared.
    fn comparison(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Typed {
        let spans = [lhs.span, rhs.span];
        let (lhs, rhs) = self.operands(lhs, rhs, None);
        let bool_ty = self.std_type("Bool");
        let op_symbol = op.symbol();
        let compared = match lhs.ty.clone().or(rhs.ty.clone()) {
            Some(ty) if self.int_type(&ty).is_some() => Some(ty),
            Some(found) => {
                // The first operand whose type is known decided what's compared.
                let span = if lhs.ty.is_some() { spans[0] } else { spans[1] };
                self.error(
                    span,
                    CheckError::NotInteger {
                        op: op_symbol,
                        found,
                    },
                );
                None
            }
            None => None,
        };
        if let Some(compared) = &compared {
            for (found, span) in [&lhs.ty, &rhs.ty].into_iter().zip(spans) {
                if let Some(found) = found
                    && found != compared
                {
                    self.error(
                        span,
                        CheckError::InvalidOperand {
                            op: op_symbol,
                            expected: compared.clone(),
                            found: found.clone(),
                        },
                    );
                }
            }
        }

        let signed = compared
            .as_ref()
            .and_then(|ty| self.int_type(ty))
            .is_some_and(|(_, signed)| signed);
        let expr = compared
            .and(lhs.expr.zip(rhs.expr))
            .map(|(lhs, rhs)| ir::Expr::Binary {
                lhs: Box::new(lhs),
                op: op.clone(),
                rhs: Box::new(rhs),
                signed,
            });
        Typed {
            ty: Some(bool_ty),
            expr,
        }
    }

    /// Checks the receiver of a field access or method call, which is followed
    /// by `.member`. A type's name is the type itself, for its static
    /// functions. A module's name gets its own error, since `::` was probably
    /// meant.
    ///
    /// A generic type's name can leave out its arguments when the value is
    /// used as `expected`, if that's made from it, as in `Optional.none`.
    fn receiver(
        &mut self,
        receiver: &Expr,
        member: &str,
        expected: Option<&Type>,
    ) -> Option<Receiver> {
        if let ExprKind::GenericType { name, args } = &receiver.kind {
            let ty = self.generic_type(name, args, true)?;
            return Some(Receiver::Type(ty));
        }
        if let ExprKind::Var(name) = &receiver.kind {
            match self.lookup(name) {
                Ok(Binding::Type(ty)) => return Some(Receiver::Type(ty)),
                Ok(Binding::Generic(_)) => {
                    let type_name = TypeName::Named(name.clone());
                    let ty = self.type_in_context(&type_name, &[expected], true)?;
                    return Some(Receiver::Type(ty));
                }
                Err(LookupError::Undeclared)
                    if name.module.is_none()
                        && self.modules[self.module]
                            .qualifiers
                            .iter()
                            .any(|(qualifier, _)| *qualifier == name.name) =>
                {
                    self.error(
                        receiver.span,
                        CheckError::ModuleNotValue {
                            module: name.name.clone(),
                            member: member.to_string(),
                        },
                    );
                    return None;
                }
                _ => {}
            }
        }
        let Typed { ty, expr } = self.expr(receiver);
        Some(Receiver::Value(ty?, expr))
    }

    /// Finds the field or method of `ty` called `name`. A tuple's elements
    /// are its fields, found by their names.
    fn member(&self, ty: &Type, name: &str) -> Option<Member> {
        if let Some(elements) = self.tuples.get(&ty.id)
            && let Some(index) = elements
                .iter()
                .position(|element| element.name.as_deref() == Some(name))
        {
            return Some(Member::Field(index, Some(elements[index].ty.clone())));
        }
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

    /// The `Base` and `Value` of `ty`, if it's made from `ChoiceOption`.
    fn option_args(&self, ty: &Type) -> Option<(Type, Type)> {
        let (template, args) = self.instances.get(&ty.id)?;
        match &args[..] {
            [base, value] if *template == self.std_template("ChoiceOption") => {
                Some((base.clone(), value.clone()))
            }
            _ => None,
        }
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
    fn field(
        &mut self,
        receiver: &Expr,
        field: &str,
        span: Span,
        expected: Option<&Type>,
    ) -> Typed {
        // A generic type's name can leave out its arguments when one of its
        // options is used as `expected`, as in `Optional.none`.
        let base = expected.and_then(|expected| self.option_args(expected));
        let expected = base.as_ref().map(|(base, _)| base).or(expected);
        let Some(receiver) = self.receiver(receiver, field, expected) else {
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
            // Naming an option doesn't create the choice, but gives its
            // `ChoiceOption`.
            Receiver::Type(ty) if let Some((option, value_ty)) = self.option(&ty, field) => {
                let template = self.std_template("ChoiceOption");
                return Typed {
                    ty: value_ty
                        .and_then(|value| self.instance(template, vec![ty, value], span, true)),
                    expr: Some(ir::Expr::Tag(option)),
                };
            }
            Receiver::Type(ty) => match self.static_function(&ty, field) {
                Some(found) => {
                    self.check_imported(&ty, field, &found, span);
                    return self.function_value(found.function, span);
                }
                None => self.no_static_function(ty, field),
            },
        };
        self.error(span, error);
        Typed::UNKNOWN
    }

    /// Checks calling `callee`, of type `ty`, called `name`, with `args`, at
    /// `span`, which calls its `call` from the standard library's `Call` trait, as in
    /// `adder(a: 3)` for `adder.call(input: (a: 3))`. If `Input` is a tuple,
    /// the arguments are its elements, so a tuple is never unpacked into them.
    /// Otherwise, the only argument is the `Input`, as in `negate(3)`.
    fn call_value(
        &mut self,
        ty: Option<Type>,
        callee: Callee,
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        if let Some(option_ty) = ty.as_ref()
            && let Some((base, value_ty)) = self.option_args(option_ty)
            && let Callee::Value(value) = callee
        {
            return self.call_option(option_ty, value, (base, value_ty), name, args, span);
        }
        // Like a function's arguments, the input is checked first, so its
        // errors come before the call's, and it's used as `Input`, which is
        // found without reporting errors.
        let call_trait = match self.modules[self.core].scope.get("Call") {
            Some(Binding::Trait(index)) => *index,
            _ => panic!("the standard library should declare `Call`"),
        };
        let param = ty
            .as_ref()
            .filter(|ty| self.implementations.contains(&(ty.id, call_trait)))
            .and_then(|ty| match self.member(ty, "call") {
                Some(Member::Method(found)) => self.functions[found.function].params.first(),
                _ => None,
            })
            .and_then(|param| param.ty.clone());
        let input_is_tuple = param
            .as_ref()
            .is_some_and(|param| self.tuples.contains_key(&param.id));
        let input = match args {
            [
                Arg {
                    label: None, value, ..
                },
            ] if !input_is_tuple => value.clone(),
            _ => Expr {
                kind: ExprKind::Tuple(args.to_vec()),
                span,
            },
        };
        let Typed {
            ty: found,
            expr: input,
        } = self.expr_as(&input, param.as_ref());

        let function = ty.as_ref().and_then(|ty| {
            let not_callable = CheckError::NotCallable {
                name: name.to_string(),
                ty: ty.clone(),
            };
            self.trait_method("Call", "call", ty, span, not_callable)
        });
        let returns = function.and_then(|function| self.functions[function].returns.clone());
        if found != param {
            if let Callee::Function(index) = callee {
                self.check_function_args(index, name, args, span);
            } else if let (Some(expected), Some(found)) = (param, found) {
                self.error(
                    span,
                    CheckError::WrongCallInput {
                        name: name.to_string(),
                        expected,
                        found,
                    },
                );
            }
            return Typed {
                ty: returns,
                expr: None,
            };
        }
        let expr = function.zip(input).and_then(|(call, input)| match callee {
            Callee::Value(value) => value.map(|value| ir::Expr::Call {
                function: call,
                args: vec![value, input],
            }),
            Callee::Function(index) => Some(self.call_function(index, input)),
        });
        Typed { ty: returns, expr }
    }

    /// Checks calling `value`, whose type `ty` is a `ChoiceOption` with
    /// `base` and `value_ty`, called `name`, with `args`, at `span`.
    fn call_option(
        &mut self,
        ty: &Type,
        value: Option<ir::Expr>,
        (base, value_ty): (Type, Type),
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let not_callable = CheckError::NotCallable {
            name: name.to_string(),
            ty: ty.clone(),
        };
        let function = self.trait_method("Call", "call", ty, span, not_callable);
        let Typed { ty: returns, expr } =
            self.option_value(name.to_string(), base, Some(value_ty), args, span);
        Typed {
            ty: returns,
            expr: function
                .zip(value)
                .zip(expr)
                .map(|((function, value), input)| ir::Expr::Call {
                    function,
                    args: vec![value, input],
                }),
        }
    }

    /// Checks `args` as the value for an option of the choice `base`, called
    /// `name`, which holds a `value_ty`. Creating an option is like calling a
    /// function with one unlabelled parameter, so a tuple is always one
    /// argument, except that a `Void` can be left out. Returns the choice's
    /// type, along with what the value compiles to.
    fn option_value(
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
                expr: Some(self.void()),
            };
        }
        let (found, lowered) = self.args(args, std::slice::from_ref(&value_ty));
        self.checked_option_value(name, base, value_ty, args, found, lowered, span)
    }

    /// Checks `args`, which have the types `found` and compile to `lowered`,
    /// as the value for an option, as `option_value` does.
    #[allow(clippy::too_many_arguments)]
    fn checked_option_value(
        &mut self,
        name: String,
        base: Type,
        value_ty: Option<Type>,
        args: &[Arg],
        found: Vec<Option<Type>>,
        lowered: Option<Vec<ir::Expr>>,
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
        };
        Typed {
            ty: self.check_args(name, args, found, signature, span),
            expr: lowered
                .and_then(|args| <[_; 1]>::try_from(args).ok())
                .map(|[value]| value),
        }
    }

    /// The declared types of the parameters of the static function or option
    /// called `method` of the generic type at `template` in `templates`, if
    /// it has one.
    fn declared_params(&self, template: usize, method: &str) -> Option<DeclaredParams<'a>> {
        let Template { module, stmt, .. } = self.templates[template];
        if let StmtKind::Choice { options, .. } = &stmt.kind
            && let Some(option) = options.iter().find(|option| option.name == method)
        {
            return Some(DeclaredParams {
                module,
                types: vec![&option.ty],
                in_impl: false,
            });
        }
        self.templates[template]
            .impls
            .iter()
            .find_map(|&(module, stmt)| {
                let StmtKind::Impl { functions, .. } = &stmt.kind else {
                    return None;
                };
                let found = functions
                    .iter()
                    .find(|found| found.is_static && found.function.name == method)?;
                Some(DeclaredParams {
                    module,
                    types: found
                        .function
                        .params
                        .iter()
                        .map(|param| &param.ty)
                        .collect(),
                    in_impl: true,
                })
            })
    }

    /// Checks a call to the static function or option called `method` of the
    /// generic type called `name`, at `template` in `templates`, working out
    /// its arguments from the types of `args`, which are matched against
    /// `declared`. Each of `args` is checked once, in order, so once every
    /// argument of the type is known, the rest take their types from its
    /// parameters, as in any other call.
    fn inferred_call(
        &mut self,
        name: &Name,
        template: usize,
        method: &str,
        declared: &DeclaredParams<'a>,
        args: &[Arg],
        span: Span,
    ) -> Typed {
        let mut inferred = vec![None; self.templates[template].params.len()];
        let mut found = vec![];
        let mut lowered = vec![];
        for (index, arg) in args.iter().enumerate() {
            let expected = self.param_once_inferred(template, method, &inferred, name, index);
            let Typed { ty, expr } = self.expr_as(&arg.value, expected.as_ref());
            if let (Some(param), Some(ty)) = (declared.types.get(index), &ty) {
                self.infer_from(param, ty, template, declared, &mut inferred);
            }
            found.push(ty);
            lowered.push(expr);
        }
        let lowered: Option<Vec<_>> = lowered.into_iter().collect();

        let Some(inferred) = inferred.into_iter().collect() else {
            // An unknown argument has been reported, and may be why.
            if found.iter().all(Option::is_some) {
                self.error(name.span, CheckError::CannotInferGeneric(name.to_string()));
            }
            return Typed::UNKNOWN;
        };
        let Some(ty) = self.named_instance(template, inferred, name, true) else {
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
        if !self.check_imported(&ty, method, &function, span) {
            return Typed::UNKNOWN;
        }
        let signature = self.functions[function.function].clone();
        Typed {
            ty: self.check_args(full_name, args, found, signature, span),
            expr: lowered.map(|args| ir::Expr::Call {
                function: function.function,
                args,
            }),
        }
    }

    /// The type of the parameter at `index` of the static function or option
    /// called `method` of the generic type called `name`, once all of its
    /// arguments have been `inferred`.
    fn param_once_inferred(
        &mut self,
        template: usize,
        method: &str,
        inferred: &[Option<Type>],
        name: &Name,
        index: usize,
    ) -> Option<Type> {
        let args = inferred.iter().cloned().collect::<Option<Vec<_>>>()?;
        let ty = self.named_instance(template, args, name, false)?;
        if let Some((_, value_ty)) = self.option(&ty, method) {
            return value_ty.filter(|_| index == 0);
        }
        let function = self.static_function(&ty, method)?;
        self.functions[function.function]
            .params
            .get(index)?
            .ty
            .clone()
    }

    /// Works out which of the arguments of the generic type at `template` in
    /// `templates` are known from a value of type `found` being given to a
    /// parameter declared as `param`, adding them to `inferred`. The first
    /// type found for one is kept, so any others are reported as mismatches.
    fn infer_from(
        &mut self,
        param: &TypeName,
        found: &Type,
        template: usize,
        declared: &DeclaredParams<'a>,
        inferred: &mut [Option<Type>],
    ) {
        let params = self.templates[template].params;
        let position = |name: &str| params.iter().position(|param| param == name);
        let index = match param {
            TypeName::SelfParam(param, _) => position(param),
            TypeName::Named(name) if !declared.in_impl && name.module.is_none() => {
                position(&name.name)
            }
            TypeName::Named(name) if name.is_self() && self.is_made_from(found, template) => {
                let args = self.instances[&found.id].1.clone();
                for (inferred, arg) in inferred.iter_mut().zip(args) {
                    inferred.get_or_insert(arg);
                }
                None
            }
            TypeName::Named(_) => None,
            TypeName::Tuple(elements) => {
                let types = self.tuples.get(&found.id).cloned().unwrap_or_default();
                for (element, ty) in elements.iter().zip(&types) {
                    self.infer_from(&element.ty, &ty.ty, template, declared, inferred);
                }
                None
            }
            TypeName::Generic { name, args } => {
                let binding =
                    self.at_top_level(declared.module, None, |checker| checker.lookup(name));
                if let Ok(Binding::Generic(generic)) = binding
                    && self.is_made_from(found, generic)
                {
                    let found_args = self.instances[&found.id].1.clone();
                    for (arg, found_arg) in args.iter().zip(&found_args) {
                        self.infer_from(arg, found_arg, template, declared, inferred);
                    }
                }
                None
            }
        };
        if let Some(index) = index {
            inferred[index].get_or_insert_with(|| found.clone());
        }
    }

    /// Reports what's wrong with each of `args`, which don't make the `Input`
    /// of the function at `index`, called `name`, against its parameters. Each
    /// is checked again as its parameter, as it may not have had a type to use
    /// as part of the input, so errors in the arguments themselves, which have
    /// already been reported, are dropped.
    fn check_function_args(&mut self, index: usize, name: &str, args: &[Arg], span: Span) {
        let errors = self.errors.len();
        let params = self.param_types(index);
        let (found, _) = self.args(args, &params);
        self.errors.truncate(errors);
        let signature = self.functions[index].clone();
        self.check_args(name.to_string(), args, found, signature, span);
    }

    /// What calling the function at `index` with `input`, its `Input`,
    /// compiles to. It's the same as using `Fn.call`, but the function is called
    /// directly, rather than through a pointer. If `Input` is a tuple, it's
    /// always built in the call, with its elements as the arguments.
    fn call_function(&self, index: usize, input: ir::Expr) -> ir::Expr {
        let args = match input {
            ir::Expr::Construct { group, fields } if self.tuples.contains_key(&group) => fields,
            input => vec![input],
        };
        ir::Expr::Call {
            function: index,
            args,
        }
    }

    /// Reports an error unless `found`, the method or static function of `ty`
    /// called `member`, can be used here. Like functions, they can only be used
    /// from their own module or one that imports it. Returns whether it can.
    fn check_imported(&mut self, ty: &Type, member: &str, found: &Method, span: Span) -> bool {
        let imported = self.visible_modules().any(|module| module == found.module);
        if !imported {
            self.error(
                span,
                CheckError::MemberNotImported {
                    ty: ty.clone(),
                    member: member.to_string(),
                    module: self.modules[found.module].display.clone(),
                },
            );
        }
        imported
    }

    /// Checks a call to a method on a value, a static function on a type, or
    /// the creation of an option of a choice, returning its return type, or
    /// `None` if it can't be determined, along with what it compiles to.
    fn method_call(
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
        // The receiver and arguments are checked first, so their errors come
        // before the call's. What's called is found in between, without
        // reporting errors, so the arguments can take their types from its
        // parameters.
        let receiver = self.receiver(receiver, method, expected);
        // A field followed by arguments is called, like any other value.
        let receiver = match receiver {
            Some(Receiver::Value(ty, lowered)) => match self.member(&ty, method) {
                Some(Member::Field(index, field_ty)) => {
                    let value = lowered.map(|receiver| ir::Expr::Field {
                        receiver: Box::new(receiver),
                        index,
                    });
                    return self.call_value(field_ty, Callee::Value(value), method, args, span);
                }
                _ => Some(Receiver::Value(ty, lowered)),
            },
            // So is a static function, like any other function.
            Some(Receiver::Type(ty)) if let Some(found) = self.static_function(&ty, method) => {
                self.check_imported(&ty, method, &found, span);
                let fn_type = self.function_type(found.function, span);
                let name = format!("{ty}.{method}");
                let callee = Callee::Function(found.function);
                return self.call_value(fn_type, callee, &name, args, span);
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
        let params = match &receiver {
            Some(Receiver::Value(ty, _)) => match self.member(ty, method) {
                Some(Member::Method(found)) => self.param_types(found.function),
                _ => vec![],
            },
            Some(Receiver::Type(_)) | None => vec![],
        };
        let (found, lowered_args) = self.args(args, &params);

        let Some(receiver) = receiver else {
            return Typed::UNKNOWN;
        };
        let (ty, found_method, lowered_receiver) = match receiver {
            Receiver::Value(ty, lowered_receiver) => match self.member(&ty, method) {
                Some(Member::Method(found_method)) => (ty, found_method, lowered_receiver),
                Some(Member::Field(..)) => unreachable!("fields were handled above"),
                None => {
                    let error = self.no_member(ty, method);
                    self.error(span, error);
                    return Typed::UNKNOWN;
                }
            },
            Receiver::Type(ty) => {
                let error = self.no_static_function(ty, method);
                self.error(span, error);
                return Typed::UNKNOWN;
            }
        };
        if !self.check_imported(&ty, method, &found_method, span) {
            return Typed::UNKNOWN;
        }

        let signature = self.functions[found_method.function].clone();
        let returns = self.check_args(format!("{ty}.{method}"), args, found, signature, span);

        // A method's receiver is its first argument.
        let lowered_args = lowered_receiver
            .zip(lowered_args)
            .map(|(receiver, args)| iter::once(receiver).chain(args).collect());
        Typed {
            ty: returns,
            expr: lowered_args.map(|args| ir::Expr::Call {
                function: found_method.function,
                args,
            }),
        }
    }

    /// Checks the construction of a group, returning its type, or `None` if
    /// it can't be determined, along with what it compiles to.
    ///
    /// A generic group's name can leave out its arguments when the group is
    /// used as `expected`, or is the `impl`'s type, if that's made from it.
    fn construct(
        &mut self,
        type_name: &TypeName,
        span: Span,
        values: &[FieldValue],
        expected: Option<&Type>,
    ) -> Typed {
        let impl_type = self.impl_type.clone().flatten();
        let contexts = [expected, impl_type.as_ref()];
        // Values are checked first, so their errors come before the group's,
        // but the group is found first, without reporting errors, so they
        // can take their types from its fields.
        let fields = match self.type_in_context(type_name, &contexts, false) {
            Some(ty) => self.group_fields[ty.id].clone(),
            None => vec![],
        };
        let typed = values.iter().enumerate().map(|(index, value)| {
            let expected = fields
                .get(index)
                .filter(|field| field.name == value.name)
                .and_then(|field| field.ty.as_ref());
            self.expr_as(&value.value, expected)
        });
        let (found, lowered) = split(typed.collect());

        let Some(ty) = self.type_in_context(type_name, &contexts, true) else {
            return Typed::UNKNOWN;
        };
        let with_error = |ty| Typed {
            ty: Some(ty),
            expr: None,
        };
        if self.choice_options.contains_key(&ty.id) {
            self.error(span, CheckError::ConstructedChoice(ty.clone()));
            return with_error(ty);
        }
        if self.primitives.contains_key(&ty.id) {
            self.error(span, CheckError::ConstructedPrimitive(ty.clone()));
            return with_error(ty);
        }
        // Only the type's own module can construct it, and only in the type's
        // `impl`s. Elsewhere, values come from its functions, such as
        // `Void.new()`. An unknown `impl` type isn't reported again.
        let declared_in = self.group_modules[ty.id];
        if declared_in != self.module {
            self.error(
                span,
                CheckError::ConstructedOutsideModule {
                    ty: ty.clone(),
                    module: self.modules[declared_in].display.clone(),
                },
            );
            return with_error(ty);
        }
        match &self.impl_type {
            Some(Some(impl_type)) if *impl_type == ty => {}
            Some(None) => {}
            _ => {
                self.error(span, CheckError::ConstructedOutsideImpl(ty.clone()));
                return with_error(ty);
            }
        }
        let fields = self.group_fields[ty.id].clone();
        let name = type_name.to_string();

        // Every field must be given, in the order the group declares them.
        if values.len() != fields.len() {
            self.error(
                span,
                CheckError::WrongFieldCount {
                    ty: name,
                    expected: fields.len(),
                    found: values.len(),
                },
            );
            return with_error(ty);
        }

        for ((value, found), field) in values.iter().zip(found).zip(&fields) {
            if value.name != field.name {
                self.error(
                    value.span,
                    CheckError::WrongField {
                        ty: name.clone(),
                        expected: field.name.to_string(),
                        found: value.name.clone(),
                    },
                );
            } else if let (Some(expected), Some(found)) = (&field.ty, found)
                && *expected != found
            {
                self.error(
                    value.value.span,
                    CheckError::FieldTypeMismatch {
                        ty: name.clone(),
                        field: field.name.to_string(),
                        expected: expected.clone(),
                        found,
                    },
                );
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

    /// Checks a call, returning what's called's return type, or `None` if it
    /// can't be determined, along with what it compiles to. Functions and other
    /// values are called alike, through the standard library's `Call` trait.
    fn call(&mut self, callee: &Name, args: &[Arg], span: Span) -> Typed {
        let name = callee.to_string();
        match self.lookup(callee) {
            Ok(Binding::Function(index)) => {
                let ty = self.function_type(index, span);
                self.call_value(ty, Callee::Function(index), &name, args, span)
            }
            Ok(Binding::Value { .. } | Binding::Global(_)) => {
                let Typed { ty, expr } = self.expr(&Expr {
                    kind: ExprKind::Var(callee.clone()),
                    span: callee.span,
                });
                self.call_value(ty, Callee::Value(expr), &name, args, span)
            }
            // Arguments are checked first, so their errors come before the
            // call's.
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

    /// The type `Fn<Input, Output>` of the function at `index` in `functions`,
    /// or `None` if it's unknown. Its `Input` is the type of its only parameter
    /// if that has no label and isn't a tuple, and otherwise a tuple of its
    /// parameters, which have only their labels, since their names are private
    /// to the function. So a tuple `Input` always holds the arguments. It's used
    /// by the code at `span`.
    fn function_type(&mut self, index: usize, span: Span) -> Option<Type> {
        let Signature {
            params, returns, ..
        } = self.functions[index].clone();
        let input = match &params[..] {
            [
                ParamSignature {
                    label: None,
                    ty: Some(ty),
                    ..
                },
            ] if !self.tuples.contains_key(&ty.id) => ty.clone(),
            params => {
                let elements = params.iter().map(|param| {
                    Some(TupleElementType {
                        label: param.label.map(str::to_string),
                        name: param.label.map(str::to_string),
                        ty: param.ty.clone()?,
                    })
                });
                let elements = elements.collect::<Option<_>>()?;
                self.tuple_type(elements)
            }
        };
        let template = self.std_template("Fn");
        self.instance(template, vec![input, returns?], span, true)
    }

    /// The function at `index` in `functions` as a value, which compiles to a
    /// pointer to a function taking its `Input` as a single parameter, so that
    /// every `Fn` can be called in the same way.
    fn function_value(&mut self, index: usize, span: Span) -> Typed {
        let Some(ty) = self.function_type(index, span) else {
            return Typed::UNKNOWN;
        };
        let value = match self.function_values.get(&index) {
            Some(&value) => value,
            None => self.single_input_function(index, &ty),
        };
        Typed {
            ty: Some(ty),
            expr: Some(ir::Expr::Function(value)),
        }
    }

    /// Makes a function that calls the function at `index` in `functions`,
    /// whose type is `ty`, with its `Input` as a single parameter, returning
    /// its index in `functions`.
    fn single_input_function(&mut self, index: usize, ty: &Type) -> usize {
        let input = self.instances[&ty.id].1[0].id;
        let Signature {
            name,
            params,
            returns,
        } = self.functions[index].clone();
        let returns = returns.expect("the function's type is known");
        let args = if self.tuples.contains_key(&input) {
            (0..params.len())
                .map(|index| ir::Expr::Field {
                    receiver: Box::new(ir::Expr::Local(0)),
                    index,
                })
                .collect()
        } else {
            vec![ir::Expr::Local(0)]
        };
        let value = self.functions.len();
        let value_name = format!("{name}.value");
        self.functions.push(Signature {
            name: value_name.clone(),
            params: vec![],
            returns: Some(returns.clone()),
        });
        self.lowered_functions.push(Some(ir::Function {
            name: value_name,
            locals: vec![input],
            params: 1,
            returns: returns.id,
            body: ir::Body::Stmts(vec![ir::Stmt::Return(ir::Expr::Call {
                function: index,
                args,
            })]),
        }));
        self.function_values.insert(index, value);
        value
    }

    /// The types of the parameters of the function at `index` in `functions`.
    fn param_types(&self, index: usize) -> Vec<Option<Type>> {
        let params = &self.functions[index].params;
        params.iter().map(|param| param.ty.clone()).collect()
    }

    /// Checks a call's arguments, where each is used as the parameter at the
    /// same position in `params`, returning their types, and what they
    /// compile to if none of them have errors.
    fn args(
        &mut self,
        args: &[Arg],
        params: &[Option<Type>],
    ) -> (Vec<Option<Type>>, Option<Vec<ir::Expr>>) {
        let typed = args.iter().enumerate().map(|(index, arg)| {
            let expected = params.get(index).and_then(Option::as_ref);
            self.expr_as(&arg.value, expected)
        });
        split(typed.collect())
    }

    /// Checks a call's arguments, whose types are `found`, against the
    /// signature of `function`, returning its return type.
    fn check_args(
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

    /// Reports an error if a value of type `found`, at `span`, can't be given to
    /// `name`, whose type is `expected`. Unknown types are assumed to be fine.
    fn check_assignable(
        &mut self,
        name: &str,
        span: Span,
        expected: Option<Type>,
        found: Option<Type>,
    ) {
        if let (Some(expected), Some(found)) = (expected, found)
            && expected != found
        {
            self.error(
                span,
                CheckError::AssignTypeMismatch {
                    name: name.to_string(),
                    expected,
                    found,
                },
            );
        }
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
                } else if let Some(elements) = self.tuples.get(&id) {
                    // A tuple is laid out like a group of its elements.
                    let fields = elements.iter().map(|element| element.ty.id);
                    ir::TypeDef::Group {
                        name,
                        fields: fields.collect(),
                    }
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
                    LazyType::Checked(Some(ty)) => ty.id,
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

/// The names in the tuple type `ty` that are `Self` or one of `aliases`,
/// which stand for a different type in each `impl` of a trait.
fn trait_names<'b>(
    ty: &'b TypeName,
    aliases: &'b [String],
) -> Box<dyn Iterator<Item = &'b Name> + 'b> {
    match ty {
        TypeName::Named(name) if name.module.is_none() => {
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
fn signature_text(function: &TraitFunction) -> String {
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

/// The smallest and largest values of an integer type `bits` wide.
fn int_range(bits: u32, signed: bool) -> (i128, i128) {
    if signed {
        (-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
    } else {
        (0, (1 << bits) - 1)
    }
}

/// Whether `expr` has a type of its own, rather than being made only of
/// integer literals, which take their type from where they're used.
fn has_own_type(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Int(_) => false,
        ExprKind::Unary(UnOp::Neg | UnOp::BitNot, operand) | ExprKind::Paren(operand) => {
            has_own_type(operand)
        }
        ExprKind::Binary(
            lhs,
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor,
            rhs,
        ) => has_own_type(lhs) || has_own_type(rhs),
        // A shift has the type of what's shifted, whatever it's shifted by.
        ExprKind::Binary(lhs, BinOp::ShiftLeft | BinOp::ShiftRight, _) => has_own_type(lhs),
        _ => true,
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
        let result = check(&modules(files)).map(|_| ());
        // Every error must point at something in the file it's reported in.
        for err in result.as_ref().err().into_iter().flatten() {
            let found = pointed_at(files, err);
            assert!(!found.is_empty(), "`{err}` points at nothing: {err:?}");
        }
        result
    }

    /// The source that `err`, from checking `files`, points at.
    fn pointed_at<'a>(files: &[(&'a str, &'a str)], err: &ModuleError) -> &'a str {
        let src = files
            .iter()
            .find(|(path, _)| *path == err.module)
            .map(|(_, src)| *src)
            .or_else(|| stdlib_source(&err.module))
            .unwrap_or_else(|| panic!("`{}` should be a module", err.module));
        src.get(err.span.start..err.span.end)
            .unwrap_or_else(|| panic!("`{err}` points outside its module: {err:?}"))
    }

    /// The source that each error from checking `src` as `/main` points at.
    fn pointed_at_top(src: &str) -> Vec<&str> {
        let files = [("/main", src)];
        check_files(&files)
            .expect_err("`src` should have errors")
            .iter()
            .map(|err| pointed_at(&files, err))
            .collect()
    }

    /// The source that each error from checking `src` as the body of a function
    /// points at.
    fn pointed_at_body(src: &str) -> Vec<String> {
        let src = format!("fn body() {{\n{src}\n}}");
        pointed_at_top(&src)
            .into_iter()
            .map(str::to_string)
            .collect()
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

    /// The error for an arithmetic operator `op` used on a `Bool`.
    fn not_integer(op: &'static str) -> CheckError {
        CheckError::NotInteger {
            op,
            found: ty("Bool"),
        }
    }

    /// The error for `-` used on a `Bool`.
    fn not_signed_integer() -> CheckError {
        CheckError::NotSignedInteger {
            op: "-",
            found: ty("Bool"),
        }
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

        assert_eq!(check_body("let x = true & 1"), Err(vec![invalid("&")]));
        assert_eq!(
            messages("let x = !1"),
            ["operator `!` expects a type that implements `Not`, found `Int`"]
        );
        // `Int` doesn't implement `Or`, so `|` can't be used on it.
        assert_eq!(
            check_body("let x = 1 | 2"),
            Err(vec![CheckError::NotImplemented {
                op: "|",
                trait_name: "Or",
                found: ty("Int"),
            }])
        );
        assert_eq!(
            messages("let x = 1 & true"),
            ["operator `&` expects a type that implements `And`, found `Int`"]
        );
    }

    #[test]
    fn boolean_operators_call_and_and_or() {
        // `Bool` implements `And` and `Or` in the standard library, so they
        // can also be called directly.
        let src = "
            let a: Bool = true.and(false)
            let b: Bool = true.or(false) & a | !a
            ";
        assert_eq!(check_body(src), Ok(()));
    }

    #[test]
    fn not_calls_not_and_keeps_its_type() {
        let src = "
            group Flags {
                let bits: Int
            }

            impl Flags with Not {
                alias Output = Self

                fn not() -> Self {
                    return Flags { bits: .!self.bits }
                }
            }

            fn f(flags: Flags) -> Flags {
                let a: Bool = true.not() & !!false
                let b: Flags = !flags
                return !!b
            }
            ";
        assert_eq!(check_top(src), Ok(()));
        assert_eq!(
            messages_top(
                "
                group Flags {
                    let bits: Int
                }
                fn f(flags: Flags) -> Bool {
                    let a: Bool = !flags
                    return !Void.new()
                }
                "
            ),
            [
                "operator `!` expects a type that implements `Not`, found `Flags`",
                "operator `!` expects a type that implements `Not`, found `Void`",
            ]
        );
    }

    #[test]
    fn any_type_implementing_and_or_or_can_use_their_operators() {
        let src = "
            group Flags {
                let bits: Int
            }

            impl Flags with And {
                alias Rhs = Self
                alias Output = Bool

                fn and(_ rhs: Self) -> Bool {
                    return self.bits .& rhs.bits != 0
                }
            }

            impl Flags with Or {
                alias Rhs = Self
                alias Output = Bool

                fn or(_ rhs: Self) -> Bool {
                    return self.bits .| rhs.bits != 0
                }
            }

            fn f(a: Flags, b: Flags) -> Bool {
                // Each side is a `Bool`, which also implements them.
                return a & b | (b | a)
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn boolean_operators_need_their_trait_and_the_same_type() {
        assert_eq!(
            messages_top(
                "
                group Flags {
                    let bits: Int
                }

                // An `and` method alone isn't enough.
                impl Flags {
                    fn and(_ rhs: Self) -> Bool {
                        return true
                    }
                }

                impl Flags with Or {
                    alias Rhs = Self
                    alias Output = Bool

                    fn or(_ rhs: Self) -> Bool {
                        return true
                    }
                }

                fn f(a: Flags) -> Bool {
                    let b = a & a
                    let c = a | true
                    return true | a
                }
                "
            ),
            [
                "operator `&` expects a type that implements `And`, found `Flags`",
                "operator `|` expects `Flags`, found `Bool`",
                "operator `|` expects `Bool`, found `Flags`",
            ]
        );
    }

    #[test]
    fn boolean_operators_need_their_implementation_imported() {
        let flags = "
            group Flags {
                let bits: Int
            }
            ";
        let and = "
            use /flags
            impl Flags with And {
                alias Rhs = Self
                alias Output = Bool

                fn and(_ rhs: Flags) -> Bool {
                    return true
                }
            }
            ";
        assert_eq!(
            file_messages(&[
                ("/flags", flags),
                ("/and", and),
                (
                    "/main",
                    "use /flags\nfn f(a: Flags) -> Bool {\nreturn a & a\n}"
                ),
                (
                    "/other",
                    "use /flags\nuse /and\nfn f(a: Flags) -> Bool {\nreturn a & a\n}"
                ),
            ]),
            ["/main: `Flags.and` is declared in `/and`, which is not imported"]
        );
    }

    #[test]
    fn only_the_stdlib_can_declare_primitive_methods() {
        assert_eq!(
            messages_top(
                "
                trait Flip {
                    fn flip() -> Self
                }
                impl Bool with Flip {
                    primitive fn flip() -> Self
                }
                "
            ),
            ["`Bool.flip` can't be a primitive, since only the standard library can declare them"]
        );
    }

    #[test]
    fn boolean_operators_produce_booleans() {
        assert_eq!(
            check_body("let x = (true | false) + 1"),
            Err(vec![not_integer("+")])
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
        // `-` needs a signed integer and `!` needs a `Bool`, so `-!x` and
        // `!-x` can never be valid.
        assert_eq!(
            check_body("let x = -!true"),
            Err(vec![not_signed_integer()])
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
            Err(vec![not_integer("+")])
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
        // `Void` doesn't implement `Equal`.
        let incomparable = |op| CheckError::NotImplemented {
            op,
            trait_name: "Equal",
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
            vec!["operator `==` expects a type that implements `Equal`, found `Void`"]
        );
    }

    #[test]
    fn accepts_ordering_integers() {
        let src = "
            let a: Bool = 1 < 2
            let b: Bool = 1 + 2 <= 3
            let c: Bool = 4 > 5 * 6
            let d: Bool = -7 >= 8 & a
            ";
        assert_eq!(check_body(src), Ok(()));
    }

    #[test]
    fn rejects_ordering_other_types() {
        assert_eq!(
            check_body("let x = true < false"),
            Err(vec![not_integer("<")])
        );
        assert_eq!(
            check_body("let x = 1 <= true"),
            Err(vec![invalid_operand("<=")])
        );
        assert_eq!(
            check_body("let x = Void.new() > Void.new()"),
            Err(vec![CheckError::NotInteger {
                op: ">",
                found: ty("Void"),
            }])
        );
        assert_eq!(
            messages("let x = true >= false"),
            vec!["operator `>=` expects an integer, found `Bool`"]
        );
    }

    #[test]
    fn accepts_bitwise_operators_on_integers() {
        let src = "
            fn f(byte: IntU8, small: IntS8, amount: IntS64) {
                let a: IntU8 = byte .& 15 .| 1 .^ byte
                let b: IntS8 = .!small .& -1
                let c: Int = .!1 .^ 2
                let d: IntU8 = byte .< amount .> 1
                let e: IntU8 = 1 .< small
                let f: IntU8 = byte + (1 .< 3)
                let g: Bool = 3 .& 1 == 1
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn rejects_bitwise_operators_on_booleans() {
        assert_eq!(
            check_body("let x = true .& 1"),
            Err(vec![not_integer(".&")])
        );
        assert_eq!(
            check_body("let x = 1 .| false"),
            Err(vec![not_integer(".|")])
        );
        assert_eq!(
            check_body("let x = true .^ true"),
            Err(vec![not_integer(".^"), not_integer(".^")])
        );
        assert_eq!(check_body("let x = .!true"), Err(vec![not_integer(".!")]));
        assert_eq!(
            messages("let x = .!false"),
            vec!["operator `.!` expects an integer, found `Bool`"]
        );
    }

    #[test]
    fn only_unsigned_integers_can_be_shifted() {
        assert_eq!(
            messages_top(
                "
                fn f(byte: IntU8, small: IntS8) {
                    let a = small .< 1
                    let b = 1 .> 2
                    let c = true .< 1
                    let d = byte .> true
                    let e: IntU8 = small .< byte
                }
                "
            ),
            [
                "operator `.<` expects an unsigned integer, found `IntS8`",
                "operator `.>` expects an unsigned integer, found `Int`",
                "operator `.<` expects an unsigned integer, found `Bool`",
                "operator `.>` expects an integer, found `Bool`",
                "operator `.<` expects an unsigned integer, found `IntS8`",
            ]
        );
    }

    #[test]
    fn shifts_have_the_type_of_what_is_shifted() {
        assert_eq!(
            messages_top(
                "
                fn f(byte: IntU8, amount: IntU64) {
                    let a: IntU64 = byte .< amount
                }
                "
            ),
            ["cannot assign `IntU8` to `a`, which has type `IntU64`"]
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
                CheckError::NotImplemented {
                    op: "==",
                    trait_name: "Equal",
                    found: ty("Void"),
                },
                CheckError::Undeclared("y".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_arithmetic_on_booleans() {
        assert_eq!(check_body("let x = true + 1"), Err(vec![not_integer("+")]));
        assert_eq!(check_body("let x = 1 - false"), Err(vec![not_integer("-")]));
        assert_eq!(check_body("let x = -true"), Err(vec![not_signed_integer()]));
        assert_eq!(
            check_body("let x = 4 / (true)"),
            Err(vec![not_integer("/")])
        );
        assert_eq!(
            check_body("let b = true\nlet x = b * b"),
            Err(vec![not_integer("*"), not_integer("*")])
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
            Err(vec![not_signed_integer()])
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
            ["operator `+` expects an integer, found `Bool`"]
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
                CheckError::NotCallable {
                    name: "y".to_string(),
                    ty: ty("Int"),
                },
            ])
        );
    }

    #[test]
    fn rejects_assigning_to_functions() {
        assert_eq!(
            messages("fn f() {}\nf = 2"),
            ["cannot assign to `f`, since it's a function"]
        );
    }

    #[test]
    fn rejects_calling_undeclared_or_non_functions() {
        assert_eq!(
            messages("let x = 1\nx()\ng()"),
            [
                "`x` is `Int`, which doesn't implement `Call`, so it can't be called",
                "`g` is not declared",
            ]
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
                "operator `+` expects an integer, found `Void`",
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
                "operator `+` expects an integer, found `Bool`",
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
    fn calls_that_never_return_end_their_path() {
        // `exit` returns `Never`, and so does `fail`, which calls it.
        let src = "
            fn fail() -> Never {
                exit()
            }
            fn f() -> Int {
                exit()
            }
            fn g(c: Bool) -> Int {
                if c {
                    return 1
                } else {
                    fail()
                }
            }
            fn h(c: Bool) {
                if c {
                    exit()
                }
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn evaluating_never_anywhere_in_a_statement_ends_its_path() {
        // `describe` and `is_done` take a `Never`, so their arguments never
        // finish being evaluated, and they're never called.
        let src = "
            choice Answer {
                yes: Void
                no: Void
            }
            fn describe(_ never: Never) -> Int {
                match never {}
            }
            fn is_done(_ never: Never) -> Bool {
                match never {}
            }
            fn answer(_ never: Never) -> Answer {
                match never {}
            }

            fn declares() -> Int {
                let stopped = exit()
            }
            fn declares_with_a_type() {
                var stopped: Never = exit()
            }
            fn assigns() -> Int {
                var total = 0
                total = describe(exit())
            }
            fn returns() -> Never {
                return exit()
            }
            fn calls_with() -> Int {
                let total = 1 + describe(exit())
            }
            fn conditions() -> Int {
                if is_done(exit()) {}
            }
            fn matches() -> Int {
                match answer(exit()) {
                    Answer.yes => {}
                    Answer.no => {}
                }
            }
            fn uses_a_never(never: Never) -> Int {
                let stopped = never
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn calls_that_never_return_only_end_their_own_path() {
        let missing = |function: &str| CheckError::MissingReturn {
            function: function.to_string(),
            expected: ty("Int"),
        };
        assert_eq!(
            check_top(
                "
                fn f(c: Bool) -> Int {
                    if c {
                        exit()
                    }
                }
                fn g() -> Int {
                    fn nested() -> Never {
                        exit()
                    }
                }
                fn h() -> Int {
                    Void.new()
                }
                fn i(c: Bool) -> Int {
                    if c {
                        let stopped = exit()
                    }
                }
                "
            ),
            Err(vec![missing("f"), missing("g"), missing("h"), missing("i")])
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
                "operator `+` expects an integer, found `User`",
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
                ("/main", "let a = x\nlet b = lib::x"),
                ("/lib", "let x = 1"),
            ]),
            [
                "/main: `x` is not declared",
                "/main: `lib` is not an imported module",
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
    fn modules_with_the_same_name_need_aliases() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "
                    use /a/other
                    use /b/other
                    let x = other::value
                    ",
                ),
                ("/a/other", "let value = 1"),
                ("/b/other", "let value = true"),
            ]),
            [
                "/main: `other` could be `/a/other` or `/b/other`, so use `as` to give them different names"
            ]
        );
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /a/other as first
                    use /b/other as second
                    let x: Int = first::value
                    let y: Bool = second::value
                    ",
                ),
                ("/a/other", "let value = 1"),
                ("/b/other", "let value = true"),
            ]),
            Ok(())
        );
        // Aliasing one is enough to leave the name for the other.
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /a/other as first
                    use /b/other
                    let x: Int = first::value
                    let y: Bool = other::value
                    ",
                ),
                ("/a/other", "let value = 1"),
                ("/b/other", "let value = true"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn top_level_values_can_be_qualified_by_module_name_or_alias() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use ./filea
                    use ./fileb

                    let x: Int = filea::temp
                    let y: Bool = fileb::temp
                    ",
                ),
                ("/filea", "let temp = 1"),
                ("/fileb", "let temp = true"),
            ]),
            Ok(())
        );
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use ./filea as aliasa
                    use ./dir/filea as aliasb

                    let x: Int = aliasa::temp
                    let y: Bool = aliasb::temp
                    ",
                ),
                ("/filea", "let temp = 1"),
                ("/dir/filea", "let temp = true"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn aliases_replace_the_names_of_modules() {
        assert_eq!(
            file_messages(&[
                (
                    "/main",
                    "use /lib as renamed\nlet x = renamed::y\nlet z = lib::y"
                ),
                ("/lib", "let y = 1"),
            ]),
            ["/main: `lib` is not an imported module"]
        );
    }

    #[test]
    fn aliased_modules_are_still_imported() {
        assert_eq!(
            check_files(&[
                ("/main", "use /lib as renamed\nlet a: Int = y"),
                ("/lib", "let y = 1"),
            ]),
            Ok(())
        );
        assert_eq!(
            file_messages(&[
                ("/main", "use /a as x\nuse /b as y\nlet z = value"),
                ("/a", "let value = 1"),
                ("/b", "let value = 2"),
            ]),
            ["/main: `value` is declared in `/a` and `/b`, so it must be qualified"]
        );
    }

    #[test]
    fn aliases_qualify_types_and_functions() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use ./users/user as person
                    alias Person = person::User
                    let a: person::User = person::make()
                    let b: Person = person::User.new()
                    let c = a.age + b.age
                    ",
                ),
                (
                    "/users/user",
                    "
                    group User {
                        let age: Int
                    }
                    impl User {
                        static fn new() -> User {
                            return User { age: 1 }
                        }
                    }
                    fn make() -> User {
                        return User.new()
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn aliases_are_only_visible_in_their_own_module() {
        assert_eq!(
            file_messages(&[
                ("/main", "use /a\nlet x = renamed::y"),
                ("/a", "use /lib as renamed"),
                ("/lib", "let y = 1"),
            ]),
            ["/main: `renamed` is not an imported module"]
        );
    }

    #[test]
    fn modules_can_be_imported_under_several_names() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /lib
                    use /lib as x
                    use /lib as y
                    let a = lib::z + x::z + y::z
                    let b = z
                    ",
                ),
                ("/lib", "let z = 1"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn modules_can_alias_themselves() {
        assert_eq!(
            check_files(&[(
                "/main",
                "use /main as me\nlet a = 1\nlet b = me::a + main::a"
            )]),
            Ok(())
        );
    }

    #[test]
    fn rejects_the_same_alias_for_two_modules() {
        // Even if it's never used, and only once if it is.
        for main in [
            "use /a as x\nuse /b as x",
            "use /a as x\nuse /b as x\nlet y = x::value",
        ] {
            assert_eq!(
                file_messages(&[
                    ("/main", main),
                    ("/a", "let value = 1"),
                    ("/b", "let value = 2"),
                ]),
                ["/main: `x` is already an alias of `/a`"]
            );
        }
        let files = [
            ("/main", "use /a as x\nuse /b as x"),
            ("/a", ""),
            ("/b", ""),
        ];
        let errs = check_files(&files).expect_err("the alias should be rejected");
        assert_eq!(pointed_at(&files, &errs[0]), "use /b as x");
    }

    #[test]
    fn the_same_alias_for_one_module_is_harmless() {
        assert_eq!(
            check_files(&[
                ("/main", "use /a as x\nuse ./a as x\nlet y = x::value"),
                ("/a", "let value = 1"),
            ]),
            Ok(())
        );
    }

    #[test]
    fn unknown_modules_can_not_be_aliased() {
        assert_eq!(
            file_messages(&[("/main", "use /missing as x\nlet y = x::z")]),
            [
                "/main: cannot find module `/missing`",
                "/main: `x` is not an imported module",
            ]
        );
    }

    #[test]
    fn aliases_are_not_values() {
        assert_eq!(
            file_messages(&[
                ("/main", "use /lib as renamed\nlet x = renamed.y"),
                ("/lib", "let y = 1"),
            ]),
            [
                "/main: `renamed` is a module, not a value, so use `renamed::y` to refer to a name in it"
            ]
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
            check_top("let x: core::Int = 1\nlet y: Int = x\nlet z: core::Bool = true"),
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
            ["operator `+` expects an integer, found `Bool`"]
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
    fn errors_point_at_names() {
        assert_eq!(pointed_at_body("let x = 1 + y"), ["y"]);
        assert_eq!(pointed_at_body("x = 1"), ["x = 1"]);
        assert_eq!(pointed_at_body("missing(a: 1)"), ["missing"]);
        assert_eq!(pointed_at_body("let x: Missing = 1"), ["Missing"]);
        assert_eq!(
            pointed_at_body("let x: other::Missing = 1"),
            ["other::Missing"]
        );
        assert_eq!(
            pointed_at_top("group Box<T> {}\nfn f(a: Box<Missing>) {}"),
            ["Missing"]
        );
        assert_eq!(pointed_at_top("fn f(a: Missing) {}"), ["Missing"]);
        assert_eq!(pointed_at_top("fn f() -> Missing {}"), ["Missing"]);
        assert_eq!(pointed_at_top("use /missing"), ["use /missing"]);
    }

    #[test]
    fn errors_point_at_the_values_with_the_wrong_type() {
        assert_eq!(pointed_at_body("let x: Bool = 1 + 2"), ["1 + 2"]);
        assert_eq!(pointed_at_body("if 1 + 2 {}"), ["1 + 2"]);
        assert_eq!(pointed_at_body("let x = 1 + true"), ["true"]);
        assert_eq!(pointed_at_body("let x = true + 1"), ["true"]);
        assert_eq!(pointed_at_body("let x = true < 1"), ["true"]);
        assert_eq!(pointed_at_body("let x = 1 == true"), ["true"]);
        assert_eq!(pointed_at_body("let x = -true"), ["true"]);
        assert_eq!(pointed_at_body("let x = !1"), ["1"]);
        assert_eq!(
            pointed_at_body("let x = (1 + 2).missing"),
            ["(1 + 2).missing"]
        );
        assert_eq!(pointed_at_top("fn f() -> Int { return true }"), ["true"]);
        assert_eq!(pointed_at_top("fn f() -> Int { return }"), ["return"]);
    }

    #[test]
    fn errors_point_at_the_arguments_of_calls() {
        let f = "fn f(to value: Int) -> Int { return value }";
        let in_main = |call: &str| format!("{f}\nlet x = {call}");
        assert_eq!(pointed_at_top(&in_main("f(to: true)")), ["true"]);
        assert_eq!(pointed_at_top(&in_main("f(from: 1)")), ["from: 1"]);
        assert_eq!(pointed_at_top(&in_main("f()")), ["f()"]);
        assert_eq!(
            pointed_at_top(&in_main("f(to: 1, to: 2)")),
            ["f(to: 1, to: 2)"]
        );
    }

    #[test]
    fn errors_point_at_the_first_line_of_statements_with_blocks() {
        assert_eq!(
            pointed_at_top(
                "
                fn f() -> Int {
                    let x = 1
                }
                "
            ),
            ["fn f() -> Int"]
        );
        assert_eq!(
            pointed_at_top(
                "
                group Point {
                    let x: Point
                }
                "
            ),
            ["group Point"]
        );
        assert_eq!(
            pointed_at_top(
                "
                choice Answer {
                    yes
                    no
                }
                fn f(answer: Answer) {
                    match answer {
                        Answer.yes => {}
                    }
                }
                "
            ),
            ["match answer"]
        );
    }

    #[test]
    fn errors_point_at_redeclarations() {
        assert_eq!(pointed_at_body("let x = 1\nlet x = 2"), ["let x = 2"]);
        assert_eq!(pointed_at_top("fn f(a: Int, a: Int) {}"), ["a: Int"]);
        assert_eq!(
            pointed_at_top(
                "
                group Point {
                    let x: Int
                    let x: Int
                }
                "
            ),
            ["let x: Int"]
        );
        assert_eq!(
            pointed_at_top(
                "
                fn f() {}
                fn f() {}
                "
            ),
            ["fn f()"]
        );
    }

    #[test]
    fn errors_point_at_patterns() {
        let in_match = |patterns: &str| {
            format!(
                "
                fn f(point: (x: Int, y: Int)) {{
                    match point {{
                        {patterns}
                    }}
                }}
                "
            )
        };
        assert_eq!(
            pointed_at_top(&in_match("(x: true, y: _) => {}\n_ => {}")),
            ["true"]
        );
        assert_eq!(
            pointed_at_top(&in_match("_ => {}\n(x: 1, y: _) => {}")),
            ["(x: 1, y: _)"]
        );
        assert_eq!(
            pointed_at_top(&in_match("(x: let a, y: let a) => {}")),
            ["let a"]
        );
        assert_eq!(pointed_at_body("(let a, 1) = (1, 2)"), ["(let a, 1)"]);
    }

    #[test]
    fn errors_point_at_groups_being_constructed() {
        const POINT: &str = "
            group Point {
                let x: Int
                let y: Int
            }
        ";
        let in_impl = |construct: &str| {
            format!(
                "{POINT}
                impl Point {{
                    static fn new() -> Point {{
                        return {construct}
                    }}
                }}
                "
            )
        };
        assert_eq!(
            pointed_at_top(&in_impl("Point { x: 1, y: true }")),
            ["true"]
        );
        assert_eq!(pointed_at_top(&in_impl("Point { x: 1, z: 2 }")), ["z: 2"]);
        assert_eq!(
            pointed_at_top(&in_impl("Point { x: 1 }")),
            ["Point { x: 1 }"]
        );
    }

    #[test]
    fn errors_point_at_impls_of_traits() {
        const TRAIT: &str = "
            trait Named {
                fn name() -> Int
            }
            group Thing {}
        ";
        let with_impl = |functions: &str| {
            format!(
                "{TRAIT}
                impl Thing with Named {{
                    {functions}
                }}
                "
            )
        };
        assert_eq!(pointed_at_top(&with_impl("")), ["Named"]);
        assert_eq!(
            pointed_at_top(&with_impl("fn name() -> Bool { return true }")),
            ["fn name() -> Bool"]
        );
        assert_eq!(
            pointed_at_top(&with_impl(
                "
                fn name() -> Int { return 1 }
                fn other() {}
                "
            )),
            ["fn other()"]
        );
    }

    #[test]
    fn errors_point_at_self_params() {
        assert_eq!(pointed_at_top("let x: Self.T = 1"), ["Self.T"]);
    }

    #[test]
    fn errors_in_default_trait_functions_point_at_the_trait() {
        let files = [
            (
                "/main",
                "
                use /named
                group Thing {}
                impl Thing with Named {
                    fn name() -> Int { return 1 }
                }
                ",
            ),
            (
                "/named",
                "
                trait Named {
                    fn name() -> Int
                    fn other() -> Int { return missing }
                }
                ",
            ),
        ];
        let errs = check_files(&files).expect_err("`missing` isn't declared");
        assert!(errs.iter().all(|err| err.module == "/named"), "{errs:?}");
        assert!(
            errs.iter().all(|err| pointed_at(&files, err) == "missing"),
            "{errs:?}"
        );
    }

    #[test]
    fn missing_types_that_were_not_written_point_at_their_declaration() {
        assert_eq!(
            pointed_at_top(
                "
                let Void = 1
                fn f() {}
                "
            ),
            ["fn f()"]
        );
    }

    #[test]
    fn stdlib_sources_are_found_by_module() {
        assert!(stdlib_source("stdlib/core").is_some());
        assert_eq!(stdlib_source("stdlib/missing"), None);
        assert_eq!(stdlib_source("/core"), None);
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
                "operator `+` expects an integer, found `User`",
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
                "operator `-` expects a signed integer, found `Bool`",
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
            ["operator `+` expects an integer, found `Bool`"]
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
                "operator `+` expects an integer, found `Bool`",
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
                "operator `+` expects an integer, found `Bool`",
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
                "`age` is `Int`, which doesn't implement `Call`, so it can't be called",
                "`User.nextAge` is a method, not a value",
                "`missing` is not declared",
                "`isAdmin` is `Bool`, which doesn't implement `Call`, so it can't be called",
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
    fn types_are_not_values() {
        assert_eq!(
            messages_top(&format!(
                "
                {POINT}
                let a = Int
                "
            )),
            ["`Int` is a type, not a value"]
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
    fn self_type_is_the_type_of_its_impl() {
        let src = "
            group Point {
                let x: Int
            }
            choice Shape {
                circle: Int
                dot: Void
            }

            impl Point {
                static fn origin() -> Self {
                    return Self { x: 0 }
                }

                fn moved(by amount: Int) -> Self {
                    let moved: Self = Self.origin()
                    return Point { x: self.x + moved.x + amount }
                }

                fn is_same(as other: Self) -> Bool {
                    fn origin() -> Self {
                        return Self.origin()
                    }
                    return self.x == other.x & origin().x == 0
                }
            }

            impl Shape {
                fn size() -> Int {
                    match self {
                        Self.circle(let radius) => {
                            return radius
                        }
                        Self.dot => {
                            return 0
                        }
                    }
                }

                static fn unit() -> Self {
                    return Self.circle(1)
                }
            }

            impl Int {
                fn doubled() -> Self {
                    return self * 2
                }
            }

            fn f(point: Point, shape: Shape) -> Bool {
                let moved: Point = point.moved(by: 1)
                let unit: Shape = Shape.unit()
                return moved.is_same(as: point) & unit.size() == 1.doubled()
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn self_type_errors_use_the_impl_type() {
        assert_eq!(
            messages_top(
                "
                group Point {
                    let x: Int
                }
                impl Point {
                    fn f() -> Self {
                        return 1
                    }
                }
                "
            ),
            ["`f` must return `Point`, found `Int`"]
        );
    }

    #[test]
    fn rejects_self_type_outside_impl() {
        assert_eq!(
            messages_top(
                "
                group Node {
                    let next: Self
                }
                alias Current = Self
                impl Self {}
                fn f(x: Self) -> Self {
                    let y: Self = x
                    return Self.new()
                }
                "
            ),
            [
                "`Self` can only be used inside an `impl` or a `trait`",
                "`Self` can only be used inside an `impl` or a `trait`",
                "`Self` can only be used inside an `impl` or a `trait`",
                "`Self` can only be used inside an `impl` or a `trait`",
                "`Self` can only be used inside an `impl` or a `trait`",
                "`Self` can only be used inside an `impl` or a `trait`",
                "`Self` can only be used inside an `impl` or a `trait`",
            ]
        );
    }

    #[test]
    fn self_type_in_an_unknown_impl_is_not_reported_again() {
        assert_eq!(
            messages_top(
                "
                impl Missing {
                    static fn new() -> Self {
                        let x: Self = Self.new()
                        return x
                    }
                }
                "
            ),
            ["unknown type `Missing`"]
        );
    }

    /// `Custom` and the `And` trait, for trait tests.
    const AND: &str = "
        group Custom {
            let value: Int
        }

        trait And {
            fn and(_ rhs: Self) -> Bool
            static fn zero() -> Self
        }
        ";

    #[test]
    fn impls_can_implement_traits() {
        // Parameter names don't need to match, and `Self` is the type being
        // implemented, however it's written.
        let src = format!(
            "
            {AND}
            impl Custom with And {{
                fn and(_ other: Self) -> Bool {{
                    return self.value == other.value
                }}

                static fn zero() -> Custom {{
                    return Custom {{ value: 0 }}
                }}
            }}

            impl Int with And {{
                fn and(_ rhs: Int) -> Bool {{
                    return self == rhs
                }}

                static fn zero() -> Self {{
                    return 0
                }}
            }}

            fn f(custom: Custom) -> Bool {{
                return custom.and(Custom.zero()) & 1.and(Int.zero())
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    /// A trait with aliases, for trait alias tests.
    const COMBINE: &str = "
        trait Combine {
            alias Rhs
            alias Output

            fn combine(with rhs: Rhs) -> Output
        }
        ";

    #[test]
    fn impls_give_the_types_of_trait_aliases() {
        // The aliases can be used in the `impl`'s signatures and bodies,
        // including in functions nested in them, and by later aliases.
        let src = format!(
            "
            {COMBINE}
            impl Int with Combine {{
                alias Rhs = Bool
                alias Output = Rhs

                fn combine(with rhs: Rhs) -> Bool {{
                    fn check(_ value: Output) -> Rhs {{
                        return value
                    }}
                    let checked: Output = check(rhs)
                    return checked & self == 0
                }}
            }}

            impl Bool with Combine {{
                alias Rhs = Int
                alias Output = Self

                fn combine(with rhs: Int) -> Output {{
                    return self | rhs == 0
                }}
            }}

            fn f() -> Bool {{
                return 1.combine(with: true) & false.combine(with: 2)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn impls_infer_aliases_they_do_not_give() {
        // `Rhs` and `Output` are inferred from `combine`, so its body can use
        // them, and given ones are still checked.
        let src = format!(
            "
            {COMBINE}
            impl Int with Combine {{
                fn combine(with rhs: Bool) -> Self {{
                    let keep: Rhs = rhs
                    if keep {{
                        let kept: Output = self
                        return kept
                    }}
                    return 0
                }}
            }}

            impl Bool with Combine {{
                alias Output = Bool

                fn combine(with rhs: Self) -> Bool {{
                    return self & rhs
                }}
            }}

            fn f() -> Int {{
                let a: Bool = true.combine(with: false)
                return 1.combine(with: a)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn operators_work_with_inferred_aliases() {
        let src = "
            group Flags {
                let bits: Int
            }

            impl Flags with And {
                fn and(_ rhs: Self) -> Bool {
                    return self.bits .& rhs.bits != 0
                }
            }

            impl Flags with Not {
                fn not() -> Self {
                    return Flags { bits: .!self.bits }
                }
            }

            fn f(a: Flags) -> Bool {
                let b: Flags = !a
                return a & b
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn inferred_aliases_must_be_used_consistently() {
        // `first` decides `Item`, so `second` doesn't match.
        let src = "
            trait Pair {
                alias Item
                fn first() -> Item
                fn second() -> Item
            }

            impl Int with Pair {
                fn first() -> Int {
                    return self
                }
                fn second() -> Bool {
                    return true
                }
            }
            ";
        assert_eq!(
            messages_top(src),
            ["trait `Pair` declares this function as `fn second() -> Item`"]
        );
    }

    #[test]
    fn inferred_aliases_cannot_be_used_in_signatures() {
        let src = format!(
            "
            {COMBINE}
            trait Make {{
                alias Output
                alias Unused
                fn make() -> Output
            }}

            impl Int with Combine {{
                alias Copy = Rhs
                fn combine(with rhs: Bool) -> Rhs {{
                    return rhs
                }}
            }}

            impl Bool with Make {{
                fn make() -> Bool {{
                    return true
                }}
            }}

            impl Void with Combine {{}}
            "
        );
        // A trait's alias that none of its functions use can't be inferred,
        // but one whose function is missing is only reported as that.
        let inferred = |name| {
            format!(
                "`{name}` is inferred from this `impl`'s signatures, so they can't use it unless it's given with `alias {name} = ...`"
            )
        };
        assert_eq!(
            messages_top(&src),
            [
                inferred("Rhs"),
                inferred("Rhs"),
                "`Copy` is not declared by trait `Combine`".to_string(),
                "`impl Bool with Make` must give `alias Unused`, since it can't be inferred from the `impl`'s functions".to_string(),
                "`impl Void with Combine` is missing `fn combine(with rhs: Rhs) -> Output`".to_string(),
            ]
        );
    }

    #[test]
    fn impl_aliases_are_only_visible_in_their_impl() {
        let src = format!(
            "
            {COMBINE}
            impl Int with Combine {{
                alias Rhs = Bool
                alias Output = Bool

                fn combine(with rhs: Rhs) -> Output {{
                    return rhs
                }}
            }}

            impl Int {{
                fn other(_ rhs: Rhs) {{}}
            }}

            fn f(rhs: Rhs) {{}}
            "
        );
        assert_eq!(
            messages_top(&src),
            ["unknown type `Rhs`", "unknown type `Rhs`"]
        );
    }

    #[test]
    fn impls_can_only_give_their_traits_aliases() {
        let src = format!(
            "
            {COMBINE}
            impl Int with Combine {{
                alias Rhs = Bool
                alias Extra = Int
                alias Rhs = Int

                fn combine(with rhs: Rhs) -> Bool {{
                    return true
                }}
            }}

            impl Bool {{
                alias Output = Bool
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            // `Output` isn't given, so it's inferred from `combine`.
            [
                "`Rhs` is already declared",
                "`Extra` is not declared by trait `Combine`",
                "`alias Output` can only be given in an `impl` with a trait that declares it",
            ]
        );
    }

    #[test]
    fn trait_functions_must_match_with_their_aliases() {
        let src = format!(
            "
            {COMBINE}
            trait Twice {{
                alias Same
                alias Same
            }}

            impl Int with Combine {{
                alias Rhs = Bool
                alias Output = Int

                fn combine(with rhs: Int) -> Int {{
                    return rhs
                }}
            }}

            impl Bool with Combine {{
                alias Rhs = Missing
                alias Output = Bool

                fn combine(with rhs: Int) -> Bool {{
                    return true
                }}
            }}
            "
        );
        // An unknown alias type isn't reported again.
        assert_eq!(
            messages_top(&src),
            [
                "trait `Twice` already declares `Same`",
                "trait `Combine` declares this function as `fn combine(with rhs: Rhs) -> Output`",
                "unknown type `Missing`",
            ]
        );
    }

    #[test]
    fn operators_use_their_implementations_aliases() {
        let src = "
            impl Int with And {
                alias Rhs = Bool
                alias Output = Int

                fn and(_ keep: Rhs) -> Output {
                    if keep {
                        return self
                    }
                    return 0
                }
            }

            impl Int with Not {
                alias Output = Bool

                fn not() -> Output {
                    return self == 0
                }
            }

            fn f() -> Int {
                let a: Int = 5 & true
                let b: Bool = !0 & !1
                return a
            }
            ";
        assert_eq!(check_top(src), Ok(()));
        assert_eq!(
            messages_top(&format!("{src}\nlet c = 5 & 1\nlet d: Int = !1")),
            [
                "operator `&` expects `Bool`, found `Int`",
                "cannot assign `Bool` to `d`, which has type `Int`",
            ]
        );
    }

    #[test]
    fn impls_use_default_trait_functions() {
        // Defaults can call the trait's other functions, including other
        // defaults, and use `Self` and the `impl`'s aliases.
        let src = "
            trait Size {
                alias Unit

                fn size() -> Unit
                static fn zero() -> Unit

                fn doubled() -> Unit {
                    return self.size() + self.size()
                }

                fn isEmpty() -> Bool {
                    return self.size() == Self.zero()
                }

                fn same() -> Self {
                    let unit: Unit = self.doubled()
                    return self
                }
            }

            group Box {
                let width: Int
            }

            impl Box with Size {
                fn size() -> Int {
                    return self.width
                }

                static fn zero() -> Int {
                    return 0
                }
            }

            fn f(box: Box) -> Bool {
                let size: Int = box.same().doubled()
                return box.isEmpty() & Box.zero() == 0
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn impls_can_implement_default_trait_functions_themselves() {
        let src = "
            trait Describe {
                fn size() -> Int

                fn doubled() -> Int {
                    return self.size() * 2
                }
            }

            impl Int with Describe {
                fn size() -> Int {
                    return self
                }

                fn doubled() -> Int {
                    return self + self
                }
            }

            impl Bool with Describe {
                fn size() -> Int {
                    return 1
                }

                fn doubled() -> Bool {
                    return true
                }
            }

            impl Void with Describe {}
            ";
        // Only functions without a default must be implemented.
        assert_eq!(
            messages_top(src),
            [
                "trait `Describe` declares this function as `fn doubled() -> Int`",
                "`impl Void with Describe` is missing `fn size() -> Int`",
            ]
        );
    }

    #[test]
    fn default_trait_functions_are_checked_for_each_impl() {
        // `Self` is a different type in each `impl`, so the default is
        // checked in each one, as if it were written there.
        let src = "
            trait Bump {
                fn bumped() -> Self {
                    return self + 1
                }
            }

            impl Int with Bump {}
            impl Bool with Bump {}
            ";
        assert_eq!(
            messages_top(src),
            [
                "operator `+` expects an integer, found `Bool`",
                "`bumped` must return `Bool`, found `Int`",
            ]
        );
    }

    #[test]
    fn aliases_only_used_by_defaults_must_be_given() {
        let src = "
            trait Echo {
                alias Value
                fn echo(_ value: Value) -> Value {
                    return value
                }
            }

            impl Int with Echo {
                alias Value = Bool
            }

            impl Bool with Echo {}

            fn f() -> Bool {
                return 1.echo(true)
            }
            ";
        assert_eq!(
            messages_top(src),
            [
                "`impl Bool with Echo` must give `alias Value`, since it can't be inferred from the `impl`'s functions"
            ]
        );
    }

    #[test]
    fn equality_operators_call_equal() {
        // `Point` only implements `isEqual`, so `!=` uses the default
        // `isNotEqual`. Integer literals take the other operand's type.
        let src = "
            group Point {
                let x: Int
            }

            impl Point with Equal {
                fn isEqual(to rhs: Self) -> Bool {
                    return self.x == rhs.x
                }
            }

            fn f(a: Point, b: Point, byte: IntU8) -> Bool {
                let same: Bool = a == b & a.isNotEqual(to: b)
                return a != b & 0 == byte & byte != 255 & true != false
            }
            ";
        assert_eq!(check_top(src), Ok(()));
        assert_eq!(
            messages_top(
                "
                group Point {
                    let x: Int
                }
                fn f(a: Point, byte: IntU8) -> Bool {
                    let b = byte == 256
                    let c = 1 == true
                    return a == a
                }
                "
            ),
            [
                "`256` doesn't fit in `IntU8`, which holds `0` to `255`",
                "operator `==` expects `Int`, found `Bool`",
                "operator `==` expects a type that implements `Equal`, found `Point`",
            ]
        );
    }

    #[test]
    fn traits_can_be_implemented_from_other_modules() {
        assert_eq!(
            check_files(&[
                (
                    "/logic",
                    "
                    trait Flip {
                        fn flipped() -> Self
                    }
                    ",
                ),
                (
                    "/main",
                    "
                    use /logic
                    impl Bool with Flip {
                        fn flipped() -> Bool {
                            return !self
                        }
                    }
                    impl Int with logic::Flip {
                        fn flipped() -> Int {
                            return .!self
                        }
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn trait_functions_must_match_their_declarations() {
        let src = format!(
            "
            {AND}
            trait Reset {{
                fn reset(to value: Int)
            }}

            impl Custom with And {{
                fn and(rhs: Self) -> Bool {{
                    return true
                }}

                fn zero() -> Self {{
                    return self
                }}
            }}

            impl Int with And {{
                fn and(_ rhs: Bool) -> Bool {{
                    return true
                }}

                static fn zero() -> Bool {{
                    return true
                }}
            }}

            impl Bool with Reset {{
                fn reset(to value: Int, twice: Bool) {{}}
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "trait `And` declares this function as `fn and(_ rhs: Self) -> Bool`",
                "trait `And` declares this function as `static fn zero() -> Self`",
                "trait `And` declares this function as `fn and(_ rhs: Self) -> Bool`",
                "trait `And` declares this function as `static fn zero() -> Self`",
                "trait `Reset` declares this function as `fn reset(to value: Int)`",
            ]
        );
    }

    #[test]
    fn impls_with_traits_have_exactly_their_functions() {
        let src = format!(
            "
            {AND}
            impl Custom with And {{
                fn and(_ rhs: Self) -> Bool {{
                    return true
                }}

                fn or(_ rhs: Self) -> Bool {{
                    return true
                }}
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "`or` is not declared by trait `And`",
                "`impl Custom with And` is missing `static fn zero() -> Self`",
            ]
        );
    }

    #[test]
    fn traits_are_not_types_or_values() {
        let src = format!(
            "
            {AND}
            impl Custom with Missing {{}}
            impl Custom with Int {{}}
            impl And {{}}
            fn f(x: And) {{
                let y = And
                And()
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "unknown trait `Missing`",
                "`Int` is not a trait",
                "`And` is not a type",
                "`And` is not a type",
                "`And` is a trait, not a value",
                "`And` is not a function",
            ]
        );
    }

    #[test]
    fn traits_report_their_own_errors_once() {
        assert_eq!(
            messages_top(
                "
                trait Broken {
                    fn f(_ x: Missing)
                    fn f()
                }
                trait Twice {}
                group Twice {}
                impl Int with Broken {
                    fn f(_ x: Int) {}
                }
                impl Bool with Broken {
                    fn f(_ x: Bool) {}
                }
                "
            ),
            [
                "`Twice` is already declared",
                "trait `Broken` already declares `f`",
                "unknown type `Missing`",
            ]
        );
    }

    #[test]
    fn only_the_stdlib_can_declare_primitive_functions() {
        assert_eq!(
            messages_top("primitive fn stop() -> Never"),
            ["`stop` can't be a primitive, since only the standard library can declare them"]
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
                    let b = core::flag
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
            [
                "/main: `core` could be `stdlib/core` or `/core`, so use `as` to give them different names",
            ]
        );
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /core as mine
                    let a: Bool = flag
                    let b: Bool = mine::flag
                    let c: Int = core::Int.zero()
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
            Ok(())
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
            let b: Shape = Shape.empty()
            let c: Shape = Shape.empty(Void.new())
            let d: Shape = Shape.nothing()
            fn f() -> Shape {{
                choice Inner {{
                    only
                }}
                let inner: Inner = Inner.only()
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
                        return Shape.empty()
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
                "
            )),
            [
                "argument `value` of `Shape.circle` expects `Int`, found `Bool`",
                "argument to `Shape.circle` should not have a label, found `value`",
                "`Shape.circle` takes 1 argument, found 0",
                "`Shape.circle` takes 1 argument, found 2",
                "argument `value` of `Shape.empty` expects `Void`, found `Int`",
            ]
        );
    }

    #[test]
    fn naming_an_option_does_not_create_the_choice() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let a: Shape = Shape.circle
                let b: Shape = Shape.empty
                let c: ChoiceOption<Shape, Bool> = Shape.circle
                "
            )),
            [
                "cannot assign `ChoiceOption<Shape, Int>` to `a`, which has type `Shape`",
                "cannot assign `ChoiceOption<Shape, Void>` to `b`, which has type `Shape`",
                "cannot assign `ChoiceOption<Shape, Int>` to `c`, which has type `ChoiceOption<Shape, Bool>`",
            ]
        );
    }

    #[test]
    fn options_are_values_with_tags() {
        let src = "
            choice Shape {
                square: Int
                circle: Bool
                other
            }

            let a: ChoiceOption<Shape, Int> = Shape.square
            let b: ChoiceTag<Shape> = Shape.square.tag()
            let c: Shape = Shape.square(5)
            let d: ChoiceTag<Shape> = ChoiceTag.new(of: Shape.square(5))

            let n: ChoiceOption<Shape, Void> = Shape.other
            let o: ChoiceTag<Shape> = Shape.other.tag()
            let p: Shape = Shape.other()
            let q: ChoiceTag<Shape> = ChoiceTag.new(of: Shape.other(Void.new()))
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn options_can_be_called_like_other_values() {
        let src = format!(
            "
            {SHAPE}
            group Maker {{
                let make: ChoiceOption<Shape, Int>
            }}

            fn make(with option: ChoiceOption<Shape, Int>) -> Shape {{
                return option(1)
            }}

            fn f(maker: Maker) {{
                let circle = Shape.circle
                let a: Shape = circle(1)
                let b: Shape = circle.call(input: 1)
                let c: Shape = Shape.circle.call(input: 1)
                let d: Shape = make(with: Shape.circle)
                let e: Shape = maker.make(1)
                let empty = Shape.empty
                let g: Shape = empty()
                let h: Shape = empty(Void.new())
                let i: Shape = empty.call(input: Void.new())
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn rejects_wrong_values_for_stored_options() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f() {{
                    let circle = Shape.circle
                    let a = circle(true)
                    let b = circle(value: 1)
                    let c = circle()
                    let d = circle(1, 2)
                    let empty = Shape.empty
                    let e = empty(1)
                }}
                "
            )),
            [
                "argument `value` of `circle` expects `Int`, found `Bool`",
                "argument to `circle` should not have a label, found `value`",
                "`circle` takes 1 argument, found 0",
                "`circle` takes 1 argument, found 2",
                "argument `value` of `empty` expects `Void`, found `Int`",
            ]
        );
    }

    #[test]
    fn options_take_tuples_as_one_argument() {
        let pair = "
            choice Shape {
                pair: (a: Int, b: Int)
            }
            ";
        let src = format!(
            "
            {pair}
            fn f() {{
                let a: Shape = Shape.pair((a: 1, b: 2))
                let pair = Shape.pair
                let b: Shape = pair((a: 1, b: 2))
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
        assert_eq!(
            messages_top(&format!(
                "
                {pair}
                fn f() {{
                    let a = Shape.pair(a: 1, b: 2)
                    let pair = Shape.pair
                    let b = pair(a: 1, b: 2)
                }}
                "
            )),
            [
                "`Shape.pair` takes 1 argument, found 2",
                "`pair` takes 1 argument, found 2",
            ]
        );
    }

    #[test]
    fn only_options_can_leave_out_void() {
        assert_eq!(
            messages_top(
                "
                group Runner {}

                impl Runner with Call {
                    fn call(input: Void) -> Int {
                        return 1
                    }
                }

                fn f(runner: Runner) -> Int {
                    return runner()
                }
                "
            ),
            ["`runner` is called with `Void`, but was given `()`"]
        );
    }

    #[test]
    fn options_of_generic_choices_infer_their_arguments() {
        let src = format!(
            "
            {GENERICS}
            let a: ChoiceOption<Optional<Int>, Void> = Optional.none
            let b: ChoiceOption<Optional<Int>, Int> = Optional.some
            let c: Optional<Int> = Optional.none()
            let d: Optional<Int> = Optional<Int>.some(1)
            let e: ChoiceTag<Optional<Int>> = Optional<Int>.none.tag()
            "
        );
        assert_eq!(check_top(&src), Ok(()));
        assert_eq!(
            messages_top(&format!(
                "
                {GENERICS}
                let a: Optional<Int> = Optional.none
                let b = Optional.none.tag()
                "
            )),
            [
                "cannot assign `ChoiceOption<Optional<Int>, Void>` to `a`, which has type `Optional<Int>`",
                "the arguments of `Optional` can't be inferred here, so they must be given, as in `Optional<...>`",
            ]
        );
    }

    #[test]
    fn tags_can_be_compared() {
        let src = format!(
            "
            {SHAPE}
            fn isEmpty(shape: Shape) -> Bool {{
                return ChoiceTag<Shape>.new(of: shape) == Shape.empty.tag()
            }}

            fn isNotEmpty(shape: Shape) -> Bool {{
                return Shape.empty.tag() != ChoiceTag.new(of: shape)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                choice Other {{
                    only
                }}
                let a = Shape.empty.tag() == Other.only.tag()
                let b = Shape.empty.tag() == Shape.empty
                let c: ChoiceTag<Shape> = 0
                let d: ChoiceTag<Shape> = Other.only.tag()
                "
            )),
            [
                "operator `==` expects `ChoiceTag<Shape>`, found `ChoiceTag<Other>`",
                "operator `==` expects `ChoiceTag<Shape>`, found `ChoiceOption<Shape, Void>`",
                "cannot assign `Int` to `c`, which has type `ChoiceTag<Shape>`",
                "cannot assign `ChoiceTag<Other>` to `d`, which has type `ChoiceTag<Shape>`",
            ]
        );
    }

    #[test]
    fn options_and_tags_cannot_be_matched() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                fn f() {{
                    match Shape.empty.tag() {{
                        Shape.empty => {{}}
                        _ => {{}}
                    }}
                    match Shape.empty {{
                        Shape.empty => {{}}
                        _ => {{}}
                    }}
                }}
                "
            )),
            [
                "`match` is on `ChoiceTag<Shape>`, so it can't match an option of `Shape`",
                "`match` is on `ChoiceOption<Shape, Void>`, so it can't match an option of `Shape`",
            ]
        );
    }

    #[test]
    fn errors_in_impls_of_tags_are_reported() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                impl ChoiceTag {{
                    fn broken() -> Int {{
                        return true
                    }}
                }}
                "
            )),
            ["`broken` must return `Int`, found `Bool`"]
        );
    }

    #[test]
    fn tags_and_options_can_have_impls() {
        let src = format!(
            "
            {SHAPE}
            impl ChoiceTag {{
                fn isSame(as other: Self) -> Bool {{
                    return self == other
                }}

                static fn from(value: Self.Base) -> ChoiceTag<Self.Base> {{
                    return Self.new(of: value)
                }}
            }}

            impl ChoiceOption {{
                fn make(with value: Self.Value) -> Self.Base {{
                    return self.call(input: value)
                }}
            }}

            let a: Bool = Shape.empty.tag().isSame(as: ChoiceTag.from(value: Shape.empty()))
            let b: Shape = Shape.circle.make(with: 1)
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn tag_types_can_be_used_anywhere() {
        let src = "
            alias ShapeTag = ChoiceTag<Shape>

            group Holder {
                let tag: ChoiceTag<Later>
            }

            choice Nested {
                tag: ChoiceTag<Nested>
                other: ChoiceTag<Later>
            }

            choice Generic<T> {
                value: T
                tag: ChoiceTag<Generic<T>>
            }

            let a: ShapeTag = Shape.empty.tag()

            fn f() {
                choice Local {
                    only
                    tag: ChoiceTag<Local>
                }
                let tag: ChoiceTag<Local> = Local.only.tag()
                let b: ChoiceTag<Generic<Int>> = Generic<Int>.value.tag()
            }

            choice Shape {
                empty
            }

            choice Later {
                empty
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn rejects_wrong_values_for_tags() {
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                choice Other {{
                    only
                }}
                let a = ChoiceTag<Shape>.new(of: Other.only())
                let b = ChoiceTag<Shape>.new(Shape.empty())
                let c = ChoiceTag<Shape>.new()
                let d = ChoiceTag.new()
                let e = ChoiceTag<Shape> {{}}
                "
            )),
            [
                "argument `of` of `ChoiceTag<Shape>.new` expects `Shape`, found `Other`",
                "argument to `ChoiceTag<Shape>.new` is missing the label `of`",
                "`ChoiceTag<Shape>.new` takes 1 argument, found 0",
                "the arguments of `ChoiceTag` can't be inferred here, so they must be given, as in `ChoiceTag<...>`",
                "`ChoiceTag<Shape>` is a primitive, so it can only be created by the compiler, such as with a literal",
            ]
        );
    }

    #[test]
    fn tags_and_options_are_only_for_choices() {
        assert_eq!(
            messages_top(
                "
                group Point {}
                let a: ChoiceTag<Int> = 1
                let b: ChoiceOption<Point, Int> = 1
                let c = ChoiceTag<Int>.new(of: 1)
                let d: ChoiceTag<Missing> = 1
                alias Tag = ChoiceTag<Bool>

                group Holder<T> {
                    let tag: ChoiceTag<T>
                }
                "
            ),
            [
                "`ChoiceTag` can only be used with a choice, found `Bool`",
                // Generic types are checked before top-level values.
                "`ChoiceTag` can only be used with a choice, found `T`",
                "`ChoiceTag` can only be used with a choice, found `Int`",
                "`ChoiceOption` can only be used with a choice, found `Point`",
                "`ChoiceTag` can only be used with a choice, found `Int`",
                "unknown type `Missing`",
            ]
        );
    }

    #[test]
    fn tags_are_available_from_any_module() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /shapes
                    let a: ChoiceTag<Shape> = Shape.empty.tag()
                    let b: ChoiceTag<Shape> = ChoiceTag.new(of: Shape.empty())
                    ",
                ),
                ("/shapes", SHAPE),
            ]),
            Ok(())
        );
    }

    #[test]
    fn choices_have_no_reserved_names() {
        let src = "
            choice Shape {
                tag: Int
                Tag
            }

            impl Shape {
                fn tag() -> Int {
                    return 1
                }
            }

            trait Choice {}

            let a: Int = Shape.tag(1).tag()
            let b: ChoiceTag<Shape> = Shape.tag.tag()
            ";
        assert_eq!(check_top(src), Ok(()));
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                let a = Shape.empty().tag()
                "
            )),
            ["`Shape` has no field or method `tag`"]
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
                let shape = Shape.empty()
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
                let a = Shape.empty() == Shape.empty()
                "
            )),
            ["operator `==` expects a type that implements `Equal`, found `Shape`"]
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
    fn tuples_are_built_with_labels_and_read_by_names() {
        let src = "
            alias Point = (at x: Int, y: Int)

            fn make() -> Point {
                return (at: 1, y: 2)
            }

            fn f(point: Point, pair: (_ first: Bool, _: Int)) -> Int {
                let unit: () = ()
                let single: (_: Int) = (1)
                let plain: Int = (1) + 2
                let named = (count: 3)
                let first: Bool = pair.first
                return point.x + point.y + make().x + named.count + (x: 4).x
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn tuples_without_names_cannot_be_read() {
        assert_eq!(
            messages_top(
                "
                fn f(point: (at x: Int, _: Int)) -> Int {
                    let a = (1, 2).x
                    return point.at
                }
                "
            ),
            [
                "`(_: Int, _: Int)` has no field or method `x`",
                "`(at x: Int, _: Int)` has no field or method `at`",
            ]
        );
    }

    #[test]
    fn tuples_are_the_same_type_only_if_everything_matches() {
        // Labels, names, types and their order all count.
        assert_eq!(
            messages_top(
                "
                fn f(point: (x: Int, y: Int)) -> (at x: Int, y: Int) {
                    let a: (y: Int, x: Int) = point
                    let b: (x: Bool, y: Int) = (x: 1, y: 2)
                    let c: (_: Int) = 1
                    let d: (x: Int) = (x: 1, y: 2)
                    return point
                }
                "
            ),
            [
                "cannot assign `(x: Int, y: Int)` to `a`, which has type `(y: Int, x: Int)`",
                "cannot assign `(x: Int, y: Int)` to `b`, which has type `(x: Bool, y: Int)`",
                "cannot assign `Int` to `c`, which has type `(_: Int)`",
                "cannot assign `(x: Int, y: Int)` to `d`, which has type `(x: Int)`",
                "`f` must return `(at x: Int, y: Int)`, found `(x: Int, y: Int)`",
            ]
        );
    }

    #[test]
    fn tuples_can_be_in_any_type() {
        assert_eq!(
            check_top(
                "
                group Line {
                    let start: (x: Int, y: Int)
                }
                choice Path {
                    line: (from: Line, to: Line)
                    none
                }
                "
            ),
            Ok(())
        );
        assert_eq!(
            messages_top(
                "
                group Node {
                    let next: (node: Node, _: Int)
                }
                trait Pair {
                    fn pair() -> (first: Self, second: Int)
                }
                fn f(x: (y: Missing)) {}
                "
            ),
            [
                "`Self` can't be used inside another type in a trait's signatures, since it's a different type in each `impl`",
                "`Node` contains itself",
                "unknown type `Missing`",
            ]
        );
    }

    #[test]
    fn matches_bind_and_compare_values() {
        // Bindings can be used in their arm's block, and every value must be
        // matched.
        let src = format!(
            "
            {SHAPE}
            fn f(point: (x: Int, y: Bool), shape: (_: Shape, _: Int)) -> Int {{
                match point {{
                    (x: 0, y: true) => {{
                        return 0
                    }}
                    (x: let x, y: true) => {{
                        return x
                    }}
                    (x: -1, y: _) => {{
                        return 1
                    }}
                    _ => {{}}
                }}
                match shape {{
                    (Shape.circle(0), _) => {{}}
                    (Shape.circle(let radius), let scale) => {{
                        return radius * scale
                    }}
                    (Shape.empty, _) => {{}}
                    (Shape.nothing, 3) => {{}}
                    (Shape.nothing, _) => {{}}
                }}
                match true {{
                    true => {{}}
                    false => {{}}
                }}
                return 2
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn matches_report_the_values_they_miss() {
        let src = format!(
            "
            {SHAPE}
            fn f(pair: (a: Bool, b: Bool), shape: (_: Shape, _: Int), n: IntU8) {{
                match pair {{
                    (a: true, b: true) => {{}}
                }}
                match shape {{
                    (Shape.circle(1), _) => {{}}
                    (_, 0) => {{}}
                }}
                match n {{
                    0 => {{}}
                }}
                match pair {{
                    (a: true, b: _) => {{}}
                }}
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "`match` is missing the patterns `(a: true, b: false)` and `(a: false, b: _)`",
                "`match` is missing the patterns `(Shape.circle, _)`, `(Shape.empty, _)` and `(Shape.nothing, _)`",
                "`match` is missing the pattern `_`",
                "`match` is missing the pattern `(a: false, b: _)`",
            ]
        );
    }

    #[test]
    fn matches_reject_unreachable_patterns() {
        let src = format!(
            "
            {SHAPE}
            fn f(pair: (a: Bool, b: Int), shape: Shape) {{
                match pair {{
                    (a: true, b: _) => {{}}
                    (a: let a, b: 0) => {{}}
                    (a: true, b: 1) => {{}}
                    _ => {{}}
                    (a: false, b: 2) => {{}}
                }}
                match shape {{
                    Shape.circle(_) => {{}}
                    Shape.circle(1) => {{}}
                    let other => {{}}
                }}
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "pattern `(a: true, b: 1)` can't be reached, since the patterns before it match everything it does",
                "pattern `(a: false, b: 2)` can't be reached, since the patterns before it match everything it does",
                "pattern `Shape.circle(1)` can't be reached, since the patterns before it match everything it does",
            ]
        );
    }

    #[test]
    fn patterns_must_match_their_types() {
        let src = format!(
            "
            {SHAPE}
            fn f(pair: (a: Bool, b: IntU8), shape: Shape) {{
                match pair {{
                    (a: 1, b: _) => {{}}
                    (b: _, a: _) => {{}}
                    (a: true, b: 256) => {{}}
                    (a: let x, b: let x) => {{}}
                    (a: _, b: true) => {{}}
                    Shape.empty => {{}}
                }}
                match shape {{
                    Shape.circle(true) => {{}}
                    Shape.circle((x: _)) => {{}}
                    _ => {{
                        let y = missing
                    }}
                }}
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "pattern `1` can't match `Bool`",
                "pattern `(b: _, a: _)` can't match `(a: Bool, b: IntU8)`",
                "`256` doesn't fit in `IntU8`, which holds `0` to `255`",
                "`x` is already declared",
                "pattern `true` can't match `IntU8`",
                "`match` is on `(a: Bool, b: IntU8)`, so it can't match an option of `Shape`",
                "pattern `true` can't match `Int`",
                "pattern `(x: _)` can't match `Int`",
                "`missing` is not declared",
            ]
        );
    }

    #[test]
    fn destructuring_declares_constants() {
        // The values can be used after it, and nested patterns can take
        // values apart further.
        let src = "
            fn f(point: (x: Int, y: (z: Bool, w: IntU8))) -> Int {
                let x = 3
                let y = 5
                (let a, let b) = (y, x)
                (x: let px, y: (z: let pz, w: _)) = point
                (_, let c) = (true, a + b)
                let flag: Bool = pz
                return a + b + px + c
            }
            ";
        assert_eq!(check_top(src), Ok(()));
        assert_eq!(
            messages_top(
                "
                fn f(point: (x: Int, y: Int)) {
                    (let a, let b) = (1, 2)
                    a = 3
                    (let a, let c) = (1, 2)
                }
                "
            ),
            [
                "cannot assign to `a` because it was declared with `let`",
                "`a` is already declared",
            ]
        );
    }

    #[test]
    fn destructuring_must_match_every_value() {
        let src = format!(
            "
            {SHAPE}
            fn f(point: (x: Int, y: Bool), shape: (_: Shape)) {{
                (x: 0, y: let y) = point
                (x: let x, y: true) = point
                (Shape.circle(let r)) = shape
                (let a, let b) = point
                (x: let c) = point
                (let d, let e) = missing
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "pattern `(x: 0, y: let y)` doesn't match every value, such as `(x: _, y: _)`, so it can only be used in a `match`",
                "pattern `(x: let x, y: true)` doesn't match every value, such as `(x: _, y: false)`, so it can only be used in a `match`",
                "pattern `(Shape.circle(let r))` doesn't match every value, such as `(Shape.empty)`, so it can only be used in a `match`",
                "pattern `(let a, let b)` can't match `(x: Int, y: Bool)`",
                "pattern `(x: let c)` can't match `(x: Int, y: Bool)`",
                "`missing` is not declared",
            ]
        );
    }

    /// Types that implement `Call`, for call tests.
    const CALLABLES: &str = "
        group Adder {
            let base: Int
        }

        impl Adder with Call {
            fn call(input: (a: Int, b: Int)) -> Int {
                return self.base + input.a + input.b
            }
        }

        group Negate {}

        impl Negate with Call {
            fn call(input: Int) -> Int {
                return -input
            }
        }

        group Counter {}

        impl Counter with Call {
            fn call(input: ()) -> Bool {
                return true
            }
        }

        group Tools {
            let adder: Adder
            let negate: Negate
        }
        ";

    #[test]
    fn values_implementing_call_can_be_called() {
        // By name or as a field, with the input written like a tuple without
        // its brackets, or by calling `call` itself.
        let src = format!(
            "
            {CALLABLES}
            fn f(adder: Adder, negate: Negate, counter: Counter, tools: Tools) -> Int {{
                let sum: Int = adder(a: 3, b: 5)
                let same: Int = adder.call(input: (a: 3, b: 5))
                let negated: Int = negate(sum) + tools.negate(1)
                let counted: Bool = counter()
                let pair = (first: adder, second: 2)
                return tools.adder(a: 1, b: 2) + pair.first(a: negated, b: same)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn calls_need_call_and_its_input() {
        let src = format!(
            "
            {CALLABLES}
            fn f(adder: Adder, negate: Negate, tools: Tools, count: Int) {{
                let a = count(1)
                let b = adder(a: 3)
                let c = adder(b: 3, a: 5)
                let d = negate(true)
                let e = negate()
                let f = tools.adder(a: missing, b: 1)
                let g = unknown(1)
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "`count` is `Int`, which doesn't implement `Call`, so it can't be called",
                "`adder` is called with `(a: Int, b: Int)`, but was given `(a: Int)`",
                "`adder` is called with `(a: Int, b: Int)`, but was given `(b: Int, a: Int)`",
                "`negate` is called with `Int`, but was given `Bool`",
                "`negate` is called with `Int`, but was given `()`",
                "`missing` is not declared",
                "`unknown` is not declared",
            ]
        );
    }

    #[test]
    fn functions_are_values_of_type_fn() {
        // The `Input` is the type of an only parameter without a label that
        // isn't a tuple, and otherwise a tuple of the parameters, by their
        // labels.
        let src = "
            fn add(a: Int, b: Int) -> Int {
                return a + b
            }

            fn negate(_ value: Int) -> Int {
                return -value
            }

            fn scale(_ value: Int, by factor: Int) -> Int {
                return value * factor
            }

            fn same(to value: Int) -> Int {
                return value
            }

            fn sum(_ pair: (a: Int, b: Int)) -> Int {
                return pair.a + pair.b
            }

            fn nothing() {}

            fn f() {
                let a: Fn<(a: Int, b: Int), Int> = add
                let b: Fn<Int, Int> = negate
                let c: Fn<(_: Int, by: Int), Int> = scale
                let d: Fn<(to: Int), Int> = same
                let e: Fn<(_: (a: Int, b: Int)), Int> = sum
                let g: Fn<(), Void> = nothing
                let i: Int = a(a: 1, b: 2) + b(3) + c(4, by: 5) + d(to: 6) + e((a: 7, b: 8))
                let j: Void = g()
                let k: Int = add.call(input: (a: 1, b: 2))
                let pair = (a: 1, b: 2)
                let l: Int = add.call(input: pair) + sum(pair) + sum.call(input: (pair))
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn functions_as_values_are_checked_like_other_values() {
        let src = "
            fn add(a: Int, b: Int) -> Int {
                return a + b
            }

            fn f() {
                let a: Fn<Int, Int> = add
                let b: Fn<Int> = add
                let c = add.missing
                let d = Fn<Int, Int> {}
                let g = add
                let h: Bool = g(a: 1, b: 2)
                let i = g(a: 1)
            }
            ";
        assert_eq!(
            messages_top(src),
            [
                "cannot assign `Fn<(a: Int, b: Int), Int>` to `a`, which has type `Fn<Int, Int>`",
                "`Fn` has 2 generic parameters, but was given 1",
                "`Fn<(a: Int, b: Int), Int>` has no field or method `missing`",
                "`Fn<Int, Int>` is a primitive, so it can only be created by the compiler, such as with a literal",
                "cannot assign `Int` to `h`, which has type `Bool`",
                "`g` is called with `(a: Int, b: Int)`, but was given `(a: Int)`",
            ]
        );
    }

    #[test]
    fn function_arguments_are_checked_as_their_parameters() {
        let src = "
            fn small(a: IntU8, b: IntU8) {}

            fn f() {
                small(a: 1, c: 2)
                small(a: 1 + true, c: 2)
                small(a: missing)
            }
            ";
        assert_eq!(
            messages_top(src),
            [
                "argument to `small` should be labelled `b`, found `c`",
                "operator `+` expects an integer, found `Bool`",
                "argument `a` of `small` expects `IntU8`, found `Int`",
                "argument to `small` should be labelled `b`, found `c`",
                "`missing` is not declared",
                "`small` takes 2 arguments, found 1",
            ]
        );
    }

    #[test]
    fn tuples_are_not_unpacked_into_arguments() {
        let src = format!(
            "
            {CALLABLES}
            fn add(x: Int, y: Int) -> Int {{
                return x + y
            }}

            fn f(adder: Adder) {{
                let pair = (x: 3, y: 5)
                let sum = add
                add(pair)
                add((x: 3, y: 5))
                sum(pair)
                adder((a: 3, b: 5))
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "`add` takes 2 arguments, found 1",
                "`add` takes 2 arguments, found 1",
                "`sum` is called with `(x: Int, y: Int)`, but was given `(_: (x: Int, y: Int))`",
                "`adder` is called with `(a: Int, b: Int)`, but was given `(_: (a: Int, b: Int))`",
            ]
        );
    }

    #[test]
    fn a_tuple_is_one_argument_to_a_function_taking_a_tuple() {
        let src = "
            fn sum(_ pair: (x: Int, y: Int)) -> Int {
                return pair.x + pair.y
            }

            fn only(_ value: (_ inner: Int)) -> Int {
                return value.inner
            }

            fn f(value: (_ inner: Int)) -> Int {
                let pair = (x: 3, y: 5)
                return sum(pair) + sum((x: 3, y: 5)) + only(value) + only((1))
            }

            fn g() {
                sum(x: 3, y: 5)
            }
            ";
        assert_eq!(messages_top(src), ["`sum` takes 1 argument, found 2"]);
    }

    #[test]
    fn call_takes_a_whole_tuple_as_its_input() {
        let src = format!(
            "
            {CALLABLES}
            fn add(x: Int, y: Int) -> Int {{
                return x + y
            }}

            fn f(adder: Adder) -> Int {{
                let pair = (x: 3, y: 5)
                let input = (a: 3, b: 5)
                return add.call(input: pair) + adder.call(input: input)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn static_functions_are_values_of_type_fn() {
        let src = "
            group Point {
                let x: Int
                let y: Int
            }

            impl Point {
                static fn at(x: Int, y: Int) -> Self {
                    return Self { x: x, y: y }
                }

                static fn origin() -> Self {
                    return Self.at(x: 0, y: 0)
                }
            }

            group Box<T> {
                let value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }
            }

            fn f() {
                let at: Fn<(x: Int, y: Int), Point> = Point.at
                let origin: Fn<(), Point> = Point.origin
                let of: Fn<Int, Box<Int>> = Box<Int>.of
                let a: Point = at(x: 1, y: 2)
                let b: Point = origin()
                let c: Box<Int> = of(1)
                let d: Point = Point.at.call(input: (x: 1, y: 2))
                let e: Box<Int> = Box.of(1)
                let pair = (x: 1, y: 2)
                let g: Point = at.call(input: pair)
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn static_functions_are_called_like_other_functions() {
        let src = "
            group Point {
                let x: Int
                let y: Int
            }

            impl Point {
                static fn at(x: Int, y: Int) -> Self {
                    return Self { x: x, y: y }
                }
            }

            fn f(point: Point) {
                let pair = (x: 1, y: 2)
                let at = Point.at
                Point.at(pair)
                Point.at(x: true, y: 2)
                Point.at(x: 1, z: 2)
                Point.at(x: 1)
                at(x: 1)
                let a: Fn<Int, Point> = Point.at
                let b = Point.missing
                let c = point.at
            }
            ";
        assert_eq!(
            messages_top(src),
            [
                "`Point.at` takes 2 arguments, found 1",
                "argument `x` of `Point.at` expects `Int`, found `Bool`",
                "argument to `Point.at` should be labelled `y`, found `z`",
                "`Point.at` takes 2 arguments, found 1",
                "`at` is called with `(x: Int, y: Int)`, but was given `(x: Int)`",
                "cannot assign `Fn<(x: Int, y: Int), Point>` to `a`, which has type `Fn<Int, Point>`",
                "`Point` has no static function `missing`",
                "`Point.at` is a static function, so it must be used on `Point` itself rather than a value",
            ]
        );
    }

    #[test]
    fn static_functions_as_values_must_be_imported() {
        let ints = "
            impl Int {
                static fn one() -> Int {
                    return 1
                }
            }
            ";
        let other = "
            fn f() -> Fn<(), Int> {
                return Int.one
            }
            ";
        assert_eq!(
            file_messages(&[("/ints", ints), ("/other", other)]),
            ["/other: `Int.one` is declared in `/ints`, which is not imported"]
        );
    }

    #[test]
    fn functions_that_never_return_end_their_path_as_values_too() {
        let src = "
            fn f() -> Int {
                let stop = exit
                stop()
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn functions_from_other_modules_are_values_once_imported() {
        let other = "
            fn one() -> Int {
                return 1
            }
            ";
        assert_eq!(
            check_files(&[
                ("/other", other),
                (
                    "/main",
                    "use /other\nfn f() -> Fn<(), Int> {\nreturn other::one\n}"
                ),
            ]),
            Ok(())
        );
        assert_eq!(
            file_messages(&[
                ("/other", other),
                ("/main", "fn f() -> Fn<(), Int> {\nreturn other::one\n}"),
            ]),
            ["/main: `other` is not an imported module"]
        );
    }

    #[test]
    fn a_type_called_fn_does_not_change_function_values() {
        let src = "
            group Fn {}

            fn one() -> Int {
                return 1
            }

            fn f(mine: Fn) -> Int {
                let value = one
                return value() + one()
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn rejects_generic_primitives_outside_the_stdlib() {
        assert_eq!(
            messages_top("primitive Pointer<T>"),
            ["`Pointer` can't be a primitive, since only the standard library can declare them"]
        );
    }

    /// Generic types for generics tests.
    const GENERICS: &str = "
        choice Optional<T> {
            none
            some: T
        }

        group Box<T> {
            let value: T
        }

        impl Box {
            static fn of(_ value: Self.T) -> Self {
                return Box { value: value }
            }

            fn get() -> Self.T {
                return self.value
            }
        }
        ";

    #[test]
    fn generic_arguments_are_inferred_from_calls() {
        let src = format!(
            "
            {GENERICS}
            choice Shape {{
                empty
            }}

            group Pair<A, B> {{
                let first: A
                let second: B
            }}

            impl Pair {{
                static fn of(first: Self.A, second: Self.B) -> Self {{
                    return Self {{ first: first, second: second }}
                }}

                static fn tupled(pair: (a: Self.A, b: Self.B)) -> Self {{
                    return Self {{ first: pair.a, second: pair.b }}
                }}

                static fn copy(of other: Self) -> Self {{
                    return other
                }}
            }}

            impl Box {{
                static fn unwrapping(_ boxed: Box<Self.T>) -> Self {{
                    return boxed
                }}

                fn again() -> Box<Self.T> {{
                    let same = Box.of(self.value)
                    return same
                }}
            }}

            choice Nested<T> {{
                boxed: Box<T>
                pair: (a: T, b: Bool)
            }}

            let a: Box<Int> = Box.of(1)
            let b: Bool = Box.of(true).get()
            let c: Optional<Int> = Optional<Int>.none()
            let d: Int = Box.of(Box.of(1)).get().get()
            let e: Optional<Bool> = Optional.some(true)
            let f = Optional.some(1)
            let g: Optional<Int> = f
            let h: Pair<Int, Bool> = Pair.of(first: 1, second: true)
            let i: Pair<Int, Bool> = Pair.tupled(pair: (a: 1, b: true))
            let j: Box<Int> = Box.unwrapping(Box.of(1))
            let k: Pair<Int, Bool> = Pair.copy(of: h)
            let l: Nested<Int> = Nested.boxed(Box.of(1))
            let m: Nested<Int> = Nested.pair((a: 1, b: true))
            let n: ChoiceTag<Shape> = ChoiceTag.new(of: Shape.empty())
            let o = ChoiceTag.new(of: Shape.empty()) == Shape.empty.tag()

            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn inferred_arguments_take_types_from_those_already_known() {
        let src = "
            choice Optional<T> {
                none
                some: T
            }

            group Box<T> {
                let value: T
            }

            impl Box {
                static fn with(value: Self.T, fallback: Optional<Self.T>) -> Self {
                    return Self { value: value }
                }
            }

            fn f() {
                let small: IntU8 = 1
                let a: Box<IntU8> = Box.with(value: small, fallback: Optional.none())
                let b: Box<IntU8> = Box.with(value: small, fallback: Optional.some(1))
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn rejects_calls_whose_generic_arguments_cannot_be_inferred() {
        assert_eq!(
            messages_top(&format!(
                "
                {GENERICS}
                group Pair<A, B> {{}}

                impl Pair {{
                    static fn left(_ value: Self.A) -> Self {{
                        return Self {{}}
                    }}

                    static fn same(first: Self.A, second: Self.A, other: Self.B) -> Self {{
                        return Self {{}}
                    }}

                    static fn optional(first: Self.A, second: Optional<Self.A>, other: Self.B) -> Self {{
                        return Self {{}}
                    }}
                }}

                impl Box {{
                    static fn plain(_ value: Int) -> Self {{
                        return Box.plain(value)
                    }}
                }}

                choice Shape {{
                    empty
                }}

                fn f() {{
                    let a = Optional.none()
                    let b = Optional.none(Void.new())
                    let c = Box.of()
                    let d = Pair.left(1)
                    let e = Box.plain(1)
                    let g = Box.of(missing)
                    let h = Box.of(1, 2)
                    let i = Box.of(value: 1)
                    let j = Pair.same(first: 1, second: true, other: 1)
                    let k: Box<Bool> = Box.of(1)
                    let l: Int = Box.of(1)
                    let m = Box.of(true) == 1
                    let n = ChoiceTag.new(of: 1)
                    let o = Box.missing(1)
                    // Arguments only take types from a generic type once all
                    // of its arguments are known, which needs `other` here.
                    let p = Pair.optional(first: 1, second: Optional.none(), other: true)
                }}
                "
            )),
            [
                "the arguments of `Optional` can't be inferred here, so they must be given, as in `Optional<...>`",
                "the arguments of `Optional` can't be inferred here, so they must be given, as in `Optional<...>`",
                "the arguments of `Box` can't be inferred here, so they must be given, as in `Box<...>`",
                "the arguments of `Pair` can't be inferred here, so they must be given, as in `Pair<...>`",
                "the arguments of `Box` can't be inferred here, so they must be given, as in `Box<...>`",
                "`missing` is not declared",
                "`Box<Int>.of` takes 1 argument, found 2",
                "argument to `Box<Int>.of` should not have a label, found `value`",
                "argument `second` of `Pair<Int, Int>.same` expects `Int`, found `Bool`",
                "argument `value` of `Box<Bool>.of` expects `Bool`, found `Int`",
                "cannot assign `Box<Int>` to `l`, which has type `Int`",
                "operator `==` expects a type that implements `Equal`, found `Box<Bool>`",
                "`ChoiceTag` can only be used with a choice, found `Int`",
                "the arguments of `Box` can't be inferred here, so they must be given, as in `Box<...>`",
                "the arguments of `Optional` can't be inferred here, so they must be given, as in `Optional<...>`",
            ]
        );
    }

    #[test]
    fn inferred_static_functions_must_be_imported() {
        let messages = check_files(&[
            (
                "/main",
                "
                use /box

                let a = Box.extra(1)
                let b = Box.of(1)
                ",
            ),
            (
                "/box",
                "
                group Box<T> {}

                impl Box {
                    static fn of(_ value: Self.T) -> Self {
                        return Self {}
                    }
                }
                ",
            ),
            (
                "/extra",
                "
                use /box

                impl Box {
                    static fn extra(_ value: Self.T) -> Self {
                        return Self.of(value)
                    }
                }
                ",
            ),
        ]);
        let messages: Vec<_> = messages
            .err()
            .into_iter()
            .flatten()
            .map(|err| err.error.to_string())
            .collect();
        assert_eq!(
            messages,
            ["`Box<Int>.extra` is declared in `/extra`, which is not imported"]
        );
    }

    #[test]
    fn generic_types_can_be_used_with_any_arguments() {
        let src = format!(
            "
            {GENERICS}
            group Pair<A, B> {{
                let first: A
                let second: B
            }}

            alias Maybe = Optional<(x: Int, y: Bool)>

            group Holder {{
                let boxes: Pair<Box<Int>, Optional<Box<Bool>>>
            }}

            fn f(holder: Holder, maybe: Maybe) -> Int {{
                let first: Box<Int> = holder.boxes.first
                let other: Optional<Box<Bool>> = holder.boxes.second
                match maybe {{
                    Optional.some((x: let x, y: true)) => {{
                        return x + first.get()
                    }}
                    Optional<(x: Int, y: Bool)>.some(_) => {{}}
                    Optional.none => {{}}
                }}
                return Box<Int>.of(1).value
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn generic_arguments_are_inferred_where_they_are_known() {
        // From where a value is used, from a pattern's matched value, and in
        // an `impl`, from `Self`.
        let src = format!(
            "
            {GENERICS}
            impl Box {{
                fn rebuilt() -> Self {{
                    return Box {{ value: self.value }}
                }}
            }}

            fn none() -> Optional<Int> {{
                return Optional.none()
            }}

            fn f(_ value: Optional<Box<Bool>>) -> Box<Int> {{
                let some: Optional<Int> = Optional.some(1)
                let boxed: Box<Int> = Box.of(2)
                match value {{
                    Optional.some(let found) => {{
                        let flag: Bool = found.get()
                    }}
                    Optional.none => {{}}
                }}
                return Box.of(some == none())
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            // `Box.of(some == none())` is a `Box<Bool>`, inferred from the
            // return type, which `==` can't compare.
            ["operator `==` expects a type that implements `Equal`, found `Optional<Int>`"]
        );
        assert_eq!(
            messages_top(&format!(
                "
                {GENERICS}
                fn f() {{
                    let a = Optional.none
                    let b = Box.of()
                    match 1 {{
                        Optional.none => {{}}
                        _ => {{}}
                    }}
                }}
                "
            )),
            [
                "the arguments of `Optional` can't be inferred here, so they must be given, as in `Optional<...>`",
                "the arguments of `Box` can't be inferred here, so they must be given, as in `Box<...>`",
                "the arguments of `Optional` can't be inferred here, so they must be given, as in `Optional<...>`",
            ]
        );
    }

    #[test]
    fn generic_types_need_the_right_arguments() {
        let src = format!(
            "
            {GENERICS}
            fn f(a: Optional, b: Int<Bool>, c: Box<Int, Bool>, d: Optional<Missing>) {{
                let e = Optional
            }}
            "
        );
        assert_eq!(
            messages_top(&src),
            [
                "`Optional` is generic, so it needs arguments, as in `Optional<...>`",
                "`Int` isn't generic, so it can't be given arguments",
                "`Box` has 1 generic parameter, but was given 2",
                "unknown type `Missing`",
                "`Optional` is a type, not a value",
            ]
        );
    }

    #[test]
    fn generic_impls_use_self_params() {
        let src = format!(
            "
            {GENERICS}
            impl Box {{
                fn other() -> Self.U {{
                    let t: T = self.value
                }}
            }}

            impl Int {{
                fn f() -> Self.T {{}}
            }}

            fn g() -> Self.T {{}}
            "
        );
        // A generic type's `impl`s are checked after the other declarations.
        assert_eq!(
            messages_top(&src),
            [
                "`Self.T` can only be used in the `impl` of a generic type",
                "`Self.T` can only be used in the `impl` of a generic type",
                "`Box<T>` has no generic parameter `U`",
                "unknown type `T`",
            ]
        );
    }

    #[test]
    fn generic_types_are_checked_once() {
        // A generic type's parameters can't be used as anything in particular,
        // and its mistakes are reported once, however many types are made
        // from it.
        let src = "
            group Box<T, T> {
                let value: T
                let value: Missing
            }

            group Counter<T> {
                let value: T
            }

            impl Counter {
                fn next() -> Self.T {
                    return self.value + 1
                }
            }

            fn f(a: Counter<Int>, b: Counter<Bool>, c: Counter<IntU8>) {}
            ";
        assert_eq!(
            messages_top(src),
            [
                "`T` is already declared",
                "`Box<T, T>` already has a field `value`",
                "unknown type `Missing`",
                "operator `+` expects an integer, found `T`",
                "`next` must return `T`, found `Int`",
            ]
        );
    }

    #[test]
    fn generic_types_cannot_contain_themselves() {
        let src = "
            group Wrap<T> {
                let inner: Wrap<T>
            }

            choice Optional<T> {
                none
                some: T
            }

            group Grow<T> {
                let next: Optional<Grow<(_: T)>>
            }

            fn f() {
                group Local<T> {}
            }
            ";
        let messages = messages_top(src);
        assert!(
            messages[0].starts_with("`Grow<(_: (_: ")
                && messages[0].ends_with(
                    "nests generic types too deeply, which usually means a generic type uses itself with different arguments"
                ),
            "{messages:?}"
        );
        assert_eq!(messages[1], "`Wrap<T>` contains itself");
        assert_eq!(
            messages[2..],
            ["`Local` can't be generic, since only types at the top level can be"]
        );
    }

    #[test]
    fn generic_impls_can_implement_traits() {
        let src = format!(
            "
            {GENERICS}
            impl Box with Equal {{
                fn isEqual(to rhs: Self) -> Bool {{
                    return true
                }}
            }}

            fn f() -> Bool {{
                return Box<Int>.of(1) == Box.of(2) & Box<Bool>.of(true) != Box.of(false)
            }}
            "
        );
        assert_eq!(check_top(&src), Ok(()));
    }

    #[test]
    fn matches_must_cover_every_value() {
        // There are too many integers to list, so they need a `_`.
        assert_eq!(
            check_body("match 1 {}"),
            Err(vec![CheckError::MissingPatterns {
                patterns: vec!["_".to_string()],
                more: false,
            }])
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
            // Once patterns have errors, which values the others match isn't
            // checked, since the ones with errors could match anything.
            [
                "`match` is on `Shape`, so it can't match an option of `Other`",
                "`Shape` has no option `square`",
                "unknown type `Missing`",
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
                "`match` is missing the patterns `Shape.circle` and `Shape.nothing`",
                "`match` is missing the pattern `Shape.nothing`",
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

    #[test]
    fn aliases_are_interchangeable_with_their_types() {
        let src = "
            alias ID = Int
            let a: ID = 1
            let b: Int = a
            let c: ID = b + a

            fn next(after id: ID) -> Int {
                return id + 1
            }

            // An `impl` of an alias is an `impl` of its type.
            impl ID {
                static fn first() -> ID {
                    return 0
                }

                fn doubled() -> Int {
                    return self * 2
                }
            }

            let d: ID = Int.first()
            let e: Int = ID.first().doubled() + next(after: 3)
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn errors_name_types_as_written() {
        // A type found through an alias is named after the alias.
        assert_eq!(
            messages_top("alias ID = Int\nlet a: ID = true\nlet b: IntS64 = true"),
            [
                "cannot assign `Bool` to `a`, which has type `ID`",
                "cannot assign `Bool` to `b`, which has type `IntS64`",
            ]
        );
    }

    #[test]
    fn aliases_can_be_used_in_any_order() {
        assert_eq!(
            check_files(&[
                (
                    "/main",
                    "
                    use /shapes
                    let origin: Origin = Spot.at(x: 0)
                    alias Origin = Location
                    alias Location = shapes::Spot
                    ",
                ),
                (
                    "/shapes",
                    "
                    alias Spot = Point
                    group Point {
                        let x: Int
                    }
                    impl Spot {
                        static fn at(x: Int) -> Point {
                            return Spot { x: x }
                        }
                    }
                    ",
                ),
            ]),
            Ok(())
        );
    }

    #[test]
    fn aliases_of_choices_have_their_options() {
        let src = format!(
            "
            {SHAPE}
            alias Figure = Shape
            fn size(of figure: Figure) -> Int {{
                match figure {{
                    Figure.circle(let radius) => {{
                        return radius
                    }}
                    Shape.empty => {{
                        return 0
                    }}
                    Figure.nothing => {{
                        return 0
                    }}
                }}
            }}
            let a = size(of: Figure.circle(2))
            let b = size(of: Shape.empty())
            "
        );
        assert_eq!(check_top(&src), Ok(()));
        assert_eq!(
            messages_top(&format!(
                "
                {SHAPE}
                alias Figure = Shape
                fn f(figure: Figure) {{
                    match figure {{
                        Figure.circle => {{}}
                        Shape.circle => {{}}
                        Shape.empty => {{}}
                        Shape.nothing => {{}}
                    }}
                }}
                "
            )),
            [
                "pattern `Shape.circle` can't be reached, since the patterns before it match everything it does"
            ]
        );
    }

    #[test]
    fn aliases_inside_functions() {
        let src = "
            fn f() -> Bool {
                alias Flag = Bool
                let flag: Flag = true
                fn invert(_ flag: Flag) -> Flag {
                    return !flag
                }
                return invert(flag)
            }
            ";
        assert_eq!(check_top(src), Ok(()));
        // Like other names in functions, they're only visible after they're
        // declared, and only in their own block.
        assert_eq!(
            messages_top(
                "
                fn f() {
                    let a: Early = 1
                    alias Early = Int
                    if true {
                        alias Inner = Int
                    }
                    let b: Inner = 1
                }
                "
            ),
            ["unknown type `Early`", "unknown type `Inner`"]
        );
    }

    #[test]
    fn rejects_cyclic_aliases() {
        assert_eq!(
            check_top("alias A = B\nalias B = C\nalias C = A\nalias D = D\nlet x: A = 1"),
            Err(vec![
                CheckError::CyclicAlias("A".to_string()),
                CheckError::CyclicAlias("D".to_string()),
            ])
        );
        assert_eq!(messages_top("alias A = A"), ["`A` is an alias of itself"]);
    }

    #[test]
    fn aliases_must_be_of_types() {
        // Uses of an alias whose type is unknown don't cause more errors.
        assert_eq!(
            messages_top(
                "
                alias A = Missing
                fn f(a: A) -> A {
                    let value = 1
                    alias B = value
                    let b: B = a
                    return A.anything()
                }
                "
            ),
            ["unknown type `Missing`", "`value` is not a type"]
        );
    }

    #[test]
    fn aliases_share_names_with_other_declarations() {
        assert_eq!(
            check_top("group Point {}\nalias Point = Int"),
            Err(vec![CheckError::Redeclared("Point".to_string())])
        );
        // A module's own names come before the standard library's.
        assert_eq!(check_top("alias Int = Bool\nlet flag: Int = true"), Ok(()));
    }

    #[test]
    fn accepts_every_integer_type() {
        let src = "
            let a: IntU8 = 255
            let b: IntU16 = 65535
            let c: IntU32 = 4294967295
            let d: IntU64 = 18446744073709551615
            let e: IntS8 = -128
            let f: IntS16 = 32767
            let g: IntS32 = -2147483648
            let h: IntS64 = -9223372036854775808
            // `Int` is `IntS64`, so they can be used in place of each other.
            let i: Int = h
            let j: IntS64 = i
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn literals_take_their_type_from_where_theyre_used() {
        let src = "
            group Pixel {
                let red: IntU8
            }

            impl Pixel {
                static fn new(red: IntU8) -> Pixel {
                    return Pixel { red: 200 }
                }

                fn brighter(by amount: IntU8) -> IntU8 {
                    return self.red + amount
                }
            }

            choice Level {
                exact: IntU8
                none
            }

            fn half(of byte: IntU8) -> IntU8 {
                return byte / 2
            }

            fn f() -> IntU8 {
                var byte: IntU8 = 250
                byte = 255
                let level = Level.exact(255)
                let pixel = Pixel.new(red: 255)
                let sum: IntU8 = half(of: 255) + pixel.brighter(by: 1)
                // The other operand gives the type, even when it's second,
                // and even through other literals.
                let a = byte + 1
                let b = 1 + byte
                let c = (1 + 2) * byte
                let d: Bool = 255 == byte
                let small: IntS8 = -(1 + 2) * -4
                return a + b + c
            }
            ";
        assert_eq!(check_top(src), Ok(()));
    }

    #[test]
    fn literals_without_a_type_are_ints() {
        assert_eq!(
            messages_top("let a = 5\nlet b: IntU8 = a\nlet c: Bool = 1"),
            [
                "cannot assign `Int` to `b`, which has type `IntU8`",
                "cannot assign `Int` to `c`, which has type `Bool`",
            ]
        );
    }

    #[test]
    fn rejects_literals_that_dont_fit() {
        assert_eq!(
            messages_top(
                "
                fn byte(_ value: IntU8) {}
                fn f() {
                    let a: IntU8 = 256
                    let b: IntU8 = -1
                    let c: IntS8 = 128
                    let d: IntS8 = -129
                    let e = 9223372036854775808
                    let g: IntU64 = 18446744073709551615
                    byte(300)
                }
                "
            ),
            [
                "`256` doesn't fit in `IntU8`, which holds `0` to `255`",
                "`-1` doesn't fit in `IntU8`, which holds `0` to `255`",
                "`128` doesn't fit in `IntS8`, which holds `-128` to `127`",
                "`-129` doesn't fit in `IntS8`, which holds `-128` to `127`",
                "`9223372036854775808` doesn't fit in `Int`, which holds `-9223372036854775808` to `9223372036854775807`",
                "`300` doesn't fit in `IntU8`, which holds `0` to `255`",
            ]
        );
    }

    #[test]
    fn rejects_mixing_integer_types() {
        assert_eq!(
            messages_top(
                "
                fn f(small: IntU8, big: IntS64) {
                    let a = small + big
                    let b = big * small
                    let c = small == big
                    let d: IntU16 = small
                    let e: Int = small + 1
                    let f = big >= small
                }
                "
            ),
            [
                "operator `+` expects `IntU8`, found `IntS64`",
                "operator `*` expects `IntS64`, found `IntU8`",
                "operator `==` expects `IntU8`, found `IntS64`",
                "cannot assign `IntU8` to `d`, which has type `IntU16`",
                "cannot assign `IntU8` to `e`, which has type `Int`",
                "operator `>=` expects `IntS64`, found `IntU8`",
            ]
        );
    }

    #[test]
    fn only_signed_integers_can_be_negated() {
        assert_eq!(
            messages_top(
                "
                fn f(byte: IntU8, small: IntS8) {
                    let a = -small
                    let b = 0 - byte
                    let c = -byte
                }
                "
            ),
            ["operator `-` expects a signed integer, found `IntU8`"]
        );
    }
}
