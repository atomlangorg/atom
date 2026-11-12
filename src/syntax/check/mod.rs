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
//! Every file is named by its path from the project root, so `dir/other.atom`
//! is `/dir/other`, and a directory of files and other directories is a module,
//! so `dir` is `/dir/`. The top level of a file can only hold
//! declarations, which are hoisted so they can be used in any order, even
//! across files. This is safe because functions can't capture values from
//! outside them, so calling one early never reads a value that hasn't been set
//! yet, and top-level values whose initialisers depend on themselves are
//! rejected.
//!
//! A file's own top-level names come before the names it imports, which
//! only clash if they're used. When they do, a name is qualified by the last
//! part of its file's path, as in `other::f`, or by the alias given in the
//! file's `use`, as in `use /dir/other as renamed`. Files can share these
//! names, as long as it isn't an alias and the name isn't used. Only a `use` can
//! contain a path.
//!
//! The standard library is a package called `stdlib`, whose files are files
//! named from the package, such as `stdlib/bool`. Every file imports all of
//! them without needing a `use`, including the standard library's own. Only the standard library can declare
//! primitive types, such as `Int`, which the compiler represents itself, and
//! whose values only come from literals and operators, and primitive
//! functions, such as `exit`, which the compiler implements itself.
//!
//! As it checks, the checker lowers the AST into an `ir::Program`. That program
//! is only returned when there are no errors, so everything in it is known.
//! Each generic type, and each of its functions, is checked once, then a
//! function is made for each set of arguments it's used with.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fmt,
    hash::{Hash, Hasher},
    iter,
    rc::Rc,
    sync::LazyLock,
};

use super::{
    ast::{
        self as ast, Arg, BinOp, ChoiceOption, Expr, ExprKind, Field, FieldValue, Function, Import,
        ImportPath, MatchArm, Mutability, Name, Pattern, PatternKind, Program, Span, Stmt,
        StmtKind, TupleElement, TypeName, UnOp,
    },
    parser::parse,
};
use crate::ir;

mod calls;
mod errors;
mod exprs;
mod functions;
mod imports;
mod instantiate;
mod patterns;
mod stmts;
#[cfg(test)]
mod tests;
mod traits;
mod types;

pub use errors::{CheckError, MemberKind};
use exprs::{Member, Receiver, Typed, call, split};
use patterns::Space;
use types::int_range;

/// Why something that must exist would, since the program is only compiled
/// once it has no errors.
const NO_ERRORS: &str = "the program has no errors";

/// The package holding the standard library.
const STDLIB_PACKAGE: &str = "stdlib";

/// The source of each file of the standard library, by its path within it.
const STDLIB_FILES: [(&str, &str); 18] = [
    ("operators", include_str!("../../stdlib/operators.atom")),
    ("bool", include_str!("../../stdlib/bool.atom")),
    ("int", include_str!("../../stdlib/int.atom")),
    ("intu8", include_str!("../../stdlib/intu8.atom")),
    ("intu16", include_str!("../../stdlib/intu16.atom")),
    ("intu32", include_str!("../../stdlib/intu32.atom")),
    ("intu64", include_str!("../../stdlib/intu64.atom")),
    ("ints8", include_str!("../../stdlib/ints8.atom")),
    ("ints16", include_str!("../../stdlib/ints16.atom")),
    ("ints32", include_str!("../../stdlib/ints32.atom")),
    ("ints64", include_str!("../../stdlib/ints64.atom")),
    ("void", include_str!("../../stdlib/void.atom")),
    ("never", include_str!("../../stdlib/never.atom")),
    ("exit", include_str!("../../stdlib/exit.atom")),
    ("call", include_str!("../../stdlib/call.atom")),
    ("functions", include_str!("../../stdlib/functions.atom")),
    ("choices", include_str!("../../stdlib/choices.atom")),
    ("option", include_str!("../../stdlib/option.atom")),
];

