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

//! Back end of the compiler: turns a checked program into LLVM IR, then into
//! a native executable.
//!
//! Every group is an LLVM struct of its fields, and the builtin integer and
//! boolean are `i64` and `i1`, so `Int` is `{ i64 }` and `Void` is `{}`. Every
//! Atom function becomes an internal LLVM function, so only the C `main` is
//! visible to the linker. It sets every top-level value, then calls Atom's
//! `main`, whose result is the exit code.
//!
//! Arithmetic that overflows, and division by zero, traps by calling C's
//! `abort`. `llvm.trap` would avoid needing C, but on macOS it can leave the
//! process hanging instead of stopping it, depending on what launched it.

use std::{fmt, io, path::Path, process::Command};

use inkwell::{
    IntPredicate::{EQ, NE},
    OptimizationLevel,
    basic_block::BasicBlock,
    builder::{Builder, BuilderError},
    context::Context,
    intrinsics::Intrinsic,
    module::{Linkage, Module},
    targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine},
    types::{BasicType, BasicTypeEnum},
    values::{BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue, PointerValue},
};

use crate::{
    ir::{Builtin, Expr, Group, Program, Stmt},
    syntax::ast::{BinOp, UnOp},
};

/// Compiles `program` into an LLVM module. The module only has a C `main`,
/// so it can only be linked into an executable, if `program.main` is set.
///
/// Panics if it builds invalid LLVM IR, which would be a bug in the compiler.
pub fn compile<'ctx>(context: &'ctx Context, program: &Program) -> Module<'ctx> {
    let mut codegen = Codegen::new(context, program);
    codegen
        .build()
        .expect("instructions are only built inside a block");
    if let Err(err) = codegen.module.verify() {
        panic!(
            "built invalid LLVM IR: {}\n{}",
            err.to_string(),
            codegen.module.print_to_string().to_string()
        );
    }
    codegen.module
}

/// Compiles `module` for this computer, then links it into an executable at
/// `path` with the system's C compiler, `cc`.
pub fn write_executable(module: &Module, path: &Path) -> Result<(), EmitError> {
    Target::initialize_native(&InitializationConfig::default()).map_err(EmitError::Target)?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|err| EmitError::Target(err.to_string()))?;
    let machine = target
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string(),
            &TargetMachine::get_host_cpu_features().to_string(),
            OptimizationLevel::Default,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| EmitError::Target(format!("no target machine for {triple}")))?;
    module.set_triple(&triple);
    module.set_data_layout(&machine.get_target_data().get_data_layout());

    let object = path.with_extension("o");
    machine
        .write_to_file(module, FileType::Object, &object)
        .map_err(|err| EmitError::Object(err.to_string()))?;
    let linked = Command::new("cc").arg(&object).arg("-o").arg(path).status();
    // The object file is only needed for linking, so failing to remove it
    // doesn't stop the executable from working.
    let _ = std::fs::remove_file(&object);

    let status = linked.map_err(EmitError::Link)?;
    if !status.success() {
        return Err(EmitError::LinkFailed(status.to_string()));
    }
    Ok(())
}

#[derive(Debug)]
pub enum EmitError {
    /// LLVM couldn't compile for this computer.
    Target(String),
    /// LLVM couldn't write the object file.
    Object(String),
    /// `cc` couldn't be run.
    Link(io::Error),
    /// `cc` ran but failed, with the status it exited with.
    LinkFailed(String),
}

impl fmt::Display for EmitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Target(err) => write!(f, "couldn't compile for this computer: {err}"),
            Self::Object(err) => write!(f, "couldn't write the object file: {err}"),
            Self::Link(err) => write!(f, "couldn't run `cc` to link: {err}"),
            Self::LinkFailed(status) => write!(f, "`cc` failed to link, with {status}"),
        }
    }
}

