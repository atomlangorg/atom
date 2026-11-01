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
//! The primitive integer types are LLVM integers of their width, such as `i8`
//! for `IntU8` and `IntS8`, since LLVM leaves signedness to the operations
//! used on them. `Bool` is `i1`. Every group is an
//! LLVM struct of its fields, so `Void` is `{}`. A choice is a struct of an
//! `i32` tag, the index of its option, then space for the largest of its
//! options' values, which they share. Every Atom function becomes an
//! internal LLVM function, so only the C `main` is
//! visible to the linker. It sets every top-level value, then calls Atom's
//! `main`, whose result is the exit code.
//!
//! Arithmetic that overflows, division by zero, and shifts that lose bits or
//! go too far trap by calling C's `abort`. `llvm.trap` would avoid needing C, but on macOS it can leave the
//! process hanging instead of stopping it, depending on what launched it.

use std::{fmt, io, num::NonZeroU32, path::Path, process::Command};

use inkwell::{
    IntPredicate::{EQ, NE, SGE, SGT, SLE, SLT, UGE, UGT, ULE, ULT},
    OptimizationLevel,
    basic_block::BasicBlock,
    builder::{Builder, BuilderError},
    context::Context,
    intrinsics::Intrinsic,
    module::{Linkage, Module},
    targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
    },
    types::{BasicType, BasicTypeEnum, IntType, StructType},
    values::{BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue, PointerValue},
};

use crate::{
    ir::{Body, Expr, MatchArm, Pattern, Primitive, PrimitiveFn, Program, Stmt, TypeDef},
    syntax::ast::{BinOp, UnOp},
};

/// The target machine for the computer the compiler is running on, which
/// decides how values are laid out in memory, and which instructions are used.
pub fn host_machine() -> Result<TargetMachine, EmitError> {
    Target::initialize_native(&InitializationConfig::default()).map_err(EmitError::Target)?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|err| EmitError::Target(err.to_string()))?;
    target
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string(),
            &TargetMachine::get_host_cpu_features().to_string(),
            OptimizationLevel::Default,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| EmitError::Target(format!("no target machine for {triple}")))
}

/// Compiles `program` into an LLVM module for `machine`. The module only has
/// a C `main`, so it can only be linked into an executable, if
/// `program.main` is set.
///
/// Panics if it builds invalid LLVM IR, which would be a bug in the compiler.
pub fn compile<'ctx>(
    context: &'ctx Context,
    program: &Program,
    machine: &TargetMachine,
) -> Module<'ctx> {
    let mut codegen = Codegen::new(context, program, machine);
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