/// The source of the standard library's file shown as `file`, such as
/// `stdlib/bool`, for showing where its errors are.
pub fn stdlib_source(file: &str) -> Option<&'static str> {
    let path = file.strip_prefix(STDLIB_PACKAGE)?.strip_prefix('/')?;
    let (_, src) = STDLIB_FILES.iter().find(|(name, _)| *name == path)?;
    Some(src)
}

/// The files of the standard library, by their paths within it.
static STDLIB: LazyLock<Vec<File>> = LazyLock::new(|| {
    STDLIB_FILES
        .into_iter()
        .map(|(name, src)| File {
            path: vec![name.to_string()],
            program: parse(src)
                .unwrap_or_else(|errs| panic!("`stdlib/{name}` should parse: {errs:?}")),
        })
        .collect()
});

/// A parsed `.atom` file.
#[derive(Debug, Clone)]
pub struct File {
    /// The file's path from the project root without `.atom`, such as
    /// `["dir", "other"]` for `dir/other.atom`.
    pub path: Vec<String>,
    pub program: Program,
}

/// Every type is a group, a choice or a primitive. Types in different scopes
/// can share a name, and aliases give a type more names, so they're told
/// apart by `id`, and `name` is only for messages. It's the name the type was
/// found by, so a type found through an alias is named after the alias.
///
/// A tuple type is only the labels and types of its elements, which are what
/// it's built with, so the names its elements are read by are like `name`:
/// they're the ones it was found with, as in `(at x: Int)`. So `parts` are
/// its parts as it was found, which only differ from the ones it was first
/// made with in those names, or `None` if they're the ones it was made with.
#[derive(Debug, Clone)]
pub struct Type {
    id: usize,
    name: String,
    parts: Option<Rc<Parts>>,
}

/// The parts of a type as it was found, as `Type::parts` says.
#[derive(Debug)]
struct Parts {
    /// The types of a tuple's elements, or the arguments of the generic type
    /// it's made from.
    types: Vec<Type>,
    /// The names a tuple's elements are read by, which is empty for other
    /// types.
    names: Vec<Option<String>>,
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Type {}

impl Hash for Type {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// An error, along with the file it's in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError {
    /// The file's path, such as `/dir/other`, or `stdlib`.
    pub file: String,
    /// Where it is in the file's source.
    pub span: Span,
    pub error: CheckError,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.error)
    }
}

/// Checks every file in a project, along with the standard library,
/// returning the program they make up.
pub fn check(files: &[File]) -> Result<ir::Program, Vec<FileError>> {
    let mut checker = Checker::default();
    checker.add_stdlib();
    for file in files {
        checker.add_file(None, file);
    }
    checker.check_files();
    let instantiated = if checker.errors.is_empty() {
        checker.instantiate()
    } else {
        None
    };
    match instantiated {
        Some(instantiated) if checker.errors.is_empty() => Ok(checker.program(instantiated)),
        _ => Err(checker.sorted_errors()),
    }
}

#[derive(Debug, Clone)]
struct Signature<'a> {
    /// Its full name, as in `ir::Function::name`.
    name: String,
    params: Vec<ParamSignature<'a>>,
    /// `None` if the return type is unknown.
    returns: Option<Type>,
    /// The generic type whose `impl` declares it, as an index into
    /// `Checker::templates`, if it's declared in one, including inside one
    /// of its functions. Its types can use the generic type's parameters,
    /// which `FnRef::args` give.
    template: Option<usize>,
}

/// A function, which a call or a function used as a value refers to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FnRef {
    /// Its index in `Checker::functions`.
    index: usize,
    /// What the parameters of its generic type stand for, if it has one, in
    /// the order they're declared.
    args: Vec<Type>,
}

/// A parameter of something that's called, which is an element of the
/// tuple its arguments build.
#[derive(Debug, Clone)]
struct ParamSignature<'a> {
    label: Option<String>,
    /// What it's called in its function, if it's declared with a name.
    name: Option<&'a str>,
    span: Span,
    /// `None` if the parameter's type is unknown.
    ty: Option<Type>,
}