struct Codegen<'a, 'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    program: &'a Program,
    /// For each group in `program`, the LLVM type it's represented by.
    types: Vec<BasicTypeEnum<'ctx>>,
    /// For each function in `program`, the LLVM function it compiles to.
    functions: Vec<FunctionValue<'ctx>>,
    /// For each top-level value in `program`, the LLVM global holding it.
    globals: Vec<GlobalValue<'ctx>>,
    /// Where each local of the function being built is stored, along with its
    /// type.
    locals: Vec<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    /// The block in the function being built that traps, once something
    /// needs it.
    trap: Option<BasicBlock<'ctx>>,
}

impl<'a, 'ctx> Codegen<'a, 'ctx> {
    fn new(context: &'ctx Context, program: &'a Program) -> Self {
        Self {
            context,
            module: context.create_module("atom"),
            builder: context.create_builder(),
            program,
            types: vec![],
            functions: vec![],
            globals: vec![],
            locals: vec![],
            trap: None,
        }
    }

    /// Builds every type, function and top-level value, then the C `main`.
    /// Everything is declared before anything uses it, so it can be used in
    /// any order.
    fn build(&mut self) -> Result<(), BuilderError> {
        self.declare_types();

        for function in &self.program.functions {
            let params: Vec<_> = function.locals[..function.params]
                .iter()
                .map(|&ty| self.types[ty].into())
                .collect();
            let fn_type = self.types[function.returns].fn_type(&params, false);
            let value = self
                .module
                .add_function(&function.name, fn_type, Some(Linkage::Internal));
            self.functions.push(value);
        }

        for global in &self.program.globals {
            let ty = self.types[global.ty];
            let value = self.module.add_global(ty, None, &global.name);
            value.set_linkage(Linkage::Internal);
            value.set_initializer(&ty.const_zero());
            self.globals.push(value);
        }

        for index in 0..self.program.functions.len() {
            self.define_function(index)?;
        }
        if let Some(main) = &self.program.main {
            self.define_main(main)?;
        }
        Ok(())
    }

    /// Creates the LLVM type for each group. Every group is named before any
    /// are given fields, since groups can hold each other in any order.
    fn declare_types(&mut self) {
        for group in &self.program.groups {
            let ty = match group {
                Group::Builtin(Builtin::Int) => self.context.i64_type().into(),
                Group::Builtin(Builtin::Bool) => self.context.bool_type().into(),
                Group::Fields { name, .. } => self.context.opaque_struct_type(name).into(),
            };
            self.types.push(ty);
        }
        for (group, ty) in self.program.groups.iter().zip(&self.types) {
            if let Group::Fields { fields, .. } = group {
                let fields: Vec<_> = fields.iter().map(|&field| self.types[field]).collect();
                ty.into_struct_type().set_body(&fields, false);
            }
        }
    }

    /// Builds the body of the function at `index` in `program.functions`.
    fn define_function(&mut self, index: usize) -> Result<(), BuilderError> {
        let function = &self.program.functions[index];
        let value = self.functions[index];
        self.start(value)?;

        // Every local gets its own slot up front, so they can be set from
        // anywhere in the body.
        self.locals = function
            .locals
            .iter()
            .map(|&ty| {
                let ty = self.types[ty];
                Ok((self.builder.build_alloca(ty, "")?, ty))
            })
            .collect::<Result<_, BuilderError>>()?;
        for (&(slot, _), param) in self.locals.iter().zip(value.get_param_iter()) {
            self.builder.build_store(slot, param)?;
        }

        self.block(&function.body)?;
        // Every path returns, so the end can't be reached, such as after an
        // `if` whose branches both return.
        if !self.is_terminated() {
            self.builder.build_unreachable()?;
        }
        Ok(())
    }