/// Compiles `module`, which was compiled for `machine`, into an object file,
/// then links it into an executable at `path` with the system's C compiler,
/// `cc`.
pub fn write_executable(
    module: &Module,
    machine: &TargetMachine,
    path: &Path,
) -> Result<(), EmitError> {
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
    /// How the target lays out values in memory.
    target_data: TargetData,
    /// For each type in `program`, the LLVM type it's represented by.
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
    fn new(context: &'ctx Context, program: &'a Program, machine: &TargetMachine) -> Self {
        let module = context.create_module("atom");
        let target_data = machine.get_target_data();
        module.set_triple(&machine.get_triple());
        module.set_data_layout(&target_data.get_data_layout());
        Self {
            context,
            module,
            builder: context.create_builder(),
            program,
            target_data,
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

    /// Creates the LLVM type for each type. Every type is named before any
    /// are given fields, since they can hold each other in any order.
    fn declare_types(&mut self) {
        for ty in &self.program.types {
            let ty = match ty {
                &TypeDef::Primitive(Primitive::Int { bits, .. }) => NonZeroU32::new(bits)
                    .and_then(|bits| self.context.custom_width_int_type(bits).ok())
                    .expect("integer types have valid widths")
                    .into(),
                TypeDef::Primitive(Primitive::Bool) => self.context.bool_type().into(),
                TypeDef::Group { name, .. } | TypeDef::Choice { name, .. } => {
                    self.context.opaque_struct_type(name).into()
                }
            };
            self.types.push(ty);
        }
        let mut defined = vec![false; self.types.len()];
        for id in 0..self.types.len() {
            self.define_type(id, &mut defined);
        }
    }

    /// Gives the type with `id` its fields, once the types it holds have
    /// theirs, since a choice's size depends on the sizes of its options'
    /// values. `defined` records which types have been done. Types can't hold
    /// themselves, so this always finishes.
    fn define_type(&self, id: usize, defined: &mut [bool]) {
        if std::mem::replace(&mut defined[id], true) {
            return;
        }
        let held = match &self.program.types[id] {
            TypeDef::Primitive(_) => return,
            TypeDef::Group { fields, .. } => fields,
            TypeDef::Choice { options, .. } => options,
        };
        for &ty in held {
            self.define_type(ty, defined);
        }

        let body = match &self.program.types[id] {
            TypeDef::Choice { .. } => {
                // LLVM has no unions, so the options' values share an array
                // of integers that's big enough, and aligned enough, for
                // any of them.
                let (size, align) = held.iter().fold((0, 1), |(size, align), &ty| {
                    let ty = &self.types[ty];
                    let ty_size = self.target_data.get_abi_size(ty);
                    let ty_align = self.target_data.get_abi_alignment(ty);
                    (size.max(ty_size), align.max(ty_align))
                });
                let unit = NonZeroU32::new(align * 8)
                    .and_then(|bits| self.context.custom_width_int_type(bits).ok())
                    .expect("alignments are valid integer widths");
                let units = size.div_ceil(u64::from(align));
                let payload = unit.array_type(units as u32);
                vec![self.tag_type().into(), payload.into()]
            }
            _ => held.iter().map(|&ty| self.types[ty]).collect(),
        };
        self.types[id].into_struct_type().set_body(&body, false);
    }

    /// The type of a choice's tag, which is the index of its option.
    fn tag_type(&self) -> IntType<'ctx> {
        self.context.i32_type()
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

        match &function.body {
            Body::Stmts(body) => {
                self.block(body)?;
                // Every path returns, so the end can't be reached, such as
                // after an `if` whose branches both return, or a call to a
                // function that never returns.
                if !self.is_terminated() {
                    self.builder.build_unreachable()?;
                }
            }
            Body::Primitive(PrimitiveFn::Exit) => {
                // C's `exit` never returns.
                let exit = self.module.get_function("exit").unwrap_or_else(|| {
                    let code = self.context.i32_type();
                    let fn_type = self.context.void_type().fn_type(&[code.into()], false);
                    self.module.add_function("exit", fn_type, None)
                });
                let code = self.context.i32_type().const_int(1, false);
                self.builder.build_call(exit, &[code.into()], "")?;
                self.builder.build_unreachable()?;
            }
            // Calls are built as the instruction itself, but the function
            // is still defined like any other.
            &Body::Primitive(primitive) => {
                let params: Vec<_> = value
                    .get_param_iter()
                    .map(|param| param.into_int_value())
                    .collect();
                let result = self
                    .primitive_instruction(primitive, &params)?
                    .expect("only `exit` isn't a single instruction");
                self.builder.build_return(Some(&result))?;
            }
        }
        Ok(())
    }

    /// Builds the single instruction that `primitive` is, on `args`, where
    /// a method's `self` comes first, or `None` if it isn't one, such as
    /// `exit`.
    fn primitive_instruction(
        &self,
        primitive: PrimitiveFn,
        args: &[IntValue<'ctx>],
    ) -> Result<Option<IntValue<'ctx>>, BuilderError> {
        Ok(Some(match primitive {
            PrimitiveFn::Exit => return Ok(None),
            PrimitiveFn::BoolAnd => self.builder.build_and(args[0], args[1], "")?,
            PrimitiveFn::BoolOr => self.builder.build_or(args[0], args[1], "")?,
            PrimitiveFn::BoolNot => self.builder.build_not(args[0], "")?,
            PrimitiveFn::Equal => self.builder.build_int_compare(EQ, args[0], args[1], "")?,
            PrimitiveFn::NotEqual => self.builder.build_int_compare(NE, args[0], args[1], "")?,
        }))
    }

    /// Builds the C `main`, which sets every top-level value, then returns
    /// the result of `main`, an `Int`, as the exit code.
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
            Stmt::Match { value, ty, arms } => {
                let value = self.expr(value)?;
                self.match_(*ty, value, arms)?;
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
            Expr::Int { value, ty } => self.types[*ty]
                .into_int_type()
                .const_int(*value, false)
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
                    // Negating the smallest integer overflows. Only signed
                    // integers can be negated.
                    UnOp::Neg => {
                        let zero = operand.get_type().const_zero();
                        self.checked("llvm.ssub.with.overflow", zero, operand)?
                    }
                    UnOp::BitNot => self.builder.build_not(operand, "")?,
                    UnOp::Not => unreachable!("`!` compiles to a call to `not`"),
                }
                .into()
            }
            Expr::Binary {
                lhs,
                op,
                rhs,
                signed,
            } => {
                let lhs = self.expr(lhs)?.into_int_value();
                let rhs = self.expr(rhs)?.into_int_value();
                let sign = if *signed { "s" } else { "u" };
                match op {
                    BinOp::Add => {
                        self.checked(&format!("llvm.{sign}add.with.overflow"), lhs, rhs)?
                    }
                    BinOp::Sub => {
                        self.checked(&format!("llvm.{sign}sub.with.overflow"), lhs, rhs)?
                    }
                    BinOp::Mul => {
                        self.checked(&format!("llvm.{sign}mul.with.overflow"), lhs, rhs)?
                    }
                    BinOp::Div => self.divide(lhs, rhs, *signed)?,
                    BinOp::Lt => {
                        let predicate = if *signed { SLT } else { ULT };
                        self.builder.build_int_compare(predicate, lhs, rhs, "")?
                    }
                    BinOp::Le => {
                        let predicate = if *signed { SLE } else { ULE };
                        self.builder.build_int_compare(predicate, lhs, rhs, "")?
                    }
                    BinOp::Gt => {
                        let predicate = if *signed { SGT } else { UGT };
                        self.builder.build_int_compare(predicate, lhs, rhs, "")?
                    }
                    BinOp::Ge => {
                        let predicate = if *signed { SGE } else { UGE };
                        self.builder.build_int_compare(predicate, lhs, rhs, "")?
                    }
                    BinOp::BitAnd => self.builder.build_and(lhs, rhs, "")?,
                    BinOp::BitOr => self.builder.build_or(lhs, rhs, "")?,
                    BinOp::And | BinOp::Or | BinOp::Eq | BinOp::Ne => {
                        unreachable!("`&`, `|`, `==` and `!=` compile to calls to trait functions")
                    }
                    BinOp::BitXor => self.builder.build_xor(lhs, rhs, "")?,
                    BinOp::ShiftLeft => self.shift_left(lhs, rhs)?,
                    BinOp::ShiftRight => {
                        let amount = self.shift_amount(lhs, rhs)?;
                        self.builder.build_right_shift(lhs, amount, false, "")?
                    }
                }
                .into()
            }
            Expr::Call { function, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<Vec<_>, BuilderError>>()?;
                // A primitive that's a single instruction, such as `==` on
                // integers, is built in place rather than called.
                if let Body::Primitive(primitive) = self.program.functions[*function].body {
                    let ints: Vec<_> = args.iter().map(|arg| arg.into_int_value()).collect();
                    if let Some(result) = self.primitive_instruction(primitive, &ints)? {
                        return Ok(result.into());
                    }
                }
                let args: Vec<_> = args.into_iter().map(Into::into).collect();
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
            Expr::Choose {
                choice,
                option,
                value,
            } => {
                let value = self.expr(value)?;
                let ty = self.types[*choice].into_struct_type();
                self.choose(ty, *option, value)?
            }
            Expr::Field { receiver, index } => {
                let receiver = self.expr(receiver)?.into_struct_value();
                self.builder
                    .build_extract_value(receiver, *index as u32, "")?
            }
        })
    }

    /// Creates a choice of type `ty`, as the option at `option`, holding
    /// `value`. LLVM has no unions, so the value is written over the choice's
    /// payload in memory.
    fn choose(
        &self,
        ty: StructType<'ctx>,
        option: usize,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let slot = self.entry_alloca(ty)?;
        let tag = self.builder.build_struct_gep(ty, slot, 0, "")?;
        let tag_value = self.tag_type().const_int(option as u64, false);
        self.builder.build_store(tag, tag_value)?;
        let payload = self.builder.build_struct_gep(ty, slot, 1, "")?;
        self.builder.build_store(payload, value)?;
        self.builder.build_load(ty, slot, "")
    }

    /// Runs the block of the first arm in `arms` whose pattern matches
    /// `value`, of the type at `ty` in `program.types`, checking each arm in
    /// turn. Some arm always matches, so the end of the last can't be reached.
    fn match_(
        &mut self,
        ty: usize,
        value: BasicValueEnum<'ctx>,
        arms: &[MatchArm],
    ) -> Result<(), BuilderError> {
        let slot = self.entry_alloca(self.types[ty])?;
        self.builder.build_store(slot, value)?;
        let function = self.current_function();
        let after = self.context.append_basic_block(function, "after");
        for arm in arms {
            let unmatched = self.context.append_basic_block(function, "unmatched");
            self.test(&arm.pattern, slot, ty, unmatched)?;
            self.block(&arm.body)?;
            if !self.is_terminated() {
                self.builder.build_unconditional_branch(after)?;
            }
            self.builder.position_at_end(unmatched);
        }
        self.builder.build_unreachable()?;
        self.builder.position_at_end(after);
        Ok(())
    }

    /// Checks that the value at `ptr`, of the type at `ty` in
    /// `program.types`, matches `pattern`, carrying on building if it does,
    /// and branching to `unmatched` as soon as it doesn't. The values it
    /// binds are stored as it goes. A choice's value is only read once its
    /// tag shows which option it is, since it can only be read as that
    /// option's value through memory, like it's written.
    fn test(
        &mut self,
        pattern: &Pattern,
        ptr: PointerValue<'ctx>,
        ty: usize,
        unmatched: BasicBlock<'ctx>,
    ) -> Result<(), BuilderError> {
        let program = self.program;
        let (found, expected) = match pattern {
            Pattern::Any => return Ok(()),
            Pattern::Binding(local) => {
                let (slot, local_ty) = self.locals[*local];
                let value = self.builder.build_load(local_ty, ptr, "")?;
                self.builder.build_store(slot, value)?;
                return Ok(());
            }
            Pattern::Int(bits) => {
                let int = self.types[ty].into_int_type();
                let found = self.builder.build_load(int, ptr, "")?.into_int_value();
                (found, int.const_int(*bits, false))
            }
            Pattern::Bool(value) => {
                let bool = self.context.bool_type();
                let found = self.builder.build_load(bool, ptr, "")?.into_int_value();
                (found, bool.const_int(u64::from(*value), false))
            }
            Pattern::Tuple(elements) => {
                let TypeDef::Group { fields, .. } = &program.types[ty] else {
                    unreachable!("only tuples are matched with tuple patterns");
                };
                let tuple = self.types[ty].into_struct_type();
                for (index, (element, &field)) in elements.iter().zip(fields).enumerate() {
                    let ptr = self
                        .builder
                        .build_struct_gep(tuple, ptr, index as u32, "")?;
                    self.test(element, ptr, field, unmatched)?;
                }
                return Ok(());
            }
            Pattern::Option { index, value } => {
                let TypeDef::Choice { options, .. } = &program.types[ty] else {
                    unreachable!("only choices are matched with option patterns");
                };
                let choice = self.types[ty].into_struct_type();
                let tag_ptr = self.builder.build_struct_gep(choice, ptr, 0, "")?;
                let tag = self.builder.build_load(self.tag_type(), tag_ptr, "")?;
                let expected = self.tag_type().const_int(*index as u64, false);
                self.check(tag.into_int_value(), expected, unmatched)?;
                let payload = self.builder.build_struct_gep(choice, ptr, 1, "")?;
                return self.test(value, payload, options[*index], unmatched);
            }
        };
        self.check(found, expected, unmatched)
    }

    /// Carries on building if `found` is `expected`, and branches to
    /// `unmatched` otherwise.
    fn check(
        &mut self,
        found: IntValue<'ctx>,
        expected: IntValue<'ctx>,
        unmatched: BasicBlock<'ctx>,
    ) -> Result<(), BuilderError> {
        let is_expected = self.builder.build_int_compare(EQ, found, expected, "")?;
        let matched = self
            .context
            .append_basic_block(self.current_function(), "matched");
        self.builder
            .build_conditional_branch(is_expected, matched, unmatched)?;
        self.builder.position_at_end(matched);
        Ok(())
    }

    /// Allocates space for a `ty` at the start of the function being built,
    /// so it's only allocated once, however often the code using it runs.
    fn entry_alloca(&self, ty: impl BasicType<'ctx>) -> Result<PointerValue<'ctx>, BuilderError> {
        let entry = self
            .current_function()
            .get_first_basic_block()
            .expect("functions start with an entry block");
        let builder = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(first) => builder.position_before(&first),
            None => builder.position_at_end(entry),
        }
        builder.build_alloca(ty, "")
    }

    /// Calls `intrinsic`, an LLVM arithmetic intrinsic that reports overflow,
    /// on integers of the same type, trapping if it overflows.
    fn checked(
        &mut self,
        intrinsic: &str,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let int = lhs.get_type();
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

    /// Divides integers of the same type, rounding towards zero. Dividing by
    /// zero traps, and so does dividing the smallest signed integer by -1,
    /// which overflows.
    fn divide(
        &mut self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        signed: bool,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let int = lhs.get_type();
        let by_zero = self
            .builder
            .build_int_compare(EQ, rhs, int.const_zero(), "")?;
        if !signed {
            self.trap_if(by_zero)?;
            return self.builder.build_int_unsigned_div(lhs, rhs, "");
        }

        // The smallest signed integer is just its top bit.
        let min = int.const_int(1 << (int.get_bit_width() - 1), false);
        let is_min = self.builder.build_int_compare(EQ, lhs, min, "")?;
        let by_minus_one = self
            .builder
            .build_int_compare(EQ, rhs, int.const_all_ones(), "")?;
        let overflows = self.builder.build_and(is_min, by_minus_one, "")?;
        let invalid = self.builder.build_or(by_zero, overflows, "")?;
        self.trap_if(invalid)?;
        self.builder.build_int_signed_div(lhs, rhs, "")
    }

    /// Shifts the unsigned integer `value` left by `amount`, filling with
    /// zeros. Traps if any `1`s are shifted off the end.
    fn shift_left(
        &mut self,
        value: IntValue<'ctx>,
        amount: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let amount = self.shift_amount(value, amount)?;
        let shifted = self.builder.build_left_shift(value, amount, "")?;
        let unshifted = self.builder.build_right_shift(shifted, amount, false, "")?;
        let lost = self.builder.build_int_compare(NE, unshifted, value, "")?;
        self.trap_if(lost)?;
        Ok(shifted)
    }

    /// Converts `amount`, an integer of any type, to the type of `value` to
    /// shift it by. Traps unless it's from 0 to one less than the width of
    /// `value`. A negative `amount` is too large when taken as unsigned.
    fn shift_amount(
        &mut self,
        value: IntValue<'ctx>,
        amount: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let ty = value.get_type();
        // Every integer type can hold the width of the widest, which is 64.
        let width = amount
            .get_type()
            .const_int(u64::from(ty.get_bit_width()), false);
        let too_far = self.builder.build_int_compare(UGE, amount, width, "")?;
        self.trap_if(too_far)?;
        self.builder.build_int_cast_sign_flag(amount, ty, false, "")
    }

    /// Traps if the `Bool` `cond` is true, then carries on building
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

    use inkwell::{execution_engine::ExecutionEngine, values::AnyValue};

    use super::*;
    use crate::syntax::{self, Module as AtomModule, parser::parse};

    fn machine() -> TargetMachine {
        host_machine().expect("LLVM should support this computer")
    }

    /// Checks and compiles each `(path, src)` pair as a module, with paths
    /// like `/dir/other`.
    fn compile_files<'ctx>(
        context: &'ctx Context,
        machine: &TargetMachine,
        files: &[(&str, &str)],
    ) -> Module<'ctx> {
        let modules: Vec<_> = files
            .iter()
            .map(|(path, src)| AtomModule {
                path: path[1..].split('/').map(str::to_string).collect(),
                program: parse(src).expect("source should parse"),
            })
            .collect();
        let program = syntax::check(&modules).expect("source should check");
        assert!(program.main.is_some(), "`/main` should declare `main`");
        compile(context, &program, machine)
    }

    /// Compiles the modules, then runs them with LLVM's JIT, returning the
    /// exit code. A program that traps would stop the tests, so those use
    /// `traps` instead.
    fn run_files(files: &[(&str, &str)]) -> i32 {
        let context = Context::create();
        let module = compile_files(&context, &machine(), files);
        let engine = module
            .create_jit_execution_engine(OptimizationLevel::None)
            .expect("the JIT should start");
        unsafe { call(&engine, "main") }
    }

    /// Calls the function called `name`, which takes no arguments, from code
    /// compiled by `engine`.
    unsafe fn call<T>(engine: &ExecutionEngine, name: &str) -> T {
        unsafe {
            engine
                .get_function::<unsafe extern "C" fn() -> T>(name)
                .unwrap_or_else(|_| panic!("the module should have `{name}`"))
                .call()
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
        let machine = machine();
        let module = compile_files(&context, &machine, &[("/main", src)]);
        write_executable(&module, &machine, &path).expect("should build an executable");
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
    fn orders_integers() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(1 < 2, at: 1)
                let b = bit(2 < 2, at: 2)
                let c = bit(2 <= 2, at: 4)
                let d = bit(3 <= 2, at: 8)
                let e = bit(3 > 2, at: 16)
                let f = bit(2 > 2, at: 32)
                let g = bit(2 >= 2, at: 64)
                let h = bit(2 >= 3, at: 128)
                return a + b + c + d + e + f + g + h
            }
            ";
        assert_eq!(run(src), 85);
    }

    #[test]
    fn orders_integers_by_their_sign() {
        // As a signed `i8`, 200 would be -56, and as an unsigned `u8`, -1
        // would be 255.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let byte: IntU8 = 200
                let small: IntS8 = -1
                let huge: IntU64 = 18446744073709551615
                let a = bit(byte > 100, at: 1)
                let b = bit(small < 0, at: 2)
                let c = bit(huge >= 9223372036854775807, at: 4)
                let d = bit(0 - 9223372036854775807 - 1 <= 9223372036854775807, at: 8)
                return a + b + c + d
            }
            ";
        assert_eq!(run(src), 15);
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

    /// Compiles `src` as the only module, `/main`, and runs it, then reads
    /// its top-level value called `name`, a choice of type `ty`. Returns the
    /// choice's tag, and the start of its payload as an integer `bits` wide.
    ///
    /// Nothing in Atom can look inside a choice yet, so this adds functions
    /// to the module that read it.
    fn read_choice(src: &str, name: &str, ty: &str, bits: u32) -> (u32, u64) {
        let context = Context::create();
        let module = compile_files(&context, &machine(), &[("/main", src)]);
        let choice = module
            .get_struct_type(&format!("/main::{ty}"))
            .expect("should declare the choice");
        let global = module
            .get_global(&format!("/main::{name}"))
            .expect("should declare the value")
            .as_pointer_value();

        let builder = context.create_builder();
        let int = context.i64_type();
        let reader = |reader: &str, index: u32, ty: IntType| -> Result<(), BuilderError> {
            let function = module.add_function(reader, int.fn_type(&[], false), None);
            builder.position_at_end(context.append_basic_block(function, "entry"));
            let field = builder.build_struct_gep(choice, global, index, "")?;
            let value = builder.build_load(ty, field, "")?.into_int_value();
            let value = builder.build_int_z_extend(value, int, "")?;
            builder.build_return(Some(&value))?;
            Ok(())
        };
        reader("read_tag", 0, context.i32_type()).expect("should build the reader");
        let payload = NonZeroU32::new(bits)
            .and_then(|bits| context.custom_width_int_type(bits).ok())
            .expect("should be a valid width");
        reader("read_payload", 1, payload).expect("should build the reader");

        let engine = module
            .create_jit_execution_engine(OptimizationLevel::None)
            .expect("the JIT should start");
        unsafe {
            call::<i32>(&engine, "main");
            let tag: u64 = call(&engine, "read_tag");
            (tag as u32, call(&engine, "read_payload"))
        }
    }

    /// A choice for tests, with options holding values of different types.
    const SHAPE: &str = "
        choice Shape {
            circle: Int
            empty
            flag: Bool
        }
        ";

    #[test]
    fn choices_hold_their_option_and_its_value() {
        let src = format!(
            "
            {SHAPE}
            let circle = Shape.circle(3)
            let flag = Shape.flag(true)
            let empty = Shape.empty
            let emptyWithValue = Shape.empty(Void.new())
            fn main() -> Int {{
                return 0
            }}
            "
        );
        assert_eq!(read_choice(&src, "circle", "Shape", 64), (0, 3));
        assert_eq!(read_choice(&src, "flag", "Shape", 8), (2, 1));
        assert_eq!(read_choice(&src, "empty", "Shape", 8).0, 1);
        assert_eq!(read_choice(&src, "emptyWithValue", "Shape", 8).0, 1);
    }

    #[test]
    fn choices_can_hold_groups_and_choices() {
        let src = format!(
            "
            {SHAPE}
            group Point {{
                let x: Int
                let y: Int
            }}

            impl Point {{
                static fn at(x: Int, y: Int) -> Point {{
                    return Point {{ x: x, y: y }}
                }}
            }}

            choice Drawing {{
                shape: Shape
                point: Point
            }}

            let shape = Drawing.shape(Shape.flag(false))
            let point = Drawing.point(Point.at(x: 7, y: 8))
            fn main() -> Int {{
                return 0
            }}
            "
        );
        // The shape's payload starts with its own tag.
        assert_eq!(read_choice(&src, "shape", "Drawing", 32), (0, 2));
        assert_eq!(read_choice(&src, "point", "Drawing", 64), (1, 7));
    }

    #[test]
    fn choices_can_be_passed_around() {
        let src = format!(
            "
            {SHAPE}
            impl Shape {{
                static fn unit() -> Shape {{
                    return Shape.circle(1)
                }}

                fn sides() -> Int {{
                    return 4
                }}
            }}

            group Canvas {{
                let shape: Shape
            }}

            impl Canvas {{
                static fn of(_ shape: Shape) -> Canvas {{
                    return Canvas {{ shape: shape }}
                }}
            }}

            fn keep(_ shape: Shape) -> Shape {{
                return shape
            }}

            fn absurd(_ never: Never) -> Int {{
                return 0
            }}

            fn main() -> Int {{
                choice Local {{
                    only
                }}
                let local = Local.only
                let canvas = Canvas.of(keep(Shape.unit()))
                return canvas.shape.sides() * 10 + Shape.empty.sides()
            }}
            "
        );
        assert_eq!(run(&src), 44);
    }

    #[test]
    fn match_runs_the_block_for_the_option() {
        let src = format!(
            "
            {SHAPE}
            fn describe(_ shape: Shape) -> Int {{
                var result = 0
                match shape {{
                    Shape.circle(let radius) => {{
                        result = radius * 10
                    }}
                    Shape.empty => {{
                        result = 1
                    }}
                    Shape.flag(let flag) => {{
                        if flag {{
                            result = 2
                        }} else {{
                            result = 3
                        }}
                    }}
                }}
                return result
            }}

            fn main() -> Int {{
                let a = describe(Shape.circle(5))
                let b = describe(Shape.empty)
                let c = describe(Shape.flag(true))
                let d = describe(Shape.flag(false))
                return a * 1000 + b * 100 + c * 10 + d
            }}
            "
        );
        assert_eq!(run(&src), 50123);
    }

    #[test]
    fn match_options_can_return() {
        let src = format!(
            "
            {SHAPE}
            impl Shape {{
                fn size() -> Int {{
                    match self {{
                        Shape.circle(let radius) => {{
                            return radius
                        }}
                        Shape.empty => {{
                            return 0
                        }}
                        Shape.flag => {{
                            return 1
                        }}
                    }}
                }}
            }}

            fn absurd(_ never: Never) -> Int {{
                match never {{}}
            }}

            fn main() -> Int {{
                return Shape.circle(7).size() * 10 + Shape.flag(true).size()
            }}
            "
        );
        assert_eq!(run(&src), 71);
    }

    #[test]
    fn match_reads_values_of_any_type() {
        let src = format!(
            "
            {SHAPE}
            group Point {{
                let x: Int
                let y: Int
            }}

            impl Point {{
                static fn at(x: Int, y: Int) -> Point {{
                    return Point {{ x: x, y: y }}
                }}
            }}

            choice Drawing {{
                shape: Shape
                point: Point
            }}

            fn total(of drawing: Drawing) -> Int {{
                match drawing {{
                    Drawing.shape(let shape) => {{
                        match shape {{
                            Shape.circle(let radius) => {{
                                return radius
                            }}
                            Shape.empty => {{
                                return 0
                            }}
                            Shape.flag => {{
                                return 0
                            }}
                        }}
                    }}
                    Drawing.point(let point) => {{
                        return point.x + point.y
                    }}
                }}
            }}

            fn main() -> Int {{
                let circle = total(of: Drawing.shape(Shape.circle(4)))
                let point = total(of: Drawing.point(Point.at(x: 20, y: 30)))
                return circle + point
            }}
            "
        );
        assert_eq!(run(&src), 54);
    }

    #[test]
    fn aliases_compile_as_their_types() {
        let src = format!(
            "
            {SHAPE}
            alias Figure = Shape
            alias Size = Int

            fn size(of figure: Figure) -> Size {{
                match figure {{
                    Figure.circle(let radius) => {{
                        return radius
                    }}
                    Shape.empty => {{
                        return 0
                    }}
                    Figure.flag => {{
                        return 1
                    }}
                }}
            }}

            fn main() -> Int {{
                alias Total = Size
                let total: Total = size(of: Figure.circle(40)) + size(of: Shape.flag(true))
                return total
            }}
            "
        );
        assert_eq!(run(&src), 41);
    }

    #[test]
    fn integer_types_have_their_own_ranges_and_signs() {
        // Each check is one bit of the result. As a signed `i8`, 200 would
        // be -56, so dividing it shows the division is unsigned.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let byte: IntU8 = 200
                let small: IntS8 = -128
                let huge: IntU64 = 18446744073709551615
                let a = bit(byte / 3 == 66, at: 1)
                let b = bit(byte + 55 == 255, at: 2)
                let c = bit(small / 2 == -64, at: 4)
                let d = bit(-(small + 1) == 127, at: 8)
                let e = bit(huge / 2 == 9223372036854775807, at: 16)
                let f = bit(huge != 0, at: 32)
                return a + b + c + d + e + f
            }
            ";
        assert_eq!(run(src), 63);
    }

    #[test]
    fn evaluates_bitwise_operators() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let byte: IntU8 = 202
                let small: IntS8 = -128
                let a = bit(12 .& 10 == 8, at: 1)
                let b = bit(12 .| 10 == 14, at: 2)
                let c = bit(12 .^ 10 == 6, at: 4)
                let d = bit(.!0 == -1, at: 8)
                let e = bit(.!byte == 53, at: 16)
                let f = bit(small .| 1 == -127, at: 32)
                let g = bit(byte .^ .!byte == 255, at: 64)
                return a + b + c + d + e + f + g
            }
            ";
        assert_eq!(run(src), 127);
    }

    #[test]
    fn shifts_unsigned_integers() {
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let byte: IntU8 = 1
                let huge: IntU64 = 18446744073709551615
                let by: IntS8 = 7
                let a = bit(byte .< 7 == 128, at: 1)
                let b = bit(byte .< by .> by == 1, at: 2)
                let c = bit(huge .> 63 == 1, at: 4)
                let d = bit(huge .> 0 == huge, at: 8)
                let e = bit((huge .> 4) .< 4 == huge - 15, at: 16)
                let f = bit(byte .> 1 == 0, at: 32)
                return a + b + c + d + e + f
            }
            ";
        assert_eq!(run(src), 63);
    }

    #[test]
    fn shifts_trap_when_they_lose_bits_or_go_too_far() {
        let cases = [
            ("shl_loses", "let x: IntU8 = 129\nlet y = x .< 1"),
            ("shl_loses_far", "let x: IntU64 = 2\nlet y = x .< 63"),
            ("shl_width", "let x: IntU8 = 0\nlet y = x .< 8"),
            ("shr_width", "let x: IntU64 = 0\nlet y = x .> 64"),
            ("shl_negative", "let x: IntU8 = 0\nlet y = x .< -1"),
            (
                "shr_negative",
                "let x: IntU8 = 0\nlet by: IntS8 = -128\nlet y = x .> by",
            ),
            (
                "shr_wide",
                "let x: IntU8 = 0\nlet by: IntU64 = 4294967296\nlet y = x .> by",
            ),
        ];
        for (name, body) in cases {
            let src = format!("fn main() -> Int {{\n{body}\nreturn 0\n}}");
            assert!(traps(name, &src), "{name} should trap");
        }
        let src = "fn main() -> Int {\nlet x: IntU8 = 127\nlet y = x .< 1 .> 7\nreturn 0\n}";
        assert!(!traps("shl_keeps", src));
    }

    #[test]
    fn exit_stops_the_program_with_exit_code_1() {
        let src = "
            fn check(_ n: Int) -> Int {
                if n > 5 {
                    exit()
                }
                return n
            }

            fn stop() {
                exit()
            }

            fn main() -> Int {
                let a = check(3)
                if a == 3 {
                    stop()
                }
                return 7
            }
            ";
        assert_eq!(run_executable("exit_in_void", src).code(), Some(1));

        let src = "
            fn check(_ n: Int) -> Int {
                if n > 5 {
                    exit()
                }
                return n
            }

            fn main() -> Int {
                return check(3) + check(9)
            }
            ";
        assert_eq!(run_executable("exit_in_int", src).code(), Some(1));

        let src = "fn main() -> Int { exit() }";
        assert_eq!(run_executable("exit_in_main", src).code(), Some(1));
    }

    #[test]
    fn exit_stops_the_program_from_inside_a_statement() {
        let src = "
            fn describe(_ never: Never) -> Int {
                match never {}
            }

            fn stop() -> Never {
                return exit()
            }

            fn total(stopping: Bool) -> Int {
                var total = 2
                if stopping {
                    total = total + describe(stop())
                }
                return total
            }

            fn main() -> Int {
                let a = total(stopping: false)
                let b = total(stopping: true)
                return a + b
            }
            ";
        assert_eq!(run_executable("exit_in_assign", src).code(), Some(1));

        let src = "
            fn stop() {
                let stopped = exit()
            }

            fn main() -> Int {
                stop()
                let stopped = exit()
            }
            ";
        assert_eq!(run_executable("exit_in_decl", src).code(), Some(1));
    }

    #[test]
    fn self_type_is_the_type_of_its_impl() {
        let src = "
            group Counter {
                let count: Int
            }

            impl Counter {
                static fn new() -> Self {
                    return Self { count: 0 }
                }

                fn plus(_ amount: Int) -> Self {
                    return Self { count: self.count + amount }
                }
            }

            fn main() -> Int {
                return Counter.new().plus(5).plus(2).count
            }
            ";
        assert_eq!(run(src), 7);
    }

    #[test]
    fn boolean_operators_call_and_and_or() {
        // Each check is one bit of the result. `Flags` uses `&` and `|` by
        // implementing `And` and `Or`.
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

            impl Flags {
                static fn of(_ bits: Int) -> Self {
                    return Self { bits: bits }
                }
            }

            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(true & true, at: 1)
                let b = bit(true & false, at: 2)
                let c = bit(false | true, at: 4)
                let d = bit(false | false, at: 8)
                let e = bit(true.and(true) & false.or(true), at: 16)
                let f = bit(Flags.of(6) & Flags.of(3), at: 32)
                let g = bit(Flags.of(4) & Flags.of(3), at: 64)
                let h = bit(Flags.of(0) | Flags.of(0), at: 128)
                return a + c + e + f + b + d + g + h
            }
            ";
        assert_eq!(run(src), 53);
    }

    #[test]
    fn not_calls_not() {
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

            impl Flags {
                static fn of(_ bits: Int) -> Self {
                    return Self { bits: bits }
                }
            }

            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let flags = Flags.of(5)
                let a = bit(!false, at: 1)
                let b = bit(!true, at: 2)
                let c = bit(!!true & false.not(), at: 4)
                let d = bit((!flags).bits == -6, at: 8)
                let e = bit((!!flags).bits == 5, at: 16)
                return a + b + c + d + e
            }
            ";
        assert_eq!(run(src), 29);
    }

    #[test]
    fn operators_use_their_implementations_aliases() {
        // `Int & Bool` keeps the `Int` if the `Bool` is `true`, and `!` on an
        // `Int` is whether it's zero.
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

            fn main() -> Int {
                let a = 40 & true
                let b = 7 & false
                let c = (2 & !0) & !b
                return a + b + c
            }
            ";
        assert_eq!(run(src), 42);
    }

    #[test]
    fn operators_work_with_inferred_aliases() {
        // `Rhs` and `Output` are inferred from `and`, so `Int & Bool` is an
        // `Int`.
        let src = "
            impl Int with And {
                fn and(_ keep: Bool) -> Int {
                    if keep {
                        return self
                    }
                    return 0
                }
            }

            fn main() -> Int {
                return (40 & true) + (7 & false) + (2 & !false)
            }
            ";
        assert_eq!(run(src), 42);
    }

    #[test]
    fn equality_operators_call_equal() {
        // `Point` uses the default `isNotEqual`, and `Loose` implements its
        // own, which is only `true` if they're far apart.
        let src = "
            group Point {
                let x: Int
            }

            impl Point with Equal {
                fn isEqual(to rhs: Self) -> Bool {
                    return self.x == rhs.x
                }
            }

            impl Point {
                static fn at(_ x: Int) -> Self {
                    return Self { x: x }
                }
            }

            group Loose {
                let x: Int
            }

            impl Loose with Equal {
                fn isEqual(to rhs: Self) -> Bool {
                    return self.x == rhs.x
                }

                fn isNotEqual(to rhs: Self) -> Bool {
                    return self.x - rhs.x > 10 | rhs.x - self.x > 10
                }
            }

            impl Loose {
                static fn at(_ x: Int) -> Self {
                    return Self { x: x }
                }
            }

            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(Point.at(1) == Point.at(1), at: 1)
                let b = bit(Point.at(1) != Point.at(2), at: 2)
                let c = bit(Point.at(1) != Point.at(1), at: 4)
                let d = bit(Loose.at(1) != Loose.at(2), at: 8)
                let e = bit(Loose.at(1) != Loose.at(20), at: 16)
                let byte: IntU8 = 200
                let f = bit(200 == byte & byte != 0, at: 32)
                return a + b + c + d + e + f
            }
            ";
        assert_eq!(run(src), 51);
    }

    #[test]
    fn primitive_operators_are_single_instructions() {
        // Each operator is built in place as one instruction, rather than as
        // a call to the primitive that implements it.
        let src = "
            fn equal(_ a: IntU8, _ b: IntU8) -> Bool {
                return a == b
            }
            fn notEqual(_ a: Int, _ b: Int) -> Bool {
                return a != b
            }
            fn bools(_ a: Bool, _ b: Bool) -> Bool {
                return !(a & b) | a == b | a != b
            }
            fn main() -> Int {
                return 0
            }
            ";
        let context = Context::create();
        let module = compile_files(&context, &machine(), &[("/main", src)]);
        let body = |name: &str| {
            module
                .get_function(&format!("/main::{name}"))
                .expect("the function should be compiled")
                .print_to_string()
                .to_string()
        };
        let equal = body("equal");
        assert!(equal.contains("icmp eq i8"), "{equal}");
        let not_equal = body("notEqual");
        assert!(not_equal.contains("icmp ne i64"), "{not_equal}");
        let bools = body("bools");
        for instruction in ["and i1", "xor i1", "or i1", "icmp eq i1", "icmp ne i1"] {
            assert!(bools.contains(instruction), "{instruction}: {bools}");
        }
        for body in [equal, not_equal, bools] {
            assert!(!body.contains("call"), "{body}");
        }
    }

    #[test]
    fn tuples_are_built_and_read() {
        let src = "
            group Line {
                let start: (x: Int, y: Int)
                let end: (x: Int, y: Int)
            }

            impl Line {
                static fn from(_ start: (x: Int, y: Int), to end: (x: Int, y: Int)) -> Self {
                    return Self { start: start, end: end }
                }
            }

            fn swapped(_ point: (at x: Int, y: Int)) -> (at x: Int, y: Int) {
                return (at: point.y, y: point.x)
            }

            fn main() -> Int {
                let point = swapped((at: 3, y: 40))
                let line = Line.from((x: 1, y: 2), to: (x: 10, y: 20))
                let single: (_ only: Int) = (100)
                let unit = ()
                return point.x + point.y + line.end.y - line.start.x + single.only
            }
            ";
        assert_eq!(run(src), 162);
    }

    #[test]
    fn matches_run_the_first_matching_arm() {
        // Each `describe` returns which arm matched, and the values it bound.
        let src = "
            choice Inner {
                circle: Int
                empty
            }

            choice Shape {
                circle: Int
                pair: (left: Inner, right: Int)
                empty
            }

            fn describe(_ point: (x: Int, y: Bool)) -> Int {
                match point {
                    (x: 0, y: true) => {
                        return 1
                    }
                    (x: let x, y: true) => {
                        return x
                    }
                    (x: -1, y: _) => {
                        return 3
                    }
                    _ => {
                        return 4
                    }
                }
            }

            fn size(of shape: Shape) -> Int {
                match shape {
                    Shape.circle(0) => {
                        return 100
                    }
                    Shape.circle(let radius) => {
                        return radius
                    }
                    Shape.pair((left: Inner.circle(let r), right: let n)) => {
                        return r * n
                    }
                    Shape.pair(_) => {
                        return 7
                    }
                    Shape.empty => {
                        return 0
                    }
                }
            }

            fn count(_ n: IntU8) -> Int {
                match n {
                    255 => {
                        return 1
                    }
                    let other => {
                        match other == 0 {
                            true => {
                                return 2
                            }
                            false => {
                                return 3
                            }
                        }
                    }
                }
            }

            fn main() -> Int {
                // 1 + 20 + 3 + 4 = 28
                let a = describe((x: 0, y: true)) + describe((x: 20, y: true))
                let b = describe((x: -1, y: false)) + describe((x: 5, y: false))
                // 100 + 9 + 30 + 7 + 0 = 146
                let c = size(of: Shape.circle(0)) + size(of: Shape.circle(9))
                let d = size(of: Shape.pair((left: Inner.circle(6), right: 5)))
                let e = size(of: Shape.pair((left: Inner.empty, right: 5))) + size(of: Shape.empty)
                // 1 + 2 + 3 = 6
                let f = count(255) + count(0) + count(9)
                return a + b + c + d + e + f
            }
            ";
        assert_eq!(run(src), 180);
    }

    #[test]
    fn destructuring_declares_constants() {
        let src = "
            fn split(_ n: Int) -> (tens: Int, ones: Int) {
                return (tens: n / 10, ones: n - n / 10 * 10)
            }

            fn main() -> Int {
                let x = 3
                let y = 5
                (let a, let b) = (y, x)
                (tens: let tens, ones: _) = split(47)
                (_, (inner: let c)) = (x, (inner: 100))
                // 5 * 1000 + 3 * 100 + 4 * 10 + 100
                return a * 1000 + b * 100 + tens * 10 + c
            }
            ";
        assert_eq!(run(src), 5440);
    }

    #[test]
    fn generic_types_are_compiled_for_each_use() {
        let src = "
            choice Optional<T> {
                none
                some: T
            }

            group Box<T> {
                let value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }

                fn get() -> Self.T {
                    return self.value
                }

                fn maybe() -> Optional<Self.T> {
                    return Optional.some(self.value)
                }
            }

            group Pair<A, B> {
                let first: A
                let second: B
            }

            impl Pair {
                static fn of(_ first: Self.A, _ second: Self.B) -> Self {
                    return Self { first: first, second: second }
                }
            }

            fn unwrap(_ value: Optional<Int>, or fallback: Int) -> Int {
                match value {
                    Optional.some(let found) => {
                        return found
                    }
                    Optional.none => {
                        return fallback
                    }
                }
            }

            fn main() -> Int {
                let a: Optional<Int> = Optional.some(40)
                let b = Optional<Int>.none
                let boxed: Box<Bool> = Box.of(true)
                let count = Box<Int>.of(1).get()
                let pair = Pair<Int, Box<Bool>>.of(100, boxed)
                let flag = pair.second.get()
                var total = unwrap(a, or: 0) + unwrap(b, or: 1) + count
                if flag {
                    total = total + pair.first
                }
                return total + unwrap(Box<Int>.of(1000).maybe(), or: 0)
            }
            ";
        assert_eq!(run(src), 1142);
    }

    #[test]
    fn values_implementing_call_can_be_called() {
        let src = "
            group Adder {
                let base: Int
            }

            impl Adder with Call {
                fn call(input: (a: Int, b: Int)) -> Int {
                    return self.base + input.a + input.b
                }
            }

            impl Adder {
                static fn from(_ base: Int) -> Self {
                    return Self { base: base }
                }
            }

            group Scale {
                let by: Int
            }

            impl Scale with Call {
                fn call(input: Int) -> Int {
                    return input * self.by
                }
            }

            group Tools {
                let adder: Adder
                let scale: Scale
            }

            impl Tools {
                static fn new() -> Self {
                    return Self { adder: Adder.from(100), scale: Scale.by(10) }
                }
            }

            impl Scale {
                static fn by(_ by: Int) -> Self {
                    return Self { by: by }
                }
            }

            fn main() -> Int {
                let adder = Adder.from(1000)
                let tools = Tools.new()
                // 1000 + 3 + 5, then 100 + 1 + 2, then 4 * 10
                let a = adder(a: 3, b: 5)
                let b = tools.adder(a: 1, b: 2)
                let c = tools.scale(4)
                return a + b + c + adder.call(input: (a: 0, b: 0))
            }
            ";
        assert_eq!(run(src), 2151);
    }

    #[test]
    fn trait_functions_are_called_like_any_other() {
        let src = "
            group Counter {
                let count: Int
            }

            trait Start {
                static fn start() -> Self
                fn next() -> Self
            }

            impl Counter with Start {
                static fn start() -> Self {
                    return Self { count: 1 }
                }

                fn next() -> Self {
                    return Self { count: self.count * 3 }
                }
            }

            impl Int with Start {
                static fn start() -> Int {
                    return 2
                }

                fn next() -> Int {
                    return self + 1
                }
            }

            fn main() -> Int {
                return Counter.start().next().next().count + Int.start().next()
            }
            ";
        assert_eq!(run(src), 12);
    }

    #[test]
    fn sized_integers_trap_when_they_overflow() {
        let cases = [
            ("u8_add", "let x: IntU8 = 255\nlet y = x + 1"),
            ("u8_sub", "let x: IntU8 = 0\nlet y = x - 1"),
            ("u8_mul", "let x: IntU8 = 16\nlet y = x * 16"),
            ("u8_div", "let x: IntU8 = 1\nlet y = x / 0"),
            ("s8_add", "let x: IntS8 = 127\nlet y = x + 1"),
            ("s8_neg", "let x: IntS8 = -128\nlet y = -x"),
            ("s8_div", "let x: IntS8 = -128\nlet y = x / -1"),
            (
                "u64_add",
                "let x: IntU64 = 18446744073709551615\nlet y = x + 1",
            ),
        ];
        for (name, body) in cases {
            let src = format!("fn main() -> Int {{\n{body}\nreturn 0\n}}");
            assert!(traps(name, &src), "{name} should trap");
        }
        let src = "fn main() -> Int {\nlet x: IntU8 = 254\nlet y = x + 1\nreturn 0\n}";
        assert!(!traps("u8_max", src));
    }
}