impl<'a> ParamSignature<'a> {
    /// The parameter declared as `element`, whose type is `ty`.
    fn declared(element: &'a TupleElement, ty: Option<Type>) -> Self {
        Self {
            label: element.label.clone(),
            name: element.name.as_deref(),
            span: element.span,
            ty,
        }
    }
}

/// An element of a tuple type as `Checker::tuple_ids` tells them apart: its
/// label and type `id`.
type TupleKey = (Option<String>, usize);

/// An element of a tuple type, which is built with its label.
#[derive(Debug, Clone)]
struct TupleElementType {
    label: Option<String>,
    ty: Type,
}

#[derive(Debug, Clone)]
struct FieldSignature<'a> {
    name: &'a str,
    /// `None` if the field's type is unknown.
    ty: Option<Type>,
}

#[derive(Debug, Clone)]
struct OptionSignature<'a> {
    /// Where it's declared, which gives its name.
    decl: &'a ChoiceOption,
    /// The type of the value it holds, or `None` if it's unknown.
    ty: Option<Type>,
}

/// What's called with arguments: a function, a method on a value, or a
/// value, through the standard library's `Call` trait.
#[derive(Debug, Clone)]
enum Callee {
    Function(FnRef),
    /// `function` called on `receiver`, which is its first argument.
    Method {
        function: FnRef,
        receiver: ir::Expr,
    },
    /// `value`, whose `call` takes a tuple `input`, which the arguments
    /// build. `call` and `returns` are `None` if they're unknown.
    Value {
        value: ir::Expr,
        call: Option<FnRef>,
        input: Type,
        returns: Option<Type>,
    },
}

/// What a type has by name, rather than its values: a static function, or
/// an option if it's a choice, as an index in the order they're declared.
#[derive(Debug, Clone)]
enum StaticMember {
    Function(FnRef),
    Option(usize),
}

impl StaticMember {
    fn kind(&self) -> MemberKind {
        match self {
            Self::Function(_) => MemberKind::StaticFunction,
            Self::Option(_) => MemberKind::Option,
        }
    }
}

/// What's called on a generic type whose name leaves out its arguments, so
/// they're worked out from the call: a static function, as in `Box.of(1)`,
/// or a method of a static member used as a value, as in
/// `Optional.some.new(1)`.
#[derive(Debug, Clone, Copy)]
enum GenericCall<'s> {
    Static(&'s str),
    Member { member: &'s str, method: &'s str },
}

/// Everything known about a type, which `Type::id` indexes.
struct TypeInfo<'a> {
    /// Its full name, such as `/main::Point`.
    name: String,
    /// The index in `Checker::files` of the file that declares it, which is
    /// the only one that can construct it if it's a group.
    file: usize,
    kind: TypeKind<'a>,
    /// Its methods by name. Methods aren't names in any scope, and are only
    /// found through the type of the value they're called on, wherever
    /// they're declared. They share names with the type's fields.
    methods: HashMap<&'a str, usize>,
    /// Its static functions by name, which are found like methods but on the
    /// type itself. They have their own names, separate from the type's
    /// fields and methods.
    statics: HashMap<&'a str, usize>,
    /// The generic type it's made from, as an index into `Checker::templates`,
    /// and its arguments, if it's made from one. Its methods and static
    /// functions are those of the type made from the generic type with its
    /// own parameters, with the arguments standing for them.
    instance: Option<(usize, Vec<Type>)>,
}