    /// Builds the C `main`, which sets every top-level value, then returns
    /// the result of `main`, a builtin integer, as the exit code.
    fn define_main(&mut self, main: &Expr) -> Result<(), BuilderError> {
        let exit_code = self.context.i32_type();
        let value = self
            .module
            .add_function("main", exit_code.fn_type(&[], false), None);
        self.start(value)?;
        self.locals = vec![];

        for &index in &self.program.global_order {
            let value = self.expr(&self.program.globals[index].value)?;
            let global = self.globals[index].as_pointer_value();
            self.builder.build_store(global, value)?;
        }

        let result = self.expr(main)?.into_int_value();
        let code = self.builder.build_int_truncate(result, exit_code, "")?;
        self.builder.build_return(Some(&code))?;
        Ok(())
    }

    /// Starts building the body of `function`.
    fn start(&mut self, function: FunctionValue<'ctx>) -> Result<(), BuilderError> {
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        self.trap = None;
        Ok(())
    }

    /// The function being built.
    fn current_function(&self) -> FunctionValue<'ctx> {
        self.builder
            .get_insert_block()
            .and_then(|block| block.get_parent())
            .expect("the builder is inside a function")
    }

    /// Whether the block being built already ends, such as with a `return`.
    fn is_terminated(&self) -> bool {
        self.builder
            .get_insert_block()
            .is_some_and(|block| block.get_terminator().is_some())
    }

    fn block(&mut self, stmts: &[Stmt]) -> Result<(), BuilderError> {
        for stmt in stmts {
            // Statements after a `return` can't be reached, so they're left
            // out.
            if self.is_terminated() {
                break;
            }
            self.stmt(stmt)?;
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), BuilderError> {
        match stmt {
            Stmt::Assign { local, value } => {
                let value = self.expr(value)?;
                self.builder.build_store(self.locals[*local].0, value)?;
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond = self.expr(cond)?.into_int_value();
                let function = self.current_function();
                let then_start = self.context.append_basic_block(function, "then");
                let else_start = self.context.append_basic_block(function, "else");
                let after = self.context.append_basic_block(function, "after");
                self.builder
                    .build_conditional_branch(cond, then_start, else_start)?;

                for (start, block) in [(then_start, then_block), (else_start, else_block)] {
                    self.builder.position_at_end(start);
                    self.block(block)?;
                    if !self.is_terminated() {
                        self.builder.build_unconditional_branch(after)?;
                    }
                }
                self.builder.position_at_end(after);
            }
            Stmt::Return(value) => {
                let value = self.expr(value)?;
                self.builder.build_return(Some(&value))?;
            }
            Stmt::Expr(expr) => {
                self.expr(expr)?;
            }
        }
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        Ok(match expr {
            Expr::Int(value) => self
                .context
                .i64_type()
                .const_int(*value as u64, true)
                .into(),
            Expr::Bool(value) => self
                .context
                .bool_type()
                .const_int(*value as u64, false)
                .into(),
            Expr::Local(local) => {
                let (slot, ty) = self.locals[*local];
                self.builder.build_load(ty, slot, "")?
            }
            Expr::Global(index) => {
                let ty = self.types[self.program.globals[*index].ty];
                let global = self.globals[*index].as_pointer_value();
                self.builder.build_load(ty, global, "")?
            }
            Expr::Unary(op, operand) => {
                let operand = self.expr(operand)?.into_int_value();
                match op {
                    // Negating the smallest integer overflows.
                    UnOp::Neg => {
                        let zero = self.context.i64_type().const_zero();
                        self.checked("llvm.ssub.with.overflow", zero, operand)?
                    }
                    UnOp::Not => self.builder.build_not(operand, "")?,
                }
                .into()
            }
            Expr::Binary(lhs, op, rhs) => {
                let lhs = self.expr(lhs)?.into_int_value();
                let rhs = self.expr(rhs)?.into_int_value();
                match op {
                    BinOp::Add => self.checked("llvm.sadd.with.overflow", lhs, rhs)?,
                    BinOp::Sub => self.checked("llvm.ssub.with.overflow", lhs, rhs)?,
                    BinOp::Mul => self.checked("llvm.smul.with.overflow", lhs, rhs)?,
                    BinOp::Div => self.divide(lhs, rhs)?,
                    BinOp::Eq => self.builder.build_int_compare(EQ, lhs, rhs, "")?,
                    BinOp::Ne => self.builder.build_int_compare(NE, lhs, rhs, "")?,
                    BinOp::And => self.builder.build_and(lhs, rhs, "")?,
                    BinOp::Or => self.builder.build_or(lhs, rhs, "")?,
                }
                .into()
            }
            Expr::Call { function, args } => {
                let args = args
                    .iter()
                    .map(|arg| Ok(self.expr(arg)?.into()))
                    .collect::<Result<Vec<_>, BuilderError>>()?;
                self.builder
                    .build_call(self.functions[*function], &args, "")?
                    .try_as_basic_value()
                    .basic()
                    .expect("every function returns a value, even if it's `Void`")
            }
            Expr::Construct { group, fields } => {
                let mut value = self.types[*group].into_struct_type().get_poison();
                for (index, field) in fields.iter().enumerate() {
                    let field = self.expr(field)?;
                    value = self
                        .builder
                        .build_insert_value(value, field, index as u32, "")?
                        .into_struct_value();
                }
                value.as_basic_value_enum()
            }
            Expr::Field { receiver, index } => {
                let receiver = self.expr(receiver)?.into_struct_value();
                self.builder
                    .build_extract_value(receiver, *index as u32, "")?
            }
        })
    }

    /// Calls `intrinsic`, an LLVM arithmetic intrinsic that reports overflow,
    /// on builtin integers, trapping if it overflows.
    fn checked(
        &mut self,
        intrinsic: &str,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let int = self.context.i64_type();
        let function = Intrinsic::find(intrinsic)
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module, &[int.into()]))
            .unwrap_or_else(|| panic!("LLVM should have `{intrinsic}`"));
        let result = self
            .builder
            .build_call(function, &[lhs.into(), rhs.into()], "")?
            .try_as_basic_value()
            .basic()
            .expect("arithmetic intrinsics return a value")
            .into_struct_value();
        let value = self.builder.build_extract_value(result, 0, "")?;
        let overflowed = self.builder.build_extract_value(result, 1, "")?;
        self.trap_if(overflowed.into_int_value())?;
        Ok(value.into_int_value())
    }

    /// Divides builtin integers, rounding towards zero. Dividing by zero, or
    /// the smallest integer by -1, which overflows, traps.
    fn divide(
        &mut self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let int = self.context.i64_type();
        let by_zero = self
            .builder
            .build_int_compare(EQ, rhs, int.const_zero(), "")?;
        let is_min =
            self.builder
                .build_int_compare(EQ, lhs, int.const_int(i64::MIN as u64, true), "")?;
        let by_minus_one =
            self.builder
                .build_int_compare(EQ, rhs, int.const_int(-1i64 as u64, true), "")?;
        let overflows = self.builder.build_and(is_min, by_minus_one, "")?;
        let invalid = self.builder.build_or(by_zero, overflows, "")?;
        self.trap_if(invalid)?;
        self.builder.build_int_signed_div(lhs, rhs, "")
    }

    /// Traps if the builtin boolean `cond` is true, then carries on building
    /// in a new block.
    fn trap_if(&mut self, cond: IntValue<'ctx>) -> Result<(), BuilderError> {
        let function = self.current_function();
        let trap = self.trap_block()?;
        let after = self.context.append_basic_block(function, "");
        self.builder.build_conditional_branch(cond, trap, after)?;
        self.builder.position_at_end(after);
        Ok(())
    }

    /// The block in the function being built that traps, which is shared by
    /// everything in the function that can trap. It calls `abort`, which
    /// never returns.
    fn trap_block(&mut self) -> Result<BasicBlock<'ctx>, BuilderError> {
        if let Some(trap) = self.trap {
            return Ok(trap);
        }
        let current = self
            .builder
            .get_insert_block()
            .expect("the builder is inside a function");
        let trap = self
            .context
            .append_basic_block(self.current_function(), "trap");
        self.builder.position_at_end(trap);
        let abort = self.module.get_function("abort").unwrap_or_else(|| {
            let fn_type = self.context.void_type().fn_type(&[], false);
            self.module.add_function("abort", fn_type, None)
        });
        self.builder.build_call(abort, &[], "")?;
        self.builder.build_unreachable()?;
        self.builder.position_at_end(current);

        self.trap = Some(trap);
        Ok(trap)
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, os::unix::process::ExitStatusExt, process::ExitStatus};

    use super::*;
    use crate::syntax::{self, Module as AtomModule, parser::parse};

    /// Checks and compiles each `(path, src)` pair as a module, with paths
    /// like `/dir/other`.
    fn compile_files<'ctx>(context: &'ctx Context, files: &[(&str, &str)]) -> Module<'ctx> {
        let modules: Vec<_> = files
            .iter()
            .map(|(path, src)| AtomModule {
                path: path[1..].split('/').map(str::to_string).collect(),
                program: parse(src).expect("source should parse"),
            })
            .collect();
        let program = syntax::check(&modules).expect("source should check");
        assert!(program.main.is_some(), "`/main` should declare `main`");
        compile(context, &program)
    }

    /// Compiles the modules, then runs them with LLVM's JIT, returning the
    /// exit code. A program that traps would stop the tests, so those use
    /// `traps` instead.
    fn run_files(files: &[(&str, &str)]) -> i32 {
        Target::initialize_native(&InitializationConfig::default())
            .expect("LLVM should support this computer");
        let context = Context::create();
        let module = compile_files(&context, files);
        let engine = module
            .create_jit_execution_engine(OptimizationLevel::None)
            .expect("the JIT should start");
        unsafe {
            let main = engine
                .get_function::<unsafe extern "C" fn() -> i32>("main")
                .expect("the module should have a C `main`");
            main.call()
        }
    }

    /// Runs `src` as the only module, `/main`.
    fn run(src: &str) -> i32 {
        run_files(&[("/main", src)])
    }

    /// Builds `src` as the only module, `/main`, into an executable called
    /// `name`, then runs it.
    fn run_executable(name: &str, src: &str) -> ExitStatus {
        let dir = env::temp_dir().join(format!("atom-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("should create the temporary directory");
        let path = dir.join(name);

        let context = Context::create();
        let module = compile_files(&context, &[("/main", src)]);
        write_executable(&module, &path).expect("should build an executable");
        let status = Command::new(&path).status().expect("should run");
        fs::remove_file(&path).expect("should remove the executable");
        status
    }

    /// Whether `src`, as the only module, traps when run, which aborts it.
    fn traps(name: &str, src: &str) -> bool {
        const SIGABRT: i32 = 6;
        run_executable(name, src).signal() == Some(SIGABRT)
    }

    #[test]
    fn main_gives_the_exit_code() {
        assert_eq!(run("fn main() -> Int { return 42 }"), 42);
    }

    #[test]
    fn executables_exit_with_the_result_of_main() {
        let status = run_executable("exit_code", "fn main() -> Int { return 7 }");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn exit_codes_are_truncated() {
        assert_eq!(run("fn main() -> Int { return 0 - 1 }"), -1);
        let status = run_executable("truncated", "fn main() -> Int { return 257 }");
        assert_eq!(status.code(), Some(1));
    }

    #[test]
    fn evaluates_arithmetic() {
        assert_eq!(run("fn main() -> Int { return -1 + 2 * 3 - 10 / 4 }"), 3);
        assert_eq!(run("fn main() -> Int { return (1 + 2) * -(3 - 5) }"), 6);
    }

    #[test]
    fn division_rounds_towards_zero() {
        assert_eq!(run("fn main() -> Int { return -7 / 2 }"), -3);
        assert_eq!(run("fn main() -> Int { return 7 / -2 }"), -3);
    }

    #[test]
    fn locals_can_be_reassigned() {
        let src = "
            fn main() -> Int {
                let a = 4
                var b: Int = a * 2
                b = b + a
                return b
            }
            ";
        assert_eq!(run(src), 12);
    }

    #[test]
    fn if_takes_the_branch_matching_its_condition() {
        let src = "
            fn pick(when cond: Bool) -> Int {
                var result = 0
                if cond {
                    result = 1
                } else {
                    result = 2
                }
                return result
            }

            fn main() -> Int {
                return pick(when: true) * 10 + pick(when: false)
            }
            ";
        assert_eq!(run(src), 12);
    }

    #[test]
    fn if_without_else_can_be_skipped() {
        let src = "
            fn main() -> Int {
                var result = 1
                if false {
                    result = 2
                }
                return result
            }
            ";
        assert_eq!(run(src), 1);
    }

    #[test]
    fn else_if_takes_the_first_matching_branch() {
        let src = "
            fn grade(first: Bool, second: Bool) -> Int {
                if first {
                    return 1
                } else if second {
                    return 2
                } else {
                    return 3
                }
            }

            fn main() -> Int {
                let a = grade(first: true, second: true)
                let b = grade(first: false, second: true)
                let c = grade(first: false, second: false)
                return a * 100 + b * 10 + c
            }
            ";
        assert_eq!(run(src), 123);
    }

    #[test]
    fn statements_after_return_are_skipped() {
        let src = "
            fn main() -> Int {
                return 1
                return 1 / 0
            }
            ";
        assert_eq!(run(src), 1);
    }

    #[test]
    fn evaluates_boolean_operators() {
        let src = "
            fn bit(of value: Bool) -> Int {
                if value {
                    return 1
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(of: true & false)
                let b = bit(of: true | false)
                let c = bit(of: !false & !(false | false))
                return a * 100 + b * 10 + c
            }
            ";
        assert_eq!(run(src), 11);
    }

    #[test]
    fn functions_can_recurse() {
        let src = "
            fn factorial(of n: Int) -> Int {
                if n == 0 {
                    return 1
                }
                return n * factorial(of: n - 1)
            }

            fn main() -> Int {
                return factorial(of: 5)
            }
            ";
        assert_eq!(run(src), 120);
    }

    #[test]
    fn compares_integers() {
        let src = "
            fn bit(of value: Bool) -> Int {
                if value {
                    return 1
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(of: 2 + 2 == 4)
                let b = bit(of: 2 + 2 == 5)
                let c = bit(of: 0 - 9223372036854775807 - 1 != 9223372036854775807)
                let d = bit(of: 7 != 7)
                return a * 1000 + b * 100 + c * 10 + d
            }
            ";
        assert_eq!(run(src), 1010);
    }

    #[test]
    fn compares_booleans() {
        let src = "
            fn bit(of value: Bool) -> Int {
                if value {
                    return 1
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(of: true == true)
                let b = bit(of: true == false)
                let c = bit(of: false != true)
                let d = bit(of: (1 == 2) == false)
                return a * 1000 + b * 100 + c * 10 + d
            }
            ";
        assert_eq!(run(src), 1011);
    }

    #[test]
    fn nested_functions_are_compiled() {
        let src = "
            fn main() -> Int {
                fn triple(_ n: Int) -> Int {
                    return n * 3
                }
                return triple(4)
            }
            ";
        assert_eq!(run(src), 12);
    }

    #[test]
    fn void_functions_can_finish_without_return() {
        let src = "
            fn nothing() {}

            fn early(stop: Bool) {
                if stop {
                    return
                }
                let unused = Void.new()
            }

            fn main() -> Int {
                nothing()
                early(stop: true)
                early(stop: false)
                let void: Void = nothing()
                return 5
            }
            ";
        assert_eq!(run(src), 5);
    }

    #[test]
    fn groups_have_fields_and_methods() {
        let src = "
            group Point {
                let x: Int
                let y: Int
            }

            group Line {
                let start: Point
                let end: Point
            }

            impl Point {
                static fn at(x: Int, y: Int) -> Point {
                    return Point { x: x, y: y }
                }

                fn sum() -> Int {
                    return self.x + self.y
                }
            }

            impl Line {
                static fn from(_ start: Point, to end: Point) -> Line {
                    return Line { start: start, end: end }
                }

                fn width() -> Int {
                    return self.end.x - self.start.x
                }
            }

            fn main() -> Int {
                let line = Line.from(Point.at(x: 1, y: 2), to: Point.at(x: 5, y: 3))
                return line.width() * 10 + line.end.sum()
            }
            ";
        assert_eq!(run(src), 48);
    }

    #[test]
    fn standard_types_can_have_methods() {
        let src = "
            impl Int {
                fn doubled() -> Int {
                    return self * 2
                }

                static fn ten() -> Int {
                    return 10
                }
            }

            fn main() -> Int {
                return Int.ten().doubled() + 3.doubled()
            }
            ";
        assert_eq!(run(src), 26);
    }

    #[test]
    fn nested_groups_are_compiled() {
        let src = "
            fn main() -> Int {
                group Pair {
                    let first: Int
                    let second: Int
                }
                fn first(of pair: Pair) -> Int {
                    return pair.first
                }
                return 1
            }
            ";
        assert_eq!(run(src), 1);
    }

    #[test]
    fn top_level_values_are_set_before_main() {
        // Functions can't read top-level values, so this only shows that
        // they're evaluated, in an order that works.
        let src = "
            let a = b * 2
            let b = 1 / 0
            fn main() -> Int {
                return 0
            }
            ";
        assert!(traps("global_trap", src));
        let src = "
            let a = b * 2
            let b = 3
            fn main() -> Int {
                return 0
            }
            ";
        assert_eq!(run(src), 0);
    }

    #[test]
    fn compiles_across_modules() {
        let files = [
            (
                "/main",
                "
                use ./shapes/*

                fn main() -> Int {
                    let box = Square.new(side: 3)
                    return box.area() + square::describe(square: box)
                }
                ",
            ),
            (
                "/shapes/square",
                "
                group Square {
                    let side: Int
                }

                impl Square {
                    static fn new(side: Int) -> Square {
                        return Square { side: side }
                    }

                    fn area() -> Int {
                        return self.side * self.side
                    }
                }

                fn describe(square: Square) -> Int {
                    return 100
                }
                ",
            ),
        ];
        assert_eq!(run_files(&files), 109);
    }

    /// The largest integer.
    const MAX: &str = "9223372036854775807";
    /// The smallest integer, which can't be written as a literal.
    const MIN: &str = "(0 - 9223372036854775807 - 1)";

    /// A `main` that returns `value`.
    fn main_returning(value: &str) -> String {
        format!("fn main() -> Int {{ return {value} }}")
    }

    #[test]
    fn overflow_traps() {
        assert!(traps("add", &main_returning(&format!("{MAX} + 1"))));
        assert!(traps("sub", &main_returning(&format!("{MIN} - 1"))));
        assert!(traps("mul", &main_returning(&format!("{MAX} * 2"))));
        assert!(traps("neg", &main_returning(&format!("-{MIN}"))));
        assert!(!traps("min_value", &main_returning(MIN)));
    }

    #[test]
    fn invalid_division_traps() {
        assert!(traps("by_zero", &main_returning("1 / 0")));
        assert!(traps(
            "min_by_minus_one",
            &main_returning(&format!("{MIN} / -1"))
        ));
        assert!(!traps("min_by_one", &main_returning(&format!("{MIN} / 1"))));
    }
}
