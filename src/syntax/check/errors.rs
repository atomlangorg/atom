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

//! The errors the checker reports, and their messages.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A name was used or assigned without being declared.
    Undeclared(String),
    /// A function used a value declared outside it.
    Captured(String),
    /// A name without a file was declared in more than one of the files
    /// it could come from.
    Ambiguous { name: String, files: Vec<String> },
    /// A `use` named a file that doesn't exist.
    UnknownFile(String),
    /// A `use` named a module that doesn't exist.
    UnknownModule(String),
    /// A `use` named a module without the slash that imports its files.
    ModuleNotFile { path: String, aliased: bool },
    /// A `use` named a path with more `../` than there are directories above
    /// the current file.
    PathAboveRoot(String),
    /// A `use` named a file with the slash that imports a module's files.
    FileNotModule(String),
    /// A `use` named something that exists, but without the `./` or `/` that
    /// every path in the project starts with, given what it could have meant.
    PathWithoutStart(Vec<String>),
    /// A qualified name's file isn't the current file or one it imports.
    NotImported(String),
    /// A `use` gave a file an alias when it already has a different one.
    FileAliasedTwice { file: String, alias: String },
    /// A `use` of a module found no files that could be imported, since the
    /// only one in it is the file importing it.
    EmptyModule(String),
    /// A package was used where one of its files was needed.
    PackageNotFile(String),
    /// A primitive type was constructed, such as `Int`, whose values come
    /// from literals.
    ConstructedPrimitive(Type),
    /// A primitive type was declared outside the standard library.
    PrimitiveOutsideStdlib(String),
    /// A group was constructed outside the file that declares it.
    ConstructedOutsideFile { ty: Type, file: String },
    /// `::` was used after a name that's declared, but isn't a file.
    NotAFile(String),
    /// An alias is the name of another file too, or a qualified name's file
    /// is the name of more than one file.
    FileNameClash { name: String, files: Vec<String> },
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
        /// The argument's label, or the parameter name if it has none, or
        /// `None` if it has neither, so it's named by its `position`, which
        /// starts at 1.
        param: Option<String>,
        position: usize,
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
    /// `.` was used after a file's name, where `::` was probably meant.
    FileNotValue { file: String, member: String },
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
    /// An operator was used on a type that doesn't implement the trait from
    /// the standard library that it calls a function of, such as `Add` for
    /// `+`.
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
            Self::Ambiguous { name, files } => write!(
                f,
                "`{name}` is declared in {}, so it must be qualified",
                quoted_list(files, "and")
            ),
            Self::UnknownFile(path) => write!(f, "cannot find file `{path}`"),
            Self::UnknownModule(path) => write!(f, "cannot find module `{path}`"),
            Self::ModuleNotFile {
                path,
                aliased: false,
            } => write!(
                f,
                "`{path}` is a module, so use `{path}/` to import every file in it"
            ),
            Self::ModuleNotFile {
                path,
                aliased: true,
            } => write!(
                f,
                "`{path}` is a module, so it can't have an alias, but `{path}/` imports every file in it"
            ),
            Self::PathAboveRoot(path) => write!(f, "`{path}` goes above the root directory"),
            Self::FileNotModule(path) => {
                write!(
                    f,
                    "`{path}` is a file, so use it without the trailing slash"
                )
            }
            Self::PathWithoutStart(paths) => write!(
                f,
                "paths start with `./` or `/`, so use {}",
                quoted_list(paths, "or")
            ),
            Self::NotImported(path) => write!(f, "`{path}` is not an imported file"),
            Self::FileAliasedTwice { file, alias } => {
                write!(f, "`{file}` is already imported as `{alias}`")
            }
            Self::EmptyModule(import) => {
                write!(
                    f,
                    "`{import}` doesn't contain any files that can be imported"
                )
            }
            Self::PackageNotFile(package) => write!(
                f,
                "`{package}` is a package, so use one of its files, such as `{package}/bool`"
            ),
            Self::ConstructedOutsideFile { ty, file } => write!(
                f,
                "`{ty}` can only be constructed in `{file}`, which declares it"
            ),
            Self::ConstructedPrimitive(ty) => write!(
                f,
                "`{ty}` is a primitive, so it can only be created by the compiler, such as with a literal"
            ),
            Self::PrimitiveOutsideStdlib(name) => write!(
                f,
                "`{name}` can't be a primitive, since only the standard library can declare them"
            ),
            Self::NotAFile(name) => {
                write!(f, "`{name}` is not a file, so it can't be followed by `::`")
            }
            Self::FileNameClash { name, files } => write!(
                f,
                "`{name}` is the name of {}, so use `as` to give them different names",
                quoted_list(files, "and")
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
                position,
                expected,
                found,
            } => match param {
                Some(param) => write!(
                    f,
                    "argument `{param}` of `{function}` expects `{expected}`, found `{found}`"
                ),
                None => write!(
                    f,
                    "argument {position} of `{function}` expects `{expected}`, found `{found}`"
                ),
            },
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
            Self::FileNotValue { file, member } => write!(
                f,
                "`{file}` is a file, not a value, so use `{file}::{member}` to refer to a name in it"
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