/// What a type is made of.
#[derive(Clone)]
enum TypeKind<'a> {
    /// A group's fields, in the order they're declared. A type that isn't a
    /// choice is a group without fields until it's defined.
    Group(Vec<FieldSignature<'a>>),
    /// A choice's options, in the order they're declared, which it has none
    /// of until it's defined.
    Choice(Vec<OptionSignature<'a>>),
    /// A type that the compiler represents itself.
    Primitive(ir::Primitive),
    /// A tuple's elements.
    Tuple(Vec<TupleElementType>),
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
    file: usize,
    params: &'a [String],
    /// The group, choice or primitive declaring it.
    stmt: &'a Stmt,
    /// A type for each of `params`, which can't be used as anything else.
    stand_ins: Vec<Type>,
    /// The type made from it with `stand_ins` as its arguments, once it's
    /// been defined. It's checked like any other type, so its mistakes are
    /// reported once. Other types made from it are made by putting their
    /// arguments in place of `stand_ins`, and share its functions.
    generic: Option<Type>,
}

/// A trait, whose functions an `impl` with it must implement exactly.
#[derive(Clone)]
struct TraitInfo<'a> {
    name: &'a str,
    span: Span,
    file: usize,
    /// The names of its aliases, such as `Rhs`, whose types each
    /// implementation gives.
    aliases: &'a [ast::Alias],
    functions: &'a [Function],
    /// A type standing for `Self`, then one for each of `aliases`, since
    /// they're a different type in each `impl`.
    stand_ins: Vec<Type>,
    /// For each of `functions`, its parameter types and return type, which
    /// are `None` if they're unknown, and use `stand_ins`. Worked out once
    /// types are defined.
    signatures: Vec<(Vec<Option<Type>>, Option<Type>)>,
}

/// An `impl ty with trait_name`, whose functions are checked against the
/// trait at `index` in `Checker::traits`.
struct Implementation<'b, 'a> {
    ty: &'b Name,
    /// The type `ty` refers to.
    self_ty: &'b Type,
    trait_name: &'b Name,
    index: usize,
    aliases: &'a [ast::Alias],
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

/// A value declared at the top level of a file. Its type is worked out when
/// it's first needed, since top-level values can be used in any order.
struct Global<'a> {
    file: usize,
    name: &'a str,
    span: Span,
    ty: Option<&'a TypeName>,
    value: &'a Expr,
    state: LazyType,
    /// What its initialiser compiles to, once it's been checked.
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
    file: usize,
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

struct FileInfo<'a> {
    /// The file's path, such as `/dir/other` or `stdlib/bool`.
    display: String,
    /// The package the file is in, or `None` if it's in the project.
    package: Option<&'a str>,
    /// The file's path from the root of its package or the project.
    path: &'a [String],
    /// The name used to qualify the file's names, such as `other` in
    /// `other::f`, unless a `use` gives it an alias.
    name: &'a str,
    program: &'a Program,
    /// The names declared at the file's top level.
    scope: HashMap<&'a str, Binding>,
    /// The files whose top-level names this one can use, starting with this
    /// file itself.
    imports: Vec<ImportedFile<'a>>,
}

/// A file whose top-level names another file can use.
struct ImportedFile<'a> {
    /// The name that qualifies its names, such as `other` in `other::f`.
    name: &'a str,
    /// Its index in `Checker::files`.
    file: usize,
    /// The `use` that gave it an alias, if it has one.
    span: Option<Span>,
}

/// Why `Checker::lookup` couldn't find a name.
enum LookupError {
    Undeclared,
    Captured,
    /// The name was found in each of these files.
    Ambiguous(Vec<String>),
    /// The name's file couldn't be found.
    File(Box<CheckError>),
    /// The name is an alias whose type couldn't be worked out, or `Self` in
    /// an `impl` whose type is unknown, which has already been reported.
    Reported,
    /// The name is `Self`, outside an `impl`.
    SelfTypeOutsideImpl,
    /// The name is an `impl`'s alias that's still being inferred.
    InferredAlias,
}

/// A top-level declaration that still needs checking once every file's
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
    },
}

/// Where the code being checked is, which decides what its names refer to.
#[derive(Default)]
struct Context<'a> {
    /// The index in `Checker::files` of the file it's in.
    file: usize,
    /// One scope per enclosing block inside functions, innermost last. Empty
    /// at the top level, where names are in the file's own scope. Values,
    /// functions and types all share the same names.
    scopes: Vec<HashMap<&'a str, Binding>>,
    /// The index in `scopes` of the current function's outermost scope. Names
    /// can't be redeclared from here inwards.
    fn_scope_start: usize,
    current_fn: Option<FnContext<'a>>,
    /// The type whose `impl` it's in, including inside functions nested in
    /// its methods. Only it can be constructed. `Some(None)` if the `impl`'s
    /// type is unknown.
    impl_type: Option<Option<Type>>,
    /// The index in `Checker::impl_aliases` of the aliases it can use, if
    /// it's in an `impl` that gives any, including inside functions nested
    /// in its methods.
    impl_aliases: Option<usize>,
    /// While a generic type is defined, its index in `Checker::templates`,
    /// and the stand-ins for its parameters.
    type_params: Option<(usize, Vec<Type>)>,
}

#[derive(Default)]
struct Checker<'a> {
    /// The standard library first, then the project's files.
    files: Vec<FileInfo<'a>>,
    /// Every type, indexed by `id`.
    types: Vec<TypeInfo<'a>>,
    /// Where the code being checked is.
    cx: Context<'a>,
    /// The aliases of each `impl` with a trait, by name, whether they're
    /// given, such as `alias Rhs = Self`, or inferred.
    impl_aliases: Vec<HashMap<&'a str, ImplAliasType>>,
    functions: Vec<Signature<'a>>,
    traits: Vec<TraitInfo<'a>>,
    templates: Vec<Template<'a>>,
    /// The `id` of each type made from a generic type, by the index in
    /// `templates` of what it's made from and its arguments' `id`s.
    instance_ids: HashMap<(usize, Vec<usize>), usize>,
    /// The types made from generic types whose fields or options haven't
    /// been made from the generic type's yet, since it hasn't been defined.
    unfilled: Vec<usize>,
    /// Whether `fill_instances` is running, so it isn't started again.
    filling: bool,
    /// The values and function bodies still to check, with their files.
    pending: Vec<(usize, Pending<'a>)>,
    /// The functions that are called or used as values, which
    /// `ir::Expr::Call` and `ir::Expr::Function` index until `instantiate`
    /// replaces them with a function for each set of arguments. Each has the
    /// file and place it's first used.
    callees: Vec<(FnRef, usize, Span)>,
    /// The index of each of `callees`.
    callee_ids: HashMap<FnRef, usize>,
    /// Which types implement which traits, as a type `id` and an index into
    /// `traits`.
    implementations: HashSet<(usize, usize)>,
    /// For each of `functions`, what it compiles to, once its body has been
    /// checked, or `None` if it has an error.
    lowered_functions: Vec<Option<ir::Function>>,
    /// Whether a `Never` has been evaluated since `evaluating` started
    /// checking some expressions.
    evaluated_never: bool,
    globals: Vec<Global<'a>>,
    aliases: Vec<Alias<'a>>,
    /// Indexes into `globals`, in the order their types were worked out, so
    /// that each comes after the values its initialiser uses.
    global_order: Vec<usize>,
    /// The `id` of each tuple type, by its elements' labels, names and type
    /// `id`s. Tuples are the same type only if all of those are the same.
    tuple_ids: HashMap<Vec<TupleKey>, usize>,
    errors: Vec<FileError>,
}

impl<'a> Checker<'a> {
    /// Adds a file from `package`, or from the project if that's `None`.
    /// Returns its index in `files`.
    fn add_file(&mut self, package: Option<&'a str>, file: &'a File) -> usize {
        let path = file.path.join("/");
        let display = match package {
            Some(package) => format!("{package}/{path}"),
            None => format!("/{path}"),
        };
        self.files.push(FileInfo {
            display,
            package,
            path: &file.path,
            name: file.path.last().expect("file paths aren't empty"),
            program: &file.program,
            scope: HashMap::new(),
            imports: vec![],
        });
        self.files.len() - 1
    }
    /// Adds the standard library's files.
    fn add_stdlib(&mut self) {
        for file in STDLIB.iter() {
            self.add_file(Some(STDLIB_PACKAGE), file);
        }
    }
    /// The errors, in the order of the files they're in, and of where they
    /// are in each file. Errors in the same place stay in the order they
    /// were found.
    fn sorted_errors(self) -> Vec<FileError> {
        let file_order: HashMap<_, _> = self
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.display.as_str(), index))
            .collect();
        let mut errors = self.errors;
        errors.sort_by_key(|error| (file_order[error.file.as_str()], error.span.start));
        errors
    }
    fn error(&mut self, span: Span, error: CheckError) {
        self.error_in(self.cx.file, span, error);
    }
    fn error_in(&mut self, file: usize, span: Span, error: CheckError) {
        let file = self.files[file].display.clone();
        self.errors.push(FileError { file, span, error });
    }
    /// Checks every file. Each kind of top-level declaration is declared in
    /// every file before the next kind, and before anything is checked, so
    /// top-level names can be used in any order.
    fn check_files(&mut self) {
        for file in 0..self.files.len() {
            self.cx.file = file;
            self.resolve_imports();
        }

        // Every type and alias is declared before any are resolved, so types
        // can contain each other, and aliases can refer to each other, in any
        // order.
        let mut types = vec![];
        for file in 0..self.files.len() {
            self.cx.file = file;
            for stmt in self.files[file].program {
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
                        let id = self.declare_type(name, stmt);
                        types.push((file, id, stmt));
                    }
                    StmtKind::Alias(ast::Alias {
                        name, ty: Some(ty), ..
                    }) => {
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
        for &(file, id, stmt) in &types {
            self.cx.file = file;
            self.define_type(id, stmt);
        }
        // Each generic type is checked once, with stand-ins for its
        // parameters, so its errors are reported once.
        let mut checked = vec![];
        for template in 0..self.templates.len() {
            self.cx.file = self.templates[template].file;
            checked.extend(self.check_template(template));
        }
        for index in 0..self.traits.len() {
            self.define_trait(index);
        }
        for &(file, id, stmt) in &types {
            self.cx.file = file;
            if let StmtKind::Group { name, .. } | StmtKind::Choice { name, .. } = &stmt.kind {
                self.check_not_recursive(id, name, stmt.span);
            }
        }
        for (file, ty, span) in checked {
            self.cx.file = file;
            self.check_not_recursive(ty.id, &ty.name, span);
        }

        // Functions, methods and values come after groups, since their types
        // can be groups.
        for file in 0..self.files.len() {
            self.cx.file = file;
            for stmt in self.files[file].program {
                match &stmt.kind {
                    StmtKind::Fn(function) => {
                        let index = self.declare_fn(
                            &function.name,
                            function.span,
                            &function.params,
                            &function.ret,
                        );
                        // Only a `primitive fn` has no body here.
                        let Some(body) = &function.body else {
                            let (name, span) = (&function.name, function.span);
                            self.define_primitive_fn(index, None, name, span, FnKind::Function);
                            continue;
                        };
                        self.pending.push((
                            file,
                            Pending::Fn {
                                index,
                                name: &function.name,
                                span: function.span,
                                body,
                                kind: FnKind::Function,
                                aliases: None,
                            },
                        ));
                    }
                    StmtKind::Impl {
                        ty,
                        with,
                        aliases,
                        functions,
                    } => {
                        // A generic type's `impl` is for the type made from
                        // it with its own parameters, whose functions every
                        // type made from it shares.
                        let self_ty = match self.lookup(ty) {
                            Ok(Binding::Generic(template)) => {
                                self.templates[template].generic.clone()
                            }
                            _ => self.resolve_named_type(ty),
                        };
                        let declared =
                            self.declare_impl(ty, self_ty, with.as_ref(), aliases, functions);
                        self.pending.extend(declared);
                    }
                    StmtKind::Decl {
                        name, ty, value, ..
                    } => {
                        self.globals.push(Global {
                            file,
                            name,
                            span: stmt.span,
                            ty: ty.as_ref(),
                            value,
                            state: LazyType::Unchecked,
                            lowered: None,
                        });
                        let index = self.globals.len() - 1;
                        self.declare(name, stmt.span, Binding::Global(index));
                        self.pending.push((file, Pending::Global(index)));
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

        for (file, pending) in std::mem::take(&mut self.pending) {
            self.cx.file = file;
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
                } => self.fn_body(index, name, span, body, kind, aliases),
            }
        }
    }
    /// Whether `name` is declared anywhere the current code could see it,
    /// even if it's a value that a function can't use.
    fn is_declared(&self, name: &str) -> bool {
        self.cx.scopes.iter().any(|scope| scope.contains_key(name))
            || self.files[self.cx.file]
                .imports
                .iter()
                .any(|import| self.files[import.file].scope.contains_key(name))
    }
    /// What the standard library declares as `name`, which is found in the
    /// standard library itself, so it can't be shadowed.
    fn std_binding(&self, name: &str) -> Option<&Binding> {
        self.files
            .iter()
            .filter(|file| file.package == Some(STDLIB_PACKAGE))
            .find_map(|file| file.scope.get(name))
    }

    /// A generic type from the standard library, such as `Fn`, as an index
    /// into `templates`.
    fn std_template(&self, name: &str) -> usize {
        match self.std_binding(name) {
            Some(Binding::Generic(template)) => *template,
            _ => panic!("the standard library should declare `{name}`"),
        }
    }
    /// A type from the standard library, such as `Int` for integer literals.
    fn std_type(&self, name: &str) -> Type {
        let ty = match self.std_binding(name) {
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
            parts: None,
        }
    }
    /// Finds what `name` refers to. An alias is found as the type it's an
    /// alias of, which is worked out if it hasn't been yet.
    fn lookup(&mut self, name: &Name) -> Result<Binding, LookupError> {
        // `Self` is a keyword, so it's never declared, and always refers to
        // the type of the `impl` it's in.
        if name.is_self() {
            return match &self.cx.impl_type {
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
        let binding = match &name.file {
            Some(path) => {
                let file = self.resolve_qualifier(path)?;
                self.files[file]
                    .scope
                    .get(name.name.as_str())
                    .ok_or(LookupError::Undeclared)?
            }
            None => {
                // While a type made from a generic type is defined, its
                // parameters stand for its arguments.
                if let Some((template, args)) = &self.cx.type_params
                    && let Some(index) = self.templates[*template]
                        .params
                        .iter()
                        .position(|param| *param == name.name)
                {
                    return Ok(Binding::Type(args[index].clone()));
                }
                let local = self
                    .cx
                    .scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, scope)| scope.get(name.name.as_str()).map(|b| (index, b)));
                match local {
                    // Functions can't capture values from outside them.
                    // Functions and types can still be used, since they never
                    // depend on a value.
                    Some((index, Binding::Value { .. })) if index < self.cx.fn_scope_start => {
                        return Err(LookupError::Captured);
                    }
                    Some((_, binding)) => return Ok(binding.clone()),
                    None => {}
                }

                // Then the aliases of the `impl` it's in.
                if let Some(aliases) = self.cx.impl_aliases
                    && let Some(ty) = self.impl_aliases[aliases].get(name.name.as_str())
                {
                    return match ty {
                        ImplAliasType::Known(ty) => {
                            ty.clone().map(Binding::Type).ok_or(LookupError::Reported)
                        }
                        ImplAliasType::Inferring => Err(LookupError::InferredAlias),
                    };
                }

                // The current file's own names come first. Otherwise, a name
                // that isn't qualified must only be declared in one of the
                // files it imports.
                if let Some(binding) = self.files[self.cx.file].scope.get(name.name.as_str()) {
                    return self.top_level(binding);
                }
                let found: Vec<_> = self.files[self.cx.file].imports[1..]
                    .iter()
                    .filter_map(|import| {
                        let binding = self.files[import.file].scope.get(name.name.as_str())?;
                        Some((import.file, binding))
                    })
                    .collect();
                match found[..] {
                    [] => return Err(LookupError::Undeclared),
                    [(_, binding)] => binding,
                    _ => {
                        let files = found
                            .iter()
                            .map(|&(file, _)| self.files[file].display.clone())
                            .collect();
                        return Err(LookupError::Ambiguous(files));
                    }
                }
            }
        };

        self.top_level(binding)
    }
    /// Checks that a file's top-level `binding` can be used here. Top-level
    /// values are outside every function.
    fn top_level(&self, binding: &Binding) -> Result<Binding, LookupError> {
        if matches!(binding, Binding::Global(_)) && self.cx.current_fn.is_some() {
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
            LookupError::Ambiguous(files) => CheckError::Ambiguous {
                name: name_string,
                files,
            },
            LookupError::File(error) => *error,
            LookupError::Reported => return,
            LookupError::SelfTypeOutsideImpl => CheckError::SelfTypeOutsideImpl,
            LookupError::InferredAlias => CheckError::InferredAliasUsed(name_string),
        };
        self.error(name.span, error);
    }
    fn declare(&mut self, name: &'a str, span: Span, binding: Binding) {
        // Names can't be redeclared within a function, even to shadow an
        // outer block. A function's own scope can reuse names from outside it.
        // At the top level, names only need to be unique within their file.
        let scope = match self.cx.scopes.last_mut() {
            Some(_) => {
                if self.cx.scopes[self.cx.fn_scope_start..]
                    .iter()
                    .any(|scope| scope.contains_key(name))
                {
                    self.error(span, CheckError::Redeclared(name.to_string()));
                }
                self.cx.scopes.last_mut().expect("checked above")
            }
            None => {
                if self.files[self.cx.file].scope.contains_key(name) {
                    self.error(span, CheckError::Redeclared(name.to_string()));
                }
                &mut self.files[self.cx.file].scope
            }
        };
        scope.insert(name, binding);
    }
    /// Runs `check` as if it were at the top level of `file`, outside any
    /// function or `impl`, with `type_params` as `Context::type_params`.
    fn at_top_level<T>(
        &mut self,
        file: usize,
        type_params: Option<(usize, Vec<Type>)>,
        check: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let context = Context {
            file,
            type_params,
            ..Context::default()
        };
        let outer = std::mem::replace(&mut self.cx, context);
        let checked = check(self);
        self.cx = outer;
        checked
    }
    /// Runs `check` as if it were in an `impl` of `ty`, which is `None` if
    /// it's unknown, that can use the aliases at `aliases` in `impl_aliases`.
    fn in_impl<T>(
        &mut self,
        ty: Option<Type>,
        aliases: Option<usize>,
        check: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let outer_type = self.cx.impl_type.replace(ty);
        let outer_aliases = std::mem::replace(&mut self.cx.impl_aliases, aliases);
        let checked = check(self);
        self.cx.impl_type = outer_type;
        self.cx.impl_aliases = outer_aliases;
        checked
    }
    /// The full name of something called `name` declared here, as in
    /// `ir::Function::name`. Names inside a function start with the
    /// function's own.
    fn full_name(&self, name: &str) -> String {
        let outer = match &self.cx.current_fn {
            Some(function) => &function.full_name,
            None => &self.files[self.cx.file].display,
        };
        format!("{outer}::{name}")
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
        let local = self.add_local(ty.clone());
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
    /// Adds a local of type `ty` to the current function, without a name,
    /// returning its index.
    fn add_local(&mut self, ty: Option<Type>) -> usize {
        let locals = &mut self
            .cx
            .current_fn
            .as_mut()
            .expect("values are only declared inside functions")
            .locals;
        locals.push(ty);
        locals.len() - 1
    }
    /// Creates a `Void`, which has no fields.
    fn void(&self) -> ir::Expr {
        ir::Expr::Construct {
            group: self.std_type("Void").id,
            fields: vec![],
        }
    }
}
