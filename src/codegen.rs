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
//! LLVM struct of its fields, so `Unit` is `{}`. Its fields are ordered from
//! most to least aligned, rather than as they're declared, so there's no
//! padding between them. A function's parameters are ordered the same way,
//! so they're in the same order as a tuple of them. A choice is a struct of its
//! tag, the index of its option, then space for the largest of its options'
//! values, which they share. The tag is the narrowest integer that can number
//! every option, as `ir::tag_width` says, and a choice with at most one
//! option has none. `ChoiceTag`s and `ChoiceOption`s are as wide as their
//! choice's tag, or `{}` when it has none. Every Atom function becomes an
//! internal LLVM function, so only the C `main` is
//! visible to the linker. It sets every top-level value, then calls Atom's
//! `main`, whose result is the exit code.
//!
//! The executable is optimised with LLVM's standard `-O2` pipeline, which
//! inlines functions and splits groups into their fields. So a copy of a
//! group with one field changed, such as `user = user.revokeAdmin()`, only
//! writes the field that changed.
//!
//! Arithmetic that overflows, division by zero, and shifts that lose bits or
//! go too far trap by calling C's `abort`. `llvm.trap` would avoid needing C, but on macOS it can leave the
//! process hanging instead of stopping it, depending on what launched it.

use std::{fmt, io, num::NonZeroU32, path::Path, process::Command};

use inkwell::{
    AddressSpace,
    IntPredicate::{EQ, NE, SGE, SGT, SLE, SLT, UGE, UGT, ULE, ULT},
    OptimizationLevel,
    basic_block::BasicBlock,
    builder::{Builder, BuilderError},
    context::Context,
    intrinsics::Intrinsic,
    module::{Linkage, Module},
    passes::PassBuilderOptions,
    targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
    },
    types::{BasicType, BasicTypeEnum, IntType},
    values::{BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue, PointerValue},
};

use crate::{
    ir::{
        Body, Expr, IntWidth, MatchArm, Measure, Pattern, Primitive, PrimitiveFn, Program, Stmt,
        TypeDef, tag_width,
    },
    syntax::{
        MIN_POINTER_WIDTH,
        ast::{BinOp, UnOp},
    },
};

/// The target machine for the computer the compiler is running on, which
/// decides how values are laid out in memory, and which instructions are used.
/// Its pointers must be one of the widths integers can be, and at least
/// `MIN_POINTER_WIDTH`.
pub fn host_machine() -> Result<TargetMachine, EmitError> {
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
    check_pointer_bits(pointer_bits(&machine))?;
    Ok(machine)
}

/// The width of pointers that are `bits` wide. Fails if that's narrower than
/// the standard library allows, or isn't one of the widths integers can be.
fn check_pointer_bits(bits: u32) -> Result<IntWidth, EmitError> {
    if bits < MIN_POINTER_WIDTH.bits() {
        return Err(EmitError::PointersTooNarrow(bits));
    }
    IntWidth::from_bits(bits).ok_or(EmitError::PointersUnsupported(bits))
}

/// How many bits wide pointers are on `machine`.
fn pointer_bits(machine: &TargetMachine) -> u32 {
    machine.get_target_data().get_pointer_byte_size(None) * 8
}

/// How wide pointers are on `machine`, which is how wide `IntS` and `IntU`
/// are. `machine` must come from `host_machine`, which checks that they're
/// a width integers can be.
pub fn pointer_width(machine: &TargetMachine) -> IntWidth {
    check_pointer_bits(pointer_bits(machine)).expect("`host_machine` checks pointers' width")
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

/// Optimises `module`, which was compiled for `machine`, with LLVM's
/// standard `-O2` pipeline.
pub fn optimize(module: &Module, machine: &TargetMachine) -> Result<(), EmitError> {
    module
        .run_passes("default<O2>", machine, PassBuilderOptions::create())
        .map_err(|err| EmitError::Optimize(err.to_string()))
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
    /// This computer's pointers are this many bits, which is fewer than
    /// `MIN_POINTER_WIDTH`.
    PointersTooNarrow(u32),
    /// This computer's pointers are this many bits, which is wide enough,
    /// but isn't one of the widths integers can be.
    PointersUnsupported(u32),
    /// LLVM couldn't optimise the module.
    Optimize(String),
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
            Self::PointersTooNarrow(bits) => write!(
                f,
                "this computer's pointers are {bits} bits, but `IntS` and `IntU` need \
                 at least {}",
                MIN_POINTER_WIDTH.bits()
            ),
            Self::PointersUnsupported(bits) => {
                let widths: Vec<_> = IntWidth::ALL
                    .into_iter()
                    .filter(|&width| width >= MIN_POINTER_WIDTH)
                    .map(|width| width.bits().to_string())
                    .collect();
                let (last, rest) = widths.split_last().expect("some widths are supported");
                write!(
                    f,
                    "this computer's pointers are {bits} bits, but `IntS` and `IntU` can \
                     only be {} or {last}",
                    rest.join(", ")
                )
            }
            Self::Optimize(err) => write!(f, "couldn't optimise: {err}"),
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
    /// For each type in `program`, where each of its fields is among its
    /// struct's fields, in the order they're declared, which is empty for
    /// types other than groups.
    slots: Vec<Vec<u32>>,
    /// For each function in `program`, the LLVM function it compiles to.
    functions: Vec<FunctionValue<'ctx>>,
    /// For each function in `program`, the indexes of its parameters, in the
    /// order they're declared, in the order its LLVM function takes them.
    param_orders: Vec<Vec<usize>>,
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
            slots: vec![],
            functions: vec![],
            param_orders: vec![],
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
            let declared = &function.locals[..function.params];
            let order = self.packed_order(declared);
            let params: Vec<_> = order
                .iter()
                .map(|&index| self.types[declared[index]].into())
                .collect();
            let fn_type = self.types[function.returns].fn_type(&params, false);
            let value = self
                .module
                .add_function(&function.name, fn_type, Some(Linkage::Internal));
            self.functions.push(value);
            self.param_orders.push(order);
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
                &TypeDef::Primitive(Primitive::Int { width, .. }) => self.int_type(width).into(),
                TypeDef::Primitive(Primitive::Bool) => self.context.bool_type().into(),
                // A choice without a tag is always the same option, so its
                // tags hold nothing.
                &TypeDef::Primitive(Primitive::Tag { choice }) => match self.tag_type(choice) {
                    Some(tag) => tag.into(),
                    None => self.context.struct_type(&[], false).into(),
                },
                TypeDef::Group { name, .. } | TypeDef::Choice { name, .. } => {
                    self.context.opaque_struct_type(name).into()
                }
            };
            self.types.push(ty);
        }
        self.slots = vec![vec![]; self.types.len()];
        let mut defined = vec![false; self.types.len()];
        for id in 0..self.types.len() {
            self.define_type(id, &mut defined);
        }
    }

    /// Gives the type with `id` its fields, once the types it holds have
    /// theirs, since a choice's size depends on the sizes of its options'
    /// values, and a group's order depends on their alignments. `defined`
    /// records which types have been done. Types can't hold themselves, so
    /// this always finishes.
    fn define_type(&mut self, id: usize, defined: &mut [bool]) {
        if std::mem::replace(&mut defined[id], true) {
            return;
        }
        let program = self.program;
        let held = match &program.types[id] {
            TypeDef::Primitive(_) => return,
            TypeDef::Group { fields, .. } => fields,
            TypeDef::Choice { options, .. } => options,
        };
        for &ty in held {
            self.define_type(ty, defined);
        }

        let body = match &program.types[id] {
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
                let tag = self.tag_type(id).map(BasicTypeEnum::from);
                tag.into_iter().chain([payload.into()]).collect::<Vec<_>>()
            }
            TypeDef::Group { .. } => {
                let order = self.packed_order(held);
                let mut slots = vec![0; held.len()];
                for (slot, &index) in order.iter().enumerate() {
                    slots[index] = slot as u32;
                }
                self.slots[id] = slots;
                order.iter().map(|&index| self.types[held[index]]).collect()
            }
            TypeDef::Primitive(_) => unreachable!("primitives have no fields"),
        };
        self.types[id].into_struct_type().set_body(&body, false);
    }

    /// The indexes of `types`, which are indexes into `program.types`, in the
    /// order that packs them most tightly, which is from most to least
    /// aligned. Every type's ABI size is a multiple of its alignment, which
    /// is a power of 2, so each one after a more aligned one starts aligned.
    /// Types that are as aligned as each other stay in the order they're in.
    /// Groups' fields and functions' parameters are both ordered by this, so
    /// a tuple of a function's parameters holds them in the order it takes
    /// them.
    fn packed_order(&self, types: &[usize]) -> Vec<usize> {
        let mut order: Vec<_> = (0..types.len()).collect();
        let alignment = |index: &usize| {
            let ty = &self.types[types[*index]];
            std::cmp::Reverse(self.target_data.get_abi_alignment(ty))
        };
        order.sort_by_key(alignment);
        order
    }

    /// The LLVM integer that's `width` wide.
    fn int_type(&self, width: IntWidth) -> IntType<'ctx> {
        match width {
            IntWidth::Bits8 => self.context.i8_type(),
            IntWidth::Bits16 => self.context.i16_type(),
            IntWidth::Bits32 => self.context.i32_type(),
            IntWidth::Bits64 => self.context.i64_type(),
        }
    }

    /// The type of the tag of the choice at `choice` in `program.types`,
    /// which is the index of its option, or `None` if it has no tag. That's
    /// when it has at most one option, or isn't a choice, which a
    /// `ChoiceTag` or `ChoiceOption` can be made from, though it then has no
    /// values.
    fn tag_type(&self, choice: usize) -> Option<IntType<'ctx>> {
        let options = match &self.program.types[choice] {
            TypeDef::Choice { options, .. } => options.len(),
            TypeDef::Primitive(_) | TypeDef::Group { .. } => 0,
        };
        tag_width(options).map(|width| self.int_type(width))
    }

    /// The index of the payload of the choice at `choice` in
    /// `program.types` among its struct's fields, which is after its tag if
    /// it has one.
    fn payload_index(&self, choice: usize) -> u32 {
        u32::from(self.tag_type(choice).is_some())
    }

    /// Where the field at `index`, in the order they're declared, of the
    /// group at `group` in `program.types` is among its struct's fields.
    fn slot(&self, group: usize, index: usize) -> u32 {
        self.slots[group][index]
    }

    /// The tag of the option at `index` of the choice at `choice` in
    /// `program.types`, which is `{}` if it has no tag.
    fn tag(&self, choice: usize, index: usize) -> BasicValueEnum<'ctx> {
        match self.tag_type(choice) {
            Some(tag) => tag.const_int(index as u64, false).into(),
            None => self.context.struct_type(&[], false).const_zero().into(),
        }
    }

    /// The choice that the `ChoiceTag` or `ChoiceOption` at `ty` in
    /// `program.types` is a tag of.
    fn tagged_choice(&self, ty: usize) -> usize {
        match self.program.types[ty] {
            TypeDef::Primitive(Primitive::Tag { choice }) => choice,
            TypeDef::Primitive(Primitive::Int { .. } | Primitive::Bool)
            | TypeDef::Group { .. }
            | TypeDef::Choice { .. } => unreachable!("only tags are of a choice"),
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
        // The parameters, in the order they're declared.
        let mut args = value.get_params();
        for (param, &declared) in value.get_param_iter().zip(&self.param_orders[index]) {
            args[declared] = param;
        }
        for (&(slot, _), &arg) in self.locals.iter().zip(&args) {
            self.builder.build_store(slot, arg)?;
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
            // Calls are built as the instructions themselves, but the
            // function is still defined like any other.
            &Body::Primitive(primitive) => {
                let returns = function.returns;
                let result = self
                    .primitive_instruction(primitive, &args, returns)?
                    .expect("only `exit` isn't built in place");
                self.builder.build_return(Some(&result))?;
            }
        }
        Ok(())
    }

    /// Builds the instructions that `primitive` is, on `args`, where a
    /// method's `self` comes first, returning a value of the type at
    /// `returns` in `program.types`, or `None` for `exit`, which isn't built
    /// in place.
    fn primitive_instruction(
        &mut self,
        primitive: PrimitiveFn,
        args: &[BasicValueEnum<'ctx>],
        returns: usize,
    ) -> Result<Option<BasicValueEnum<'ctx>>, BuilderError> {
        let int = |index: usize| args[index].into_int_value();
        Ok(Some(match primitive {
            PrimitiveFn::Exit => return Ok(None),
            // Tags of a choice without a tag are `{}`, since they're always
            // the same option.
            PrimitiveFn::Binary { op, .. } if args[0].is_struct_value() => {
                let same = match op {
                    BinOp::Eq => true,
                    BinOp::Ne => false,
                    _ => unreachable!("tags are only compared with `==` and `!=`"),
                };
                self.context
                    .bool_type()
                    .const_int(u64::from(same), false)
                    .into()
            }
            PrimitiveFn::Binary { op, signed } => self.binary(op, signed, int(0), int(1))?.into(),
            // Negating the smallest integer overflows.
            PrimitiveFn::Unary(UnOp::Neg) => {
                let zero = int(0).get_type().const_zero();
                self.checked("llvm.ssub.with.overflow", zero, int(0))?
                    .into()
            }
            PrimitiveFn::Unary(UnOp::Not | UnOp::BitNot) => {
                self.builder.build_not(int(0), "")?.into()
            }
            PrimitiveFn::ChoiceTag => {
                let choice = self.tagged_choice(returns);
                match self.tag_type(choice) {
                    Some(_) => {
                        self.builder
                            .build_extract_value(args[0].into_struct_value(), 0, "")?
                    }
                    None => self.tag(choice, 0),
                }
            }
            PrimitiveFn::NewChoice => self.choose(returns, args[0], args[1])?,
            // A `ChoiceOption` is already the tag of its option.
            PrimitiveFn::OptionTag => args[0],
            PrimitiveFn::CallFn => self.call_function_value(args[0], args[1], returns)?,
            PrimitiveFn::ConvertInt { from_signed } => {
                let to = self.types[returns].into_int_type();
                self.builder
                    .build_int_cast_sign_flag(int(0), to, from_signed, "")?
                    .into()
            }
            PrimitiveFn::TryConvertInt {
                from_signed,
                to_signed,
            } => self.try_convert_int(int(0), from_signed, to_signed, returns)?,
            PrimitiveFn::Layout { measure, ty } => {
                let llvm = &self.types[ty];
                let bytes = match measure {
                    Measure::Size => self.size(ty),
                    Measure::Stride => self.target_data.get_abi_size(llvm),
                    Measure::Alignment => self.target_data.get_abi_alignment(llvm).into(),
                };
                self.types[returns]
                    .into_int_type()
                    .const_int(bytes, false)
                    .into()
            }
        }))
    }

    /// How many bytes a value of the type at `ty` in `program.types` takes
    /// up, without the padding LLVM puts after it to keep the next one
    /// aligned, which its ABI size includes. That's up to the end of its last
    /// field in memory, or of the largest of a choice's options' values.
    fn size(&self, ty: usize) -> u64 {
        let llvm = self.types[ty];
        let end_of = |index: u32, held: usize| {
            let offset = self
                .target_data
                .offset_of_element(&llvm.into_struct_type(), index)
                .expect("the field is in the struct");
            offset + self.size(held)
        };
        match &self.program.types[ty] {
            TypeDef::Primitive(Primitive::Int { .. } | Primitive::Bool | Primitive::Tag { .. }) => {
                self.target_data.get_store_size(&llvm)
            }
            TypeDef::Group { fields, .. } => fields
                .iter()
                .enumerate()
                .map(|(index, &field)| end_of(self.slot(ty, index), field))
                .max()
                .unwrap_or(0),
            // The tag comes first, if it has one, then the payload the
            // options share.
            TypeDef::Choice { options, .. } => {
                let tag = self
                    .tag_type(ty)
                    .map_or(0, |tag| self.target_data.get_store_size(&tag));
                let payload = self.payload_index(ty);
                options
                    .iter()
                    .map(|&option| end_of(payload, option))
                    .fold(tag, u64::max)
            }
        }
    }

    /// Converts `value` to the integer type held by the `Option` at
    /// `returns` in `program.types`, giving `none` if it's outside that
    /// type's range. Whether each type is signed decides how `value` is read
    /// and how it's widened.
    fn try_convert_int(
        &mut self,
        value: IntValue<'ctx>,
        from_signed: bool,
        to_signed: bool,
        returns: usize,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let TypeDef::Choice { options, .. } = &self.program.types[returns] else {
            unreachable!("lossy conversions give an `Option`");
        };
        // The option holding the integer is `some`, and the other, holding
        // `Unit`, is `none`.
        let is_int = |&ty: &usize| matches!(self.program.types[ty], TypeDef::Primitive(_));
        let some = options
            .iter()
            .position(is_int)
            .expect("`Option` has an option holding the integer");
        let none = 1 - some;
        let to = self.types[options[some]].into_int_type();

        // `value` fits if converting it back gives it again, and it keeps its
        // sign, which only changes between signed and unsigned integers.
        let converted = self
            .builder
            .build_int_cast_sign_flag(value, to, from_signed, "")?;
        let back =
            self.builder
                .build_int_cast_sign_flag(converted, value.get_type(), to_signed, "")?;
        let mut fits = self.builder.build_int_compare(EQ, back, value, "")?;
        let negative = match (from_signed, to_signed) {
            (true, false) => Some(value),
            (false, true) => Some(converted),
            _ => None,
        };
        if let Some(int) = negative {
            let zero = int.get_type().const_zero();
            let positive = self.builder.build_int_compare(SGE, int, zero, "")?;
            fits = self.builder.build_and(fits, positive, "")?;
        }

        let unit = self.types[options[none]].const_zero();
        let some = self.choose(returns, self.tag(returns, some), converted.into())?;
        let none = self.choose(returns, self.tag(returns, none), unit)?;
        self.builder.build_select(fits, some, none, "")
    }

    /// Calls the function that `function`, an `Fn`, holds the address of in
    /// its `RawPointer`. Its parameters are the elements of the tuple
    /// `input`, in the order they're stored, which is the order the function
    /// takes them, and it returns a value of the type at `returns` in
    /// `program.types`.
    fn call_function_value(
        &mut self,
        function: BasicValueEnum<'ctx>,
        input: BasicValueEnum<'ctx>,
        returns: usize,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let pointer = self
            .builder
            .build_extract_value(function.into_struct_value(), 0, "")?;
        let address = self
            .builder
            .build_extract_value(pointer.into_struct_value(), 0, "")?;
        let pointer = self.builder.build_int_to_ptr(
            address.into_int_value(),
            self.context.ptr_type(AddressSpace::default()),
            "",
        )?;
        // A function's `Input` is always a tuple, but an `Fn` with any other
        // `Input` can still be called, though it never has a value.
        let args = match input {
            BasicValueEnum::StructValue(input) => (0..input.get_type().count_fields())
                .map(|index| self.builder.build_extract_value(input, index, ""))
                .collect::<Result<Vec<_>, _>>()?,
            input => vec![input],
        };
        let params: Vec<_> = args.iter().map(|arg| arg.get_type().into()).collect();
        let fn_type = self.types[returns].fn_type(&params, false);
        let args: Vec<_> = args.into_iter().map(Into::into).collect();
        Ok(self
            .builder
            .build_indirect_call(fn_type, pointer, &args, "")?
            .try_as_basic_value()
            .basic()
            .expect("every function returns a value, even if it's `Unit`"))
    }

    /// Builds `op` on `lhs` and `rhs`, which are signed integers if `signed`
    /// is set.
    fn binary(
        &mut self,
        op: BinOp,
        signed: bool,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, BuilderError> {
        let sign = if signed { "s" } else { "u" };
        let ordered = |signed_predicate, unsigned_predicate| {
            if signed {
                signed_predicate
            } else {
                unsigned_predicate
            }
        };
        Ok(match op {
            BinOp::Add => self.checked(&format!("llvm.{sign}add.with.overflow"), lhs, rhs)?,
            BinOp::Sub => self.checked(&format!("llvm.{sign}sub.with.overflow"), lhs, rhs)?,
            BinOp::Mul => self.checked(&format!("llvm.{sign}mul.with.overflow"), lhs, rhs)?,
            BinOp::Div => self.divide(lhs, rhs, signed)?,
            BinOp::Eq => self.builder.build_int_compare(EQ, lhs, rhs, "")?,
            BinOp::Ne => self.builder.build_int_compare(NE, lhs, rhs, "")?,
            BinOp::Lt => self
                .builder
                .build_int_compare(ordered(SLT, ULT), lhs, rhs, "")?,
            BinOp::Le => self
                .builder
                .build_int_compare(ordered(SLE, ULE), lhs, rhs, "")?,
            BinOp::Gt => self
                .builder
                .build_int_compare(ordered(SGT, UGT), lhs, rhs, "")?,
            BinOp::Ge => self
                .builder
                .build_int_compare(ordered(SGE, UGE), lhs, rhs, "")?,
            BinOp::BitAnd | BinOp::And => self.builder.build_and(lhs, rhs, "")?,
            BinOp::BitOr | BinOp::Or => self.builder.build_or(lhs, rhs, "")?,
            BinOp::BitXor => self.builder.build_xor(lhs, rhs, "")?,
            BinOp::ShiftLeft => self.shift_left(lhs, rhs)?,
            BinOp::ShiftRight => {
                let amount = self.shift_amount(lhs, rhs)?;
                self.builder.build_right_shift(lhs, amount, false, "")?
            }
        })
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
        let code = self
            .builder
            .build_int_truncate_or_bit_cast(result, exit_code, "")?;
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
            Expr::Call { function, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<Vec<_>, BuilderError>>()?;
                // A primitive, such as `==` on integers, is built in place
                // rather than called.
                let callee = &self.program.functions[*function];
                if let Body::Primitive(primitive) = callee.body
                    && let Some(result) =
                        self.primitive_instruction(primitive, &args, callee.returns)?
                {
                    return Ok(result);
                }
                let args: Vec<_> = self.param_orders[*function]
                    .iter()
                    .map(|&declared| args[declared].into())
                    .collect();
                self.builder
                    .build_call(self.functions[*function], &args, "")?
                    .try_as_basic_value()
                    .basic()
                    .expect("every function returns a value, even if it's `Unit`")
            }
            Expr::FunctionAddress(index) => self
                .builder
                .build_ptr_to_int(
                    self.functions[*index].as_global_value().as_pointer_value(),
                    self.context.ptr_sized_int_type(&self.target_data, None),
                    "",
                )?
                .into(),
            Expr::Construct { group, fields } => {
                let mut value = self.types[*group].into_struct_type().get_poison();
                for (index, field) in fields.iter().enumerate() {
                    let field = self.expr(field)?;
                    value = self
                        .builder
                        .build_insert_value(value, field, self.slot(*group, index), "")?
                        .into_struct_value();
                }
                value.as_basic_value_enum()
            }
            &Expr::Tag { index, ty } => self.tag(self.tagged_choice(ty), index),
            &Expr::Field {
                ref receiver,
                group,
                index,
            } => {
                let receiver = self.expr(receiver)?.into_struct_value();
                self.builder
                    .build_extract_value(receiver, self.slot(group, index), "")?
            }
            Expr::Error => unreachable!("programs with mistakes aren't compiled"),
        })
    }

    /// Creates the choice at `choice` in `program.types`, as the option whose
    /// tag is `option`, holding `value`. LLVM has no unions, so the value is
    /// written over the choice's payload in memory.
    fn choose(
        &self,
        choice: usize,
        option: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let ty = self.types[choice].into_struct_type();
        let slot = self.entry_alloca(ty)?;
        // Without a tag, it's always the same option.
        if self.tag_type(choice).is_some() {
            let tag = self.builder.build_struct_gep(ty, slot, 0, "")?;
            self.builder.build_store(tag, option)?;
        }
        let payload = self
            .builder
            .build_struct_gep(ty, slot, self.payload_index(choice), "")?;
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
                    let slot = self.slot(ty, index);
                    let ptr = self.builder.build_struct_gep(tuple, ptr, slot, "")?;
                    self.test(element, ptr, field, unmatched)?;
                }
                return Ok(());
            }
            Pattern::Option { index, value } => {
                let TypeDef::Choice { options, .. } = &program.types[ty] else {
                    unreachable!("only choices are matched with option patterns");
                };
                let choice = self.types[ty].into_struct_type();
                // Without a tag, it's always the same option.
                if let Some(tag_type) = self.tag_type(ty) {
                    let tag_ptr = self.builder.build_struct_gep(choice, ptr, 0, "")?;
                    let tag = self.builder.build_load(tag_type, tag_ptr, "")?;
                    let expected = tag_type.const_int(*index as u64, false);
                    self.check(tag.into_int_value(), expected, unmatched)?;
                }
                let payload =
                    self.builder
                        .build_struct_gep(choice, ptr, self.payload_index(ty), "")?;
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
    use crate::syntax::{self, File as AtomFile, parser::parse};

    fn machine() -> TargetMachine {
        host_machine().expect("LLVM should support this computer")
    }

    /// Checks and compiles each `(path, src)` pair as a file, with paths
    /// like `/dir/other`.
    fn compile_files<'ctx>(
        context: &'ctx Context,
        machine: &TargetMachine,
        files: &[(&str, &str)],
    ) -> Module<'ctx> {
        let parsed: Vec<_> = files
            .iter()
            .map(|(path, src)| AtomFile {
                path: path[1..].split('/').map(str::to_string).collect(),
                program: parse(src).expect("source should parse"),
            })
            .collect();
        let program = syntax::check(&parsed, pointer_width(machine)).expect("source should check");
        assert!(program.main.is_some(), "`/main` should declare `main`");
        compile(context, &program, machine)
    }

    /// Compiles the files, then runs them with LLVM's JIT, returning the
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
    fn pointers_must_be_a_width_integers_can_be_of_at_least_16_bits() {
        assert_eq!(check_pointer_bits(16).ok(), Some(IntWidth::Bits16));
        assert_eq!(check_pointer_bits(32).ok(), Some(IntWidth::Bits32));
        assert_eq!(check_pointer_bits(64).ok(), Some(IntWidth::Bits64));
        let error = check_pointer_bits(8).expect_err("8 bits is too few");
        assert_eq!(
            error.to_string(),
            "this computer's pointers are 8 bits, but `IntS` and `IntU` need at least 16"
        );
        for bits in [24, 128] {
            let error = check_pointer_bits(bits).expect_err("integers can't be that wide");
            assert_eq!(
                error.to_string(),
                format!(
                    "this computer's pointers are {bits} bits, but `IntS` and `IntU` can only \
                     be 16, 32 or 64"
                )
            );
        }
        assert!(pointer_width(&machine()) >= MIN_POINTER_WIDTH);
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
    fn compares_comparisons_for_equality() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(Comparison.less.new(()) == Comparison.less.new(()), at: 1)
                let b = bit(Comparison.less.new(()) == Comparison.equal.new(()), at: 2)
                let c = bit(Comparison.equal.new(()) == Comparison.equal.new(()), at: 4)
                let d = bit(Comparison.greater.new(()) == Comparison.less.new(()), at: 8)
                let e = bit(Comparison.greater.new(()) != Comparison.equal.new(()), at: 16)
                let f = bit(Comparison.greater.new(()) != Comparison.greater.new(()), at: 32)
                return a + b + c + d + e + f
            }
            ";
        assert_eq!(run(src), 21);
    }

    #[test]
    fn options_from_the_stdlib_hold_a_value_or_none() {
        let src = "
            fn unwrap(_ value: Option<Int>, or fallback: Int) -> Int {
                match value {
                    Option.some.new(let found) => {
                        return found
                    }
                    Option.none.new(_) => {
                        return fallback
                    }
                }
            }

            fn main() -> Int {
                let a = unwrap(Option.some.new(5), or: 0)
                let b = unwrap(Option<Int>.none.new(()), or: 7)
                return a * 10 + b
            }
            ";
        assert_eq!(run(src), 57);
    }

    #[test]
    fn unwrap_gives_the_value_of_some() {
        let src = "
            fn main() -> Int {
                let nested = Option.some.new(Option.some.new(4))
                return Option.some.new(5).unwrap() * 10 + nested.unwrap().unwrap()
            }
            ";
        assert_eq!(run(src), 54);
    }

    #[test]
    fn unwrap_exits_on_none() {
        let src = "
            fn main() -> Int {
                let nothing: Option<Int> = Option.none.new(())
                return nothing.unwrap()
            }
            ";
        assert_eq!(run_executable("unwrap_none", src).code(), Some(1));
    }

    #[test]
    fn memory_layouts_of_primitives_are_their_widths() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let a = bit(MemoryLayout<IntU8>.size() == 1, at: 1)
                let b = bit(MemoryLayout<IntS16>.stride() == 2, at: 2)
                let c = bit(MemoryLayout<IntU32>.alignment() == 4, at: 4)
                let d = bit(MemoryLayout<IntS64>.size() == 8, at: 8)
                let e = bit(MemoryLayout<Bool>.size() == 1, at: 16)
                let f = bit(MemoryLayout<Bool>.alignment() == 1, at: 32)
                // `Int` and `IntU` are as wide as a pointer.
                let pointer = MemoryLayout<RawPointer>.size()
                let g = bit(MemoryLayout<Int>.size() == pointer, at: 64)
                let h = bit(MemoryLayout<IntU>.stride() == pointer, at: 128)
                // `Unit` takes up no space.
                let i = bit(MemoryLayout<Unit>.size() == 0, at: 256)
                let j = bit(MemoryLayout<Unit>.alignment() == 1, at: 512)
                return a + b + c + d + e + f + g + h + i + j
            }
            ";
        assert_eq!(run(src), 1023);
    }

    #[test]
    fn memory_layout_sizes_leave_out_padding_after_the_value() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            group Padded {
                wide: IntS64
                narrow: IntU8
            }

            alias Pair = (wide: IntS64, narrow: IntU8)

            fn main() -> Int {
                let a = bit(MemoryLayout<Padded>.size() == 9, at: 1)
                let b = bit(MemoryLayout<Padded>.stride() == 16, at: 2)
                let c = bit(MemoryLayout<Padded>.alignment() == 8, at: 4)
                // Tuples are laid out like groups.
                let d = bit(MemoryLayout<Pair>.size() == 9, at: 8)
                let e = bit(MemoryLayout<Pair>.stride() == 16, at: 16)
                return a + b + c + d + e
            }
            ";
        assert_eq!(run(src), 31);
    }

    #[test]
    fn memory_layouts_put_more_aligned_fields_first() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            group Spaced {
                narrow: IntU8
                wide: IntU16
            }

            group Scattered {
                first: IntU8
                wide: IntS64
                last: IntU8
            }

            alias Mixed = (a: IntU8, b: IntS32, c: IntU16)

            fn main() -> Int {
                // `wide` comes first, so there's no padding before it.
                let a = bit(MemoryLayout<Spaced>.size() == 3, at: 1)
                let b = bit(MemoryLayout<Spaced>.stride() == 4, at: 2)
                let c = bit(MemoryLayout<Spaced>.alignment() == 2, at: 4)
                // In the order they're declared, these would be 17 and 24.
                let d = bit(MemoryLayout<Scattered>.size() == 10, at: 8)
                let e = bit(MemoryLayout<Scattered>.stride() == 16, at: 16)
                // Tuples are ordered like groups.
                let f = bit(MemoryLayout<Mixed>.size() == 7, at: 32)
                let g = bit(MemoryLayout<Mixed>.stride() == 8, at: 64)
                return a + b + c + d + e + f + g
            }
            ";
        assert_eq!(run(src), 127);
    }

    #[test]
    fn reordered_fields_keep_their_values() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            group Scattered {
                first: IntU8
                wide: IntS64
                last: IntU8
            }

            impl Scattered {
                static fn new(first: IntU8, wide: IntS64, last: IntU8) -> Scattered {
                    return Self { first: first, wide: wide, last: last }
                }
            }

            fn combined(first: IntU8, wide: IntS64, last: IntU8) -> Int {
                let digits = IntS64.new(first) * 100 + wide * 10 + IntS64.new(last)
                return Int.new(digits).unwrap()
            }

            alias Elements = (first: IntU8, wide: IntS64, last: IntU8)

            fn matched(_ elements: Elements) -> Int {
                match elements {
                    (first: let first, wide: let wide, last: let last) => {
                        return combined(first: first, wide: wide, last: last)
                    }
                }
            }

            fn main() -> Int {
                let s = Scattered.new(first: 1, wide: 2, last: 3)
                let a = bit(combined(first: s.first, wide: s.wide, last: s.last) == 123, at: 1)

                let t: Elements = (first: 4, wide: 5, last: 6)
                let b = bit(combined(first: t.first, wide: t.wide, last: t.last) == 456, at: 2)

                // Patterns and functions as values take elements in the order
                // they're declared.
                let c = bit(matched((first: 7, wide: 8, last: 9)) == 789, at: 4)
                let function: Fn<Elements, Int> = combined
                let d = bit(function(first: 1, wide: 2, last: 4) == 124, at: 8)
                return a + b + c + d
            }
            ";
        assert_eq!(run(src), 15);
    }

    #[test]
    fn parameters_are_ordered_like_a_tuple_of_them() {
        let src = "
            fn combined(first: IntU8, wide: IntS64, last: IntU8) -> Int {
                return 0
            }

            impl IntU8 {
                fn widened(by amount: IntS32) -> Int {
                    return 0
                }
            }

            fn main() -> Int {
                let byte: IntU8 = 4
                return combined(first: 1, wide: 2, last: 3) + byte.widened(by: 5)
            }
            ";
        let context = Context::create();
        let module = compile_files(&context, &machine(), &[("/main", src)]);
        let params = |name: &str| -> Vec<String> {
            let function = module
                .get_function(name)
                .unwrap_or_else(|| panic!("should compile `{name}`"));
            function
                .get_type()
                .get_param_types()
                .iter()
                .map(|ty| ty.print_to_string().to_string())
                .collect()
        };
        assert_eq!(params("/main::combined"), ["i64", "i8", "i8"]);
        // A method's `self` is ordered with the rest of its parameters.
        assert_eq!(params("/main::IntU8.widened"), ["i32", "i8"]);
    }

    #[test]
    fn memory_layouts_of_choices_include_a_tag_if_they_need_one() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            group Padded {
                wide: IntS64
                narrow: IntU8
            }

            choice Wrapper {
                value: IntS64
            }

            fn main() -> Int {
                // The tag of a choice with 2 options is 1 byte, then the
                // payload is after it.
                let a = bit(MemoryLayout<Option<IntU8>>.size() == 2, at: 1)
                let b = bit(MemoryLayout<Option<IntU8>>.stride() == 2, at: 2)
                let c = bit(MemoryLayout<Option<IntU8>>.alignment() == 1, at: 4)
                // The payload is aligned for its largest value.
                let d = bit(MemoryLayout<Option<IntS64>>.size() == 16, at: 8)
                let e = bit(MemoryLayout<Option<IntS64>>.alignment() == 8, at: 16)
                let f = bit(MemoryLayout<Option<Padded>>.size() == 17, at: 32)
                let g = bit(MemoryLayout<Option<Padded>>.stride() == 24, at: 64)
                // Choices with at most one option have no tag.
                let h = bit(MemoryLayout<Never>.size() == 0, at: 128)
                let i = bit(MemoryLayout<Wrapper>.size() == 8, at: 256)
                let j = bit(MemoryLayout<Wrapper>.alignment() == 8, at: 512)
                // Tags are as wide as their choice's.
                let k = bit(MemoryLayout<ChoiceTag<Option<IntU8>>>.size() == 1, at: 1024)
                let l = bit(MemoryLayout<ChoiceTag<Wrapper>>.size() == 0, at: 2048)
                return a + b + c + d + e + f + g + h + i + j + k + l
            }
            ";
        assert_eq!(run(src), 4095);
    }

    #[test]
    fn memory_layouts_measure_what_generic_types_are_given() {
        let src = "
            group Box<Value> {
                value: Value
            }

            impl Box {
                static fn valueSize() -> IntU {
                    return MemoryLayout<Self.Value>.size()
                }
            }

            fn main() -> Int {
                let sizes = Box<IntU8>.valueSize() + 10 * Box<IntS32>.valueSize()
                return Int.new(sizes).unwrap()
            }
            ";
        assert_eq!(run(src), 41);
    }

    #[test]
    fn lossless_integer_conversions_keep_the_value() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }
            fn main() -> Int {
                let small: IntS8 = -5
                let byte: IntU8 = 200
                let big: IntU32 = 4000000000
                let a = bit(IntS16.new(small) == -5, at: 1)
                let b = bit(IntS64.new(small) == -5, at: 2)
                let c = bit(IntU16.new(byte) == 200, at: 4)
                // Unsigned integers are widened with zeros, even when signed.
                let d = bit(IntS16.new(byte) == 200, at: 8)
                let e = bit(IntS64.new(big) == 4000000000, at: 16)
                let f = bit(Int.new(small) == -5, at: 32)
                let g = bit(IntU.new(byte) == 200, at: 64)
                let h = bit(Int.new(IntS16.new(small)) == -5, at: 128)
                let i = bit(IntU.new(IntU16.new(byte)) == 200, at: 256)
                return a + b + c + d + e + f + g + h + i
            }
            ";
        assert_eq!(run(src), 511);
    }

    #[test]
    fn lossy_integer_conversions_give_none_outside_the_range() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }
            fn main() -> Int {
                let lowest: IntS16 = -128
                let tooLow: IntS16 = -129
                let highest: IntS16 = 127
                let tooHigh: IntS16 = 128
                let a = bit(IntS8.new(lowest).unwrap() == -128, at: 1)
                let b = bit(IntS8.new(tooLow) == Option<IntS8>.none.new(()), at: 2)
                let c = bit(IntS8.new(highest).unwrap() == 127, at: 4)
                let d = bit(IntS8.new(tooHigh) == Option<IntS8>.none.new(()), at: 8)

                let max: IntU64 = 18446744073709551615
                let byte: IntU64 = 255
                let overByte: IntU64 = 256
                let e = bit(IntU8.new(byte).unwrap() == 255, at: 16)
                let f = bit(IntU8.new(overByte) == Option<IntU8>.none.new(()), at: 32)
                let g = bit(IntU32.new(max) == Option<IntU32>.none.new(()), at: 64)
                let h = bit(IntS64.new(IntU64.new(Int.new(byte).unwrap()).unwrap()).unwrap() == 255, at: 128)
                // `IntS` and `IntU` could be wider than 64 bits.
                let i = bit(IntS64.new(Int.new(lowest)).unwrap() == -128, at: 256)
                let j = bit(IntU64.new(IntU.new(byte).unwrap()).unwrap() == 255, at: 512)
                return a + b + c + d + e + f + g + h + i + j
            }
            ";
        assert_eq!(run(src), 1023);
    }

    #[test]
    fn lossy_integer_conversions_give_none_when_the_sign_changes() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }
            fn main() -> Int {
                let minusOne: IntS8 = -1
                let five: IntS8 = 5
                let a = bit(IntU8.new(minusOne) == Option<IntU8>.none.new(()), at: 1)
                let b = bit(IntU64.new(minusOne) == Option<IntU64>.none.new(()), at: 2)
                let c = bit(IntU8.new(five).unwrap() == 5, at: 4)
                let d = bit(IntU64.new(five).unwrap() == 5, at: 8)

                let high: IntU8 = 200
                let half: IntU8 = 127
                let e = bit(IntS8.new(high) == Option<IntS8>.none.new(()), at: 16)
                let f = bit(IntS8.new(half).unwrap() == 127, at: 32)

                // Out of range for `IntS64` and `IntS8`, though it truncates
                // to a value in range.
                let max: IntU64 = 18446744073709551615
                let g = bit(IntS64.new(max) == Option<IntS64>.none.new(()), at: 64)
                let h = bit(IntS8.new(max) == Option<IntS8>.none.new(()), at: 128)
                let lowest: IntS64 = -9223372036854775808
                let i = bit(IntU64.new(lowest) == Option<IntU64>.none.new(()), at: 256)
                let j = bit(IntU8.new(lowest) == Option<IntU8>.none.new(()), at: 512)
                return a + b + c + d + e + f + g + h + i + j
            }
            ";
        assert_eq!(run(src), 1023);
    }

    #[test]
    fn options_are_equal_when_their_values_are() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let one = Option.some.new(1)
                let nothing: Option<Int> = Option.none.new(())
                let yes = Option.some.new(Option.some.new(true))
                let maybe: Option<Option<Bool>> = Option.some.new(Option.none.new(()))
                let a = bit(one == Option.some.new(1), at: 1)
                let b = bit(one == Option.some.new(2), at: 2)
                let c = bit(nothing == Option.none.new(()), at: 4)
                let d = bit(one != nothing, at: 8)
                let e = bit(yes == Option.some.new(Option.some.new(true)), at: 16)
                let f = bit(yes == maybe, at: 32)
                let g = bit(one.isEqual(to: nothing), at: 64)
                return a + b + c + d + e + f + g
            }
            ";
        assert_eq!(run(src), 29);
    }

    #[test]
    fn constrained_impls_call_their_arguments_functions() {
        // `sum` has whatever type `T`'s `add` returns, which is only known
        // once `total` is made for a `T`.
        let src = "
            group Box<T> {
                value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }
            }

            impl Box with Equal where Self.T == some Equal<Rhs = Self.T> {
                fn isEqual(to rhs: Self) -> Bool {
                    return Option.some.new(self.value) == Option.some.new(rhs.value)
                }
            }

            impl Box where Self.T == some Add<Rhs = Self.T>, Self.T == some Equal<Rhs = Self.T> {
                fn total(with rhs: Self) -> Int {
                    let sum = self.value + rhs.value
                    let boxed = Box.of(sum)
                    if rhs.value == self.value {
                        return 1
                    }
                    return 2
                }
            }

            fn main() -> Int {
                let a = Box.of(3) == Box.of(3)
                let b = Box.of(Box.of(true)) == Box.of(Box.of(false))
                let c = Box.of(4).total(with: Box.of(5))
                if a & !b {
                    return c * 10
                }
                return 0
            }
            ";
        assert_eq!(run(src), 20);
    }

    #[test]
    fn generic_types_call_their_constrained_arguments_functions() {
        // `made` is whatever `T`'s `Output` is, which is only known once
        // `total` is made for a `T`.
        let src = "
            trait Maker {
                alias Output

                fn make() -> Output
                fn count(of output: Output) -> Int
            }

            group Factory<T> where T == some Maker {
                maker: T
            }

            impl Factory {
                static fn of(_ maker: Self.T) -> Self {
                    return Self { maker: maker }
                }

                fn total() -> Int {
                    let made = self.maker.make()
                    return self.maker.count(of: made)
                }
            }

            group Pair {
                value: Int
            }

            impl Pair {
                static fn of(_ value: Int) -> Self {
                    return Self { value: value }
                }
            }

            impl Pair with Maker {
                fn make() -> (first: Int, second: Int) {
                    return (first: self.value, second: self.value + 1)
                }

                fn count(of output: (first: Int, second: Int)) -> Int {
                    return output.first * 10 + output.second
                }
            }

            impl Bool with Maker {
                fn make() -> Int {
                    if self {
                        return 1
                    }
                    return 0
                }

                fn count(of output: Int) -> Int {
                    return output * 100
                }
            }

            fn main() -> Int {
                return Factory.of(Pair.of(4)).total() + Factory.of(true).total()
            }
            ";
        assert_eq!(run(src), 145);
    }

    #[test]
    fn compare_on_integers_gives_their_comparison() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let small: IntS8 = -1
                let byte: IntU8 = 200
                let a = bit(1.compare(to: 2) == Comparison.less.new(()), at: 1)
                let b = bit(2.compare(to: 2) == Comparison.equal.new(()), at: 2)
                let c = bit(3.compare(to: 2) == Comparison.greater.new(()), at: 4)
                let d = bit(small.compare(to: 0) == Comparison.less.new(()), at: 8)
                let e = bit(byte.compare(to: 100) == Comparison.greater.new(()), at: 16)
                return a + b + c + d + e
            }
            ";
        assert_eq!(run(src), 31);
    }

    #[test]
    fn ordering_operators_default_to_compare() {
        // Each check is one bit of the result.
        let src = "
            group Score {
                points: Int
            }

            impl Score {
                static fn of(points: Int) -> Self {
                    return Self { points: points }
                }
            }

            impl Score with Compare {
                fn compare(to rhs: Self) -> Comparison {
                    return self.points.compare(to: rhs.points)
                }
            }

            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn main() -> Int {
                let one = Score.of(points: 1)
                let two = Score.of(points: 2)
                let a = bit(one < two, at: 1)
                let b = bit(two < two, at: 2)
                let c = bit(two <= two, at: 4)
                let d = bit(two <= one, at: 8)
                let e = bit(two > one, at: 16)
                let f = bit(two > two, at: 32)
                let g = bit(two >= two, at: 64)
                let h = bit(one >= two, at: 128)
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
    fn unit_functions_can_finish_without_return() {
        let src = "
            fn nothing() {}

            fn early(stop: Bool) {
                if stop {
                    return
                }
                let unused = ()
            }

            fn main() -> Int {
                nothing()
                early(stop: true)
                early(stop: false)
                let unit: Unit = nothing()
                return 5
            }
            ";
        assert_eq!(run(src), 5);
    }

    #[test]
    fn groups_have_fields_and_methods() {
        let src = "
            group Point {
                x: Int
                y: Int
            }

            group Line {
                start: Point
                end: Point
            }

            impl Point {
                static fn at(x: Int, y: Int) -> Point {
                    return Self { x: x, y: y }
                }

                fn sum() -> Int {
                    return self.x + self.y
                }
            }

            impl Line {
                static fn from(_ start: Point, to end: Point) -> Line {
                    return Self { start: start, end: end }
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
                    first: Int
                    second: Int
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
    fn compiles_across_files() {
        let files = [
            (
                "/main",
                "
                use ./shapes/

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
                    side: Int
                }

                impl Square {
                    static fn new(side: Int) -> Square {
                        return Self { side: side }
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

    #[test]
    fn default_trait_functions_call_names_from_their_own_file() {
        let files = [
            (
                "/main",
                "
                use /greet

                group User {}

                impl User {
                    static fn new() -> User {
                        return Self {}
                    }
                }

                impl User with Greet {}

                fn helper() -> Int {
                    return 1
                }

                fn main() -> Int {
                    return User.new().greet() + helper()
                }
                ",
            ),
            (
                "/greet",
                "
                use /helpers

                trait Greet {
                    fn greet() -> Int {
                        return helper()
                    }
                }
                ",
            ),
            (
                "/helpers",
                "
                fn helper() -> Int {
                    return 20
                }
                ",
            ),
        ];
        assert_eq!(run_files(&files), 21);
    }

    #[test]
    fn default_trait_functions_are_separate_for_each_impl() {
        let thing = |value: i32| {
            format!(
                "
                use /greet

                group Thing {{}}

                impl Thing {{
                    static fn new() -> Thing {{
                        return Self {{}}
                    }}
                }}

                impl Thing with Greet {{
                    fn value() -> Int {{
                        return {value}
                    }}
                }}
                "
            )
        };
        let (a, b) = (thing(1), thing(2));
        let files = [
            (
                "/main",
                "
                use /a
                use /b

                fn main() -> Int {
                    return a::Thing.new().greet() * 10 + b::Thing.new().greet()
                }
                ",
            ),
            (
                "/greet",
                "
                trait Greet {
                    fn value() -> Int

                    fn greet() -> Int {
                        return self.value()
                    }
                }
                ",
            ),
            ("/a", a.as_str()),
            ("/b", b.as_str()),
        ];
        assert_eq!(run_files(&files), 12);
    }

    #[test]
    fn files_with_the_same_name_can_be_imported_without_qualifiers() {
        let files = [
            (
                "/main",
                "
                use /a/

                fn main() -> Int {
                    return p() * 10 + q()
                }
                ",
            ),
            ("/a/b/x", "fn p() -> Int { return 1 }"),
            ("/a/x", "fn q() -> Int { return 2 }"),
        ];
        assert_eq!(run_files(&files), 12);
    }

    #[test]
    fn compiles_parent_paths() {
        let files = [
            (
                "/main",
                "
                use ./a/b/caller

                fn main() -> Int {
                    return call()
                }
                ",
            ),
            (
                "/a/b/caller",
                "
                use ../sibling
                use ../../root as top

                fn call() -> Int {
                    return sibling::value() * 10 + top::value()
                }
                ",
            ),
            ("/a/sibling", "fn value() -> Int { return 1 }"),
            ("/root", "fn value() -> Int { return 2 }"),
        ];
        assert_eq!(run_files(&files), 12);
    }

    #[test]
    fn qualifies_names_by_file_name_and_alias() {
        let names = [
            (
                "/main",
                "
                use ./filea
                use ./fileb

                fn main() -> Int {
                    return filea::temp() * 10 + fileb::temp()
                }
                ",
            ),
            ("/filea", "fn temp() -> Int { return 1 }"),
            ("/fileb", "fn temp() -> Int { return 2 }"),
        ];
        assert_eq!(run_files(&names), 12);

        let aliases = [
            (
                "/main",
                "
                use ./filea as aliasa
                use ./dir/filea as aliasb

                fn main() -> Int {
                    return aliasa::temp() * 10 + aliasb::temp()
                }
                ",
            ),
            ("/filea", "fn temp() -> Int { return 1 }"),
            ("/dir/filea", "fn temp() -> Int { return 2 }"),
        ];
        assert_eq!(run_files(&aliases), 12);
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
        let tag = choice
            .get_field_type_at_index(0)
            .expect("the choice should have a tag")
            .into_int_type();
        reader("read_tag", 0, tag).expect("should build the reader");
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
            empty: Unit
            flag: Bool
        }
        ";

    #[test]
    fn choices_hold_their_option_and_its_value() {
        let src = format!(
            "
            {SHAPE}
            let circle = Shape.circle.new(3)
            let flag = Shape.flag.new(true)
            let empty = Shape.empty.new(())
            fn main() -> Int {{
                return 0
            }}
            "
        );
        assert_eq!(read_choice(&src, "circle", "Shape", 64), (0, 3));
        assert_eq!(read_choice(&src, "flag", "Shape", 8), (2, 1));
        assert_eq!(read_choice(&src, "empty", "Shape", 8).0, 1);
    }

    /// Returns which of `SHAPE`'s options `shape` is, and its value, for
    /// tests.
    const DESCRIBE_SHAPE: &str = "
        fn describe(shape: Shape) -> Int {
            match shape {
                Shape.circle.new(let radius) => {
                    return radius
                }
                Shape.empty.new(_) => {
                    return 100
                }
                Shape.flag.new(let flag) => {
                    if flag {
                        return 200
                    }
                    return 300
                }
            }
        }
        ";

    #[test]
    fn options_create_their_choice_with_new() {
        let src = format!(
            "
            {SHAPE}
            {DESCRIBE_SHAPE}
            group Maker {{
                make: ChoiceOption<Shape, Int>
            }}

            impl Maker {{
                static fn new() -> Maker {{
                    return Self {{ make: Shape.circle }}
                }}
            }}

            fn make(from option: ChoiceOption<Shape, Int>) -> Shape {{
                return option.new(4)
            }}

            fn main() -> Int {{
                let circle = Shape.circle
                let empty = Shape.empty
                let a = describe(shape: circle.new(1))
                let b = describe(shape: Shape.circle.new(2))
                let c = describe(shape: make(from: Shape.circle))
                let d = describe(shape: Maker.new().make.new(8))
                let e = describe(shape: empty.new(()))
                let f = describe(shape: Shape.empty.new(()))
                let stored = Shape.flag
                let g = describe(shape: stored.new(true))
                return a + b + c + d + e + f + g
            }}
            "
        );
        assert_eq!(run(&src), 415);
    }

    #[test]
    fn options_holding_unit_create_their_choice_without_a_value() {
        let src = format!(
            "
            {SHAPE}
            {DESCRIBE_SHAPE}
            fn orNothing(_ value: Option<Int>) -> Int {{
                match value {{
                    .some.new(let found) => {{
                        return found
                    }}
                    .none.new(_) => {{
                        return 5
                    }}
                }}
            }}

            fn main() -> Int {{
                let empty = Shape.empty
                let a = describe(shape: empty.new())
                let b = describe(shape: Shape.empty.new())
                let shape: Shape = .empty.new()
                let c = describe(shape: shape)
                let d = orNothing(Option.none.new())
                let e = orNothing(.none.new())
                return a + b + c + d + e
            }}
            "
        );
        assert_eq!(run(&src), 310);
    }

    #[test]
    fn calls_choose_between_methods_sharing_a_name() {
        // Each method is one digit of the result. `Box<Int>` uses its
        // specialised `get`, which has the same signature as the other.
        let src = "
            group Box<T> {
                value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }

                fn get() -> Self.T {
                    return self.value
                }

                fn show(_ value: Int) -> Int {
                    return 1
                }

                fn show(_ value: Bool) -> Int {
                    return 20
                }

                fn show(times value: Int) -> Int {
                    return 300 * value
                }

                fn make() -> Int {
                    return 4000
                }

                fn make() -> Bool {
                    return true
                }
            }

            impl Box where Self.T == Int {
                fn get() -> Int {
                    return self.value + 50000
                }
            }

            fn main() -> Int {
                let box = Box.of(true)
                let a = box.show(1) + box.show(true) + box.show(times: 1)
                let b: Int = box.make()
                let c: Bool = box.make()
                if c & box.get() {
                    return a + b + Box.of(0).get()
                }
                return 0
            }
            ";
        assert_eq!(run(src), 54321);
    }

    #[test]
    fn stored_options_can_be_chosen_while_running() {
        let src = "
            choice Number {
                small: Int
                large: Int
            }

            fn read(number: Number) -> Int {
                match number {
                    Number.small.new(let value) => {
                        return value
                    }
                    Number.large.new(let value) => {
                        return value * 100
                    }
                }
            }

            fn pick(large: Bool) -> ChoiceOption<Number, Int> {
                if large {
                    return Number.large
                }
                return Number.small
            }

            fn main() -> Int {
                let large = pick(large: true)
                let small = pick(large: false)
                return read(number: large.new(2)) + read(number: small.new(3))
            }
            ";
        assert_eq!(run(src), 203);
    }

    #[test]
    fn options_can_be_top_level_values() {
        let src = format!(
            "
            {SHAPE}
            let option = Shape.flag
            let flag = option.new(true)
            fn main() -> Int {{
                return 0
            }}
            "
        );
        assert_eq!(read_choice(&src, "flag", "Shape", 8), (2, 1));
    }

    #[test]
    fn options_can_hold_tuples_when_stored() {
        let src = "
            choice Shape {
                empty: Unit
                pair: (a: Int, b: Int)
            }

            fn main() -> Int {
                let direct = Shape.pair.new((a: 1, b: 2))
                let pair = Shape.pair
                let stored = pair.new((a: 3, b: 4))
                var total = 0
                match direct {
                    Shape.pair.new((a: let a, b: let b)) => {
                        total = total + a * 10 + b
                    }
                    Shape.empty.new(_) => {}
                }
                match stored {
                    Shape.pair.new((a: let a, b: let b)) => {
                        total = total + a * 1000 + b * 100
                    }
                    Shape.empty.new(_) => {}
                }
                return total
            }
            ";
        assert_eq!(run(src), 3412);
    }

    #[test]
    fn options_can_hold_a_value_a_tuple_or_nothing() {
        let src = "
            choice Thing {
                first: Int
                second: (a: Int, b: Int)
                third: Unit
            }

            fn value(of thing: Thing) -> Int {
                match thing {
                    Thing.first.new(let a) => {
                        return a
                    }
                    Thing.second.new((a: let a, b: let b)) => {
                        return a * 10 + b
                    }
                    Thing.third.new(()) => {
                        return 5
                    }
                }
            }

            fn main() -> Int {
                let first = value(of: Thing.first.new(1))
                let second = value(of: Thing.second.new((a: 2, b: 3)))
                let third = value(of: Thing.third.new(()))
                return first * 1000 + second * 10 + third
            }
            ";
        assert_eq!(run(src), 1235);
    }

    #[test]
    fn tags_tell_options_apart() {
        let src = format!(
            "
            {SHAPE}
            fn score(matched: Bool, worth: Int) -> Int {{
                if matched {{
                    return worth
                }}
                return 0
            }}

            fn main() -> Int {{
                let shape = Shape.flag.new(true)
                let tag: ChoiceTag<Shape> = ChoiceTag.new(shape)
                let option = Shape.flag
                let a = score(matched: tag == Shape.flag.tag(), worth: 1)
                let b = score(matched: tag == Shape.circle.tag(), worth: 2)
                let c = score(matched: tag != Shape.circle.tag(), worth: 4)
                let d = score(matched: tag != option.tag(), worth: 8)
                let e = score(matched: tag == ChoiceTag.new(option.new(false)), worth: 16)
                let f = score(matched: ChoiceTag<Shape>.new(Shape.empty.new(())) == Shape.empty.tag(), worth: 32)
                let read = ChoiceTag<Shape>.new
                let g = score(matched: read(shape) == tag, worth: 64)
                return a + b + c + d + e + f + g
            }}
            "
        );
        assert_eq!(run(&src), 117);
    }

    /// A choice called `Many` with `count` options, from `o0` up, which hold
    /// nothing.
    fn many_options(count: usize) -> String {
        let options: String = (0..count).map(|index| format!("o{index}\n")).collect();
        format!("choice Many {{\n{options}}}\n")
    }

    #[test]
    fn tags_are_as_wide_as_their_options_need() {
        for (count, bytes) in [(256, 1), (257, 2), (65536, 2), (65537, 4)] {
            let last = count - 1;
            let src = format!(
                "
                {}
                fn main() -> Int {{
                    if MemoryLayout<Many>.size() != {bytes} {{
                        return 1
                    }}
                    // The last option's tag would be the first's if it
                    // didn't fit.
                    let first: ChoiceTag<Many> = ChoiceTag.new(Many.o0.new())
                    if ChoiceTag.new(Many.o{last}.new()) == first {{
                        return 2
                    }}
                    match Many.o{last}.new() {{
                        .o{last}.new(_) => {{
                            return 0
                        }}
                        _ => {{
                            return 3
                        }}
                    }}
                }}
                ",
                many_options(count)
            );
            assert_eq!(run(&src), 0, "{count} options");
        }
    }

    #[test]
    fn choices_with_one_option_work_without_a_tag() {
        let src = "
            choice Wrapper {
                value: Int
            }

            fn unwrapped(_ wrapper: Wrapper) -> Int {
                match wrapper {
                    .value.new(let value) => {
                        return value
                    }
                }
            }

            fn main() -> Int {
                let a = Wrapper.value.new(3)
                let b = Wrapper.value.new(4)
                let tag: ChoiceTag<Wrapper> = ChoiceTag.new(a)
                if tag != ChoiceTag.new(b) {
                    return 100
                }
                if tag != Wrapper.value.tag() {
                    return 200
                }
                return unwrapped(a) * 10 + unwrapped(b)
            }
            ";
        assert_eq!(run(src), 34);
    }

    #[test]
    fn options_are_equal_if_they_are_the_same_option() {
        let src = "
            choice Size {
                small: Int
                large: Int
            }

            fn score(matched: Bool, worth: Int) -> Int {
                if matched {
                    return worth
                }
                return 0
            }

            fn main() -> Int {
                let option: ChoiceOption<Size, Int> = Size.small
                let a = score(matched: option == Size.small, worth: 1)
                let b = score(matched: option == Size.large, worth: 2)
                let c = score(matched: option != Size.large, worth: 4)
                let d = score(matched: option != Size.small, worth: 8)
                return a + b + c + d
            }
            ";
        assert_eq!(run(src), 5);
    }

    #[test]
    fn generic_arguments_inferred_from_calls_compile() {
        let src = "
            choice Optional<T> {
                none: Unit
                some: T
            }

            group Box<T> {
                value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }

                static fn holding(_ value: Self.T, fallback: Optional<Self.T>) -> Self {
                    match fallback {
                        Optional.some.new(let found) => {
                            return Self { value: found }
                        }
                        Optional.none.new(_) => {
                            return Self { value: value }
                        }
                    }
                }
            }

            fn read(optional: Optional<Int>) -> Int {
                match optional {
                    Optional.some.new(let found) => {
                        return found
                    }
                    Optional.none.new(_) => {
                        return 0
                    }
                }
            }

            fn main() -> Int {
                let a = Box.of(1).value
                let b = Box.of(Box.of(20)).value.value
                let c = read(optional: Optional.some.new(300))
                let d = Box.holding(0, fallback: Optional.some.new(4000)).value
                let shape = Optional.some.new(true)
                if ChoiceTag.new(shape) == Optional<Bool>.some.tag() {
                    return a + b + c + d
                }
                return 0
            }
            ";
        assert_eq!(run(src), 4321);
    }

    #[test]
    fn generic_choices_have_options_and_tags() {
        let src = "
            choice Optional<T> {
                none: Unit
                some: T
            }

            fn unwrap(_ value: Optional<Int>, or fallback: Int) -> Int {
                match value {
                    Optional.some.new(let found) => {
                        return found
                    }
                    Optional.none.new(_) => {
                        return fallback
                    }
                }
            }

            fn main() -> Int {
                let some: ChoiceOption<Optional<Int>, Int> = Optional.some
                let none: ChoiceOption<Optional<Int>, Unit> = Optional.none
                let a = unwrap(some.new(5), or: 0)
                let b = unwrap(none.new(()), or: 7)
                if ChoiceTag<Optional<Int>>.new(some.new(1)) == Optional<Int>.some.tag() {
                    return a * 10 + b
                }
                return 0
            }
            ";
        assert_eq!(run(src), 57);
    }

    #[test]
    fn options_and_tags_work_as_top_level_values() {
        let src = "
            choice Shape {
                square: Int
                circle: Bool
                other: Unit
            }

            let a: ChoiceOption<Shape, Int> = Shape.square
            let b: ChoiceTag<Shape> = Shape.square.tag()
            let c: Shape = Shape.square.new(5)
            let d: ChoiceTag<Shape> = ChoiceTag.new(Shape.square.new(5))

            let n: ChoiceOption<Shape, Unit> = Shape.other
            let o: ChoiceTag<Shape> = Shape.other.tag()
            let p: Shape = Shape.other.new(())
            let q: ChoiceTag<Shape> = ChoiceTag.new(Shape.other.new(()))

            fn main() -> Int {
                return 0
            }
            ";
        assert_eq!(read_choice(src, "c", "Shape", 64), (0, 5));
        assert_eq!(read_choice(src, "p", "Shape", 8).0, 2);
    }

    #[test]
    fn choices_can_hold_groups_and_choices() {
        let src = format!(
            "
            {SHAPE}
            group Point {{
                x: Int
                y: Int
            }}

            impl Point {{
                static fn at(x: Int, y: Int) -> Point {{
                    return Self {{ x: x, y: y }}
                }}
            }}

            choice Drawing {{
                shape: Shape
                point: Point
            }}

            let shape = Drawing.shape.new(Shape.flag.new(false))
            let point = Drawing.point.new(Point.at(x: 7, y: 8))
            fn main() -> Int {{
                return 0
            }}
            "
        );
        // The shape's payload starts with its own tag.
        assert_eq!(read_choice(&src, "shape", "Drawing", 8), (0, 2));
        assert_eq!(read_choice(&src, "point", "Drawing", 64), (1, 7));
    }

    #[test]
    fn choices_can_be_passed_around() {
        let src = format!(
            "
            {SHAPE}
            impl Shape {{
                static fn unit() -> Shape {{
                    return Shape.circle.new(1)
                }}

                fn sides() -> Int {{
                    return 4
                }}
            }}

            group Canvas {{
                shape: Shape
            }}

            impl Canvas {{
                static fn of(_ shape: Shape) -> Canvas {{
                    return Self {{ shape: shape }}
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
                    only: Unit
                }}
                let local = Local.only.new(())
                let canvas = Canvas.of(keep(Shape.unit()))
                return canvas.shape.sides() * 10 + Shape.empty.new(()).sides()
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
                    Shape.circle.new(let radius) => {{
                        result = radius * 10
                    }}
                    Shape.empty.new(_) => {{
                        result = 1
                    }}
                    Shape.flag.new(let flag) => {{
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
                let a = describe(Shape.circle.new(5))
                let b = describe(Shape.empty.new(()))
                let c = describe(Shape.flag.new(true))
                let d = describe(Shape.flag.new(false))
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
                        Shape.circle.new(let radius) => {{
                            return radius
                        }}
                        Shape.empty.new(_) => {{
                            return 0
                        }}
                        Shape.flag.new(_) => {{
                            return 1
                        }}
                    }}
                }}
            }}

            fn absurd(_ never: Never) -> Int {{
                match never {{}}
            }}

            fn main() -> Int {{
                return Shape.circle.new(7).size() * 10 + Shape.flag.new(true).size()
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
                x: Int
                y: Int
            }}

            impl Point {{
                static fn at(x: Int, y: Int) -> Point {{
                    return Self {{ x: x, y: y }}
                }}
            }}

            choice Drawing {{
                shape: Shape
                point: Point
            }}

            fn total(of drawing: Drawing) -> Int {{
                match drawing {{
                    Drawing.shape.new(let shape) => {{
                        match shape {{
                            Shape.circle.new(let radius) => {{
                                return radius
                            }}
                            Shape.empty.new(_) => {{
                                return 0
                            }}
                            Shape.flag.new(_) => {{
                                return 0
                            }}
                        }}
                    }}
                    Drawing.point.new(let point) => {{
                        return point.x + point.y
                    }}
                }}
            }}

            fn main() -> Int {{
                let circle = total(of: Drawing.shape.new(Shape.circle.new(4)))
                let point = total(of: Drawing.point.new(Point.at(x: 20, y: 30)))
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
                    Figure.circle.new(let radius) => {{
                        return radius
                    }}
                    Shape.empty.new(_) => {{
                        return 0
                    }}
                    Figure.flag.new(_) => {{
                        return 1
                    }}
                }}
            }}

            fn main() -> Int {{
                alias Total = Size
                let total: Total = size(of: Figure.circle.new(40)) + size(of: Shape.flag.new(true))
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
                let by = 7
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
                "let x: IntU8 = 0\nlet by = -128\nlet y = x .> by",
            ),
            (
                "shr_wide",
                "let x: IntU8 = 0\nlet by = 4294967296\nlet y = x .> by",
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
        assert_eq!(run_executable("exit_in_unit", src).code(), Some(1));

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
    fn require_runs_its_else_only_when_its_condition_is_false() {
        let src = "
            fn halve(_ n: Int) -> Int {
                require n > 0 else {
                    return 0
                }
                return n / 2
            }

            fn main() -> Int {
                return halve(8) * 10 + halve(-3)
            }
            ";
        assert_eq!(run(src), 40);

        let src = "
            fn main() -> Int {
                let flag = false
                require flag else {
                    exit()
                }
                return 7
            }
            ";
        assert_eq!(run_executable("require_exits", src).code(), Some(1));
    }

    #[test]
    fn if_with_a_pattern_runs_its_first_block_when_it_matches() {
        let src = "
            fn describe(_ value: Option<Int>, and other: Int) -> Int {
                if value => .some.new(let n) {
                    return n
                } else if other => 0 {
                    return 1
                }
                return 2
            }

            fn main() -> Int {
                let a = describe(Option.some.new(5), and: 0)
                let b = describe(Option.none.new(()), and: 0)
                let c = describe(Option.none.new(()), and: 9)
                return a * 100 + b * 10 + c
            }
            ";
        assert_eq!(run(src), 512);
    }

    #[test]
    fn require_with_a_pattern_binds_its_values_after_it() {
        let src = "
            fn sum(_ first: Option<Int>, _ second: Option<Int>) -> Int {
                require (first, second) => (.some.new(let a), .some.new(let b)) else {
                    return 0
                }
                return a + b
            }

            fn main() -> Int {
                let a = sum(Option.some.new(3), Option.some.new(4))
                let b = sum(Option.some.new(3), Option.none.new(()))
                return a * 10 + b
            }
            ";
        assert_eq!(run(src), 70);

        let src = "
            fn main() -> Int {
                let user: Option<Int> = Option.none.new(())
                require user => .some.new(let u) else {
                    exit()
                }
                return u
            }
            ";
        assert_eq!(run_executable("require_pattern_exits", src).code(), Some(1));
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
                count: Int
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
                bits: Int
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
                bits: Int
            }

            impl Flags with Not {
                alias Output = Self

                fn not() -> Self {
                    return Self { bits: .!self.bits }
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
                x: Int
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
                x: Int
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
    fn arithmetic_and_ordering_operators_call_their_traits() {
        let src = "
            group Money {
                cents: Int
            }

            impl Money {
                static fn of(cents: Int) -> Self {
                    return Self { cents: cents }
                }
            }

            impl Money with Add {
                fn add(_ rhs: Self) -> Self {
                    return Money.of(cents: self.cents + rhs.cents)
                }
            }

            impl Money with Compare {
                fn compare(to rhs: Self) -> Comparison {
                    return self.cents.compare(to: rhs.cents)
                }
            }

            fn main() -> Int {
                let total = Money.of(cents: 30) + Money.of(cents: 12)
                if total > Money.of(cents: 41) {
                    return total.cents
                }
                return 0
            }
            ";
        assert_eq!(run(src), 42);
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
    fn changed_copies_of_groups_only_write_what_changed() {
        let src = "
            group User {
                age: Int
                isAdmin: Bool
            }

            impl User {
                fn revokeAdmin() -> User {
                    return Self { age: self.age, isAdmin: false }
                }
            }

            fn main() -> Int {
                return 0
            }
            ";
        let context = Context::create();
        let machine = machine();
        let module = compile_files(&context, &machine, &[("/main", src)]);

        // Optimising would fold away any `User` that Atom keeps, so this adds
        // a function that does `user = user.revokeAdmin()` to a `User` that it
        // only has a pointer to.
        let user = module
            .get_struct_type("/main::User")
            .expect("should declare the group");
        let revoke_admin = module
            .get_function("/main::User.revokeAdmin")
            .expect("should compile the method");
        let pointer = context.ptr_type(AddressSpace::default());
        let fn_type = context.void_type().fn_type(&[pointer.into()], false);
        let revoke = module.add_function("revoke", fn_type, None);
        let build = || -> Result<(), BuilderError> {
            let builder = context.create_builder();
            builder.position_at_end(context.append_basic_block(revoke, "entry"));
            let slot = revoke
                .get_nth_param(0)
                .expect("should take a pointer")
                .into_pointer_value();
            let value = builder.build_load(user, slot, "")?;
            let value = builder
                .build_call(revoke_admin, &[value.into()], "")?
                .try_as_basic_value()
                .basic()
                .expect("should return a `User`");
            builder.build_store(slot, value)?;
            builder.build_return(None)?;
            Ok(())
        };
        build().expect("should build the function");
        optimize(&module, &machine).expect("should optimise");

        // The method is inlined, and the `User` that's stored is the one that
        // was loaded with only `isAdmin` changed, so `age` is never read.
        let body = revoke.print_to_string().to_string();
        assert!(!body.contains("call"), "{body}");
        assert!(!body.contains("extractvalue"), "{body}");
        assert_eq!(body.matches("insertvalue").count(), 1, "{body}");
        assert!(body.contains(", i1 false, 1"), "{body}");

        // So the machine code only stores `false` into `isAdmin`'s byte.
        let assembly = machine
            .write_to_memory_buffer(&module, FileType::Assembly)
            .expect("should write assembly");
        let assembly = String::from_utf8_lossy(assembly.as_slice());
        let instructions: Vec<_> = assembly
            .lines()
            .skip_while(|line| !matches!(*line, "revoke:" | "_revoke:"))
            .skip(1)
            .take_while(|line| !line.contains(".cfi_endproc") && !line.contains("End function"))
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(['.', ';', '#']))
            .collect();
        #[cfg(target_arch = "aarch64")]
        assert_eq!(instructions, ["strb\twzr, [x0, #8]", "ret"], "{assembly}");
        #[cfg(target_arch = "x86_64")]
        assert_eq!(instructions, ["movb\t$0, 8(%rdi)", "retq"], "{assembly}");
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        assert!(!instructions.is_empty(), "{assembly}");
    }

    #[test]
    fn tuples_are_built_and_read() {
        let src = "
            group Line {
                start: (x: Int, y: Int)
                end: (x: Int, y: Int)
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
                empty: Unit
            }

            choice Shape {
                circle: Int
                pair: (left: Inner, right: Int)
                empty: Unit
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
                    Shape.circle.new(0) => {
                        return 100
                    }
                    Shape.circle.new(let radius) => {
                        return radius
                    }
                    Shape.pair.new((left: Inner.circle.new(let r), right: let n)) => {
                        return r * n
                    }
                    Shape.pair.new((left: _, right: _)) => {
                        return 7
                    }
                    Shape.empty.new(()) => {
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
                let c = size(of: Shape.circle.new(0)) + size(of: Shape.circle.new(9))
                let d = size(of: Shape.pair.new((left: Inner.circle.new(6), right: 5)))
                let e = size(of: Shape.pair.new((left: Inner.empty.new(()), right: 5))) + size(of: Shape.empty.new(()))
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
                none: Unit
                some: T
            }

            group Box<T> {
                value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }

                fn get() -> Self.T {
                    return self.value
                }

                fn maybe() -> Optional<Self.T> {
                    return Optional.some.new(self.value)
                }
            }

            group Pair<A, B> {
                first: A
                second: B
            }

            impl Pair {
                static fn of(_ first: Self.A, _ second: Self.B) -> Self {
                    return Self { first: first, second: second }
                }
            }

            fn unwrap(_ value: Optional<Int>, or fallback: Int) -> Int {
                match value {
                    Optional.some.new(let found) => {
                        return found
                    }
                    Optional.none.new(_) => {
                        return fallback
                    }
                }
            }

            fn main() -> Int {
                let a: Optional<Int> = Optional.some.new(40)
                let b = Optional<Int>.none.new(())
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
                base: Int
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
                by: Int
            }

            impl Scale with Call {
                fn call(input: (_ value: Int)) -> Int {
                    return input.value * self.by
                }
            }

            group Tools {
                adder: Adder
                scale: Scale
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
    fn functions_can_be_used_as_values() {
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

            fn ten() -> Int {
                return 10
            }

            fn main() -> Int {
                let a = add
                let b = negate
                let c = scale
                let d = same
                let e = sum
                let f = ten
                return a(a: 1, b: 2) + b(3) + c(4, by: 5) + d(to: 6) + e((a: 7, b: 8)) + f()
            }
            ";
        // 3 - 3 + 20 + 6 + 15 + 10
        assert_eq!(run(src), 51);
    }

    #[test]
    fn calling_a_function_is_the_same_as_using_call() {
        let src = "
            fn add(a: Int, b: Int) -> Int {
                return a + b
            }

            fn negate(_ value: Int) -> Int {
                return -value
            }

            fn main() -> Int {
                let pair = (a: 30, b: 12)
                let sums = add(a: 100, b: 1) + add.call(input: (a: 200, b: 2)) + add.call(input: pair)
                return sums + negate(1) + negate.call(input: (2))
            }
            ";
        // 101 + 202 + 42 - 1 - 2
        assert_eq!(run(src), 342);
    }

    #[test]
    fn raw_pointers_offset_by_bytes() {
        // Each check is one bit of the result.
        let src = "
            fn bit(_ value: Bool, at place: Int) -> Int {
                if value {
                    return place
                }
                return 0
            }

            fn inc(_ value: Int) -> Int {
                return value + 1
            }

            fn main() -> Int {
                let start = inc.pointer
                let ahead = start.offset(by: 8)
                let behind = start.offset(by: -8)
                let a = bit(ahead.address - start.address == 8, at: 1)
                let b = bit(start.address - behind.address == 8, at: 2)
                let c = bit(start.offset(by: 0).address == start.address, at: 4)
                let d = bit(ahead.offset(by: -8).address == start.address, at: 8)
                let e = bit(start.offset(by: -1).address + 1 == start.address, at: 16)
                return a + b + c + d + e
            }
            ";
        assert_eq!(run(src), 31);
    }

    #[test]
    fn raw_pointers_offset_past_either_end_of_memory_trap() {
        let src = "
            fn inc(_ value: Int) -> Int {
                return value + 1
            }

            fn main() -> Int {
                let start = inc.pointer
                let toZero = IntS.new(start.address).unwrap()
                let zero = start.offset(by: -toZero)
                let below = zero.offset(by: -1)
                return 0
            }
            ";
        assert!(traps("offset_below_zero", src));

        // Twice the largest `IntS` on a 64-bit computer is past the end of
        // memory from any address but 0 and 1.
        let src = "
            fn inc(_ value: Int) -> Int {
                return value + 1
            }

            fn main() -> Int {
                let largest: IntS = 9223372036854775807
                let start = inc.pointer
                let past = start.offset(by: largest).offset(by: largest)
                return 0
            }
            ";
        assert!(traps("offset_past_the_end", src));

        // Going to 0 itself is fine.
        let src = "
            fn inc(_ value: Int) -> Int {
                return value + 1
            }

            fn main() -> Int {
                let start = inc.pointer
                let toZero = IntS.new(start.address).unwrap()
                if start.offset(by: -toZero).address == 0 {
                    return 3
                }
                return 0
            }
            ";
        assert_eq!(run(src), 3);
    }

    #[test]
    fn fn_values_hold_the_address_of_their_function() {
        let src = "
            fn inc(_ value: Int) -> Int {
                return value + 1
            }

            fn dec(_ value: Int) -> Int {
                return value - 1
            }

            let incAddress = inc.pointer.address

            fn main() -> Int {
                let same = inc.pointer.address == inc.pointer.address
                let different = inc.pointer.address != dec.pointer.address
                if same & different {
                    return 1
                }
                return 0
            }
            ";
        let context = Context::create();
        let machine = machine();
        let module = compile_files(&context, &machine, &[("/main", src)]);

        // Nothing in Atom can turn an `IntU` into an `Int` yet, so this adds a
        // function that reads `incAddress`.
        let global = module
            .get_global("/main::incAddress")
            .expect("should declare the value")
            .as_pointer_value();
        let int = context.ptr_sized_int_type(&machine.get_target_data(), None);
        let reader = module.add_function("read_address", int.fn_type(&[], false), None);
        let build = || -> Result<(), BuilderError> {
            let builder = context.create_builder();
            builder.position_at_end(context.append_basic_block(reader, "entry"));
            let address = builder.build_load(int, global, "")?;
            builder.build_return(Some(&address))?;
            Ok(())
        };
        build().expect("should build the reader");

        let engine = module
            .create_jit_execution_engine(OptimizationLevel::None)
            .expect("the JIT should start");
        unsafe {
            assert_eq!(call::<i32>(&engine, "main"), 1);
            let inc = engine
                .get_function_address("/main::inc")
                .expect("should compile `inc`");
            assert_eq!(call::<usize>(&engine, "read_address"), inc);
        }
    }

    #[test]
    fn fn_values_can_be_passed_stored_and_returned() {
        let src = "
            choice Maybe {
                one: Fn<(_: Int), Int>
                none: Unit
            }

            group Tools {
                double: Fn<(_: Int), Int>
            }

            impl Tools {
                static fn new() -> Self {
                    fn double(_ value: Int) -> Int {
                        return value * 2
                    }

                    return Self { double: double }
                }
            }

            fn inc(_ value: Int) -> Int {
                return value + 1
            }

            fn applying(_ function: Fn<(_: Int), Int>, to value: Int) -> Int {
                return function(value)
            }

            fn chosen() -> Fn<(_: Int), Int> {
                return inc
            }

            fn main() -> Int {
                var function = chosen()
                let a = function(1)
                function = Tools.new().double
                let b = function(10)
                let c = applying(Tools.new().double, to: 100)
                let d = Tools.new().double(1000)
                match Maybe.one.new(inc) {
                    Maybe.one.new(let found) => {
                        return a + b + c + d + found(10000)
                    }
                    Maybe.none.new(_) => {
                        return 0
                    }
                }
            }
            ";
        // 2 + 20 + 200 + 2000 + 10001
        assert_eq!(run(src), 12223);
    }

    #[test]
    fn unnamed_parameters_and_values_keep_their_places() {
        let src = "
            choice Shape {
                circle: Int
                rectangle: (width _: Int, height: Int)
            }

            fn last(_: Int, to _: Int, _ value: Int) -> Int {
                return value
            }

            fn size(of shape: Shape) -> Int {
                match shape {
                    Shape.circle.new(let radius) => {
                        return radius
                    }
                    Shape.rectangle.new((width: let width, height: let height)) => {
                        return width * height
                    }
                }
            }

            fn main() -> Int {
                let point: (at x: Int, _ y: Int) = (at: 1, 2)
                let same: (at: Int, _: Int) = point
                let read: (at x: Int, _ y: Int) = same
                let circle = Shape.circle
                let shapes = size(of: circle.new(3)) + size(of: Shape.rectangle.new((width: 4, height: 5)))
                return last(100, to: 200, 7) * 1000 + shapes * 10 + read.x + read.y
            }
            ";
        assert_eq!(run(src), 7233);
    }

    #[test]
    fn generic_types_are_read_by_the_names_they_were_found_with() {
        let src = "
            group Box<T> {
                value: T
            }

            impl Box {
                static fn new(_ value: Self.T) -> Self {
                    return Self { value: value }
                }

                fn get() -> Self.T {
                    return self.value
                }
            }

            fn main() -> Int {
                let plain: Box<(at: Int)> = Box.new((at: 1))
                let a: Box<(at x: Int)> = Box.new((at: 3))
                let b = a
                let c: Box<(at: Int)> = a
                return plain.get().at * 100 + b.get().x * 10 + c.value.at
            }
            ";
        assert_eq!(run(src), 133);
    }

    #[test]
    fn function_values_are_called_with_their_input_unpacked() {
        let src = "
            fn subtract(_ a: Int, from b: Int) -> Int {
                return b - a
            }

            fn none() -> Int {
                return 4
            }

            fn main() -> Int {
                let minus = subtract
                let four = none
                return minus(2, from: 10) * 10 + minus.call(input: (1, from: 3)) + four()
            }
            ";
        assert_eq!(run(src), 86);
    }

    #[test]
    fn functions_can_take_a_tuple_as_a_value() {
        let src = "
            fn sum(_ pair: (a: Int, b: Int)) -> Int {
                return pair.a + pair.b
            }

            fn main() -> Int {
                let pair = (a: 1, b: 2)
                let value = sum
                // 3 + 7 + 3 + 3
                return sum(pair) + sum((a: 3, b: 4)) + value(pair) + value.call(input: (pair))
            }
            ";
        assert_eq!(run(src), 16);
    }

    #[test]
    fn static_functions_can_be_used_as_values() {
        let src = "
            group Point {
                x: Int
                y: Int
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
                value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }
            }

            fn built(by make: Fn<(_: Int), Box<Int>>, from value: Int) -> Int {
                return make(value).value
            }

            fn main() -> Int {
                let at = Point.at
                let origin = Point.origin
                let pair = (x: 10, y: 20)
                let a = at(x: 1, y: 2)
                let b = origin()
                let c = Point.at.call(input: pair)
                // 1 + 2 + 0 + 0 + 10 + 20 + 100
                return a.x + a.y + b.x + b.y + c.x + c.y + built(by: Box<Int>.of, from: 100)
            }
            ";
        assert_eq!(run(src), 133);
    }

    #[test]
    fn functions_can_recurse_through_fn_values() {
        let src = "
            fn sum(to value: Int) -> Int {
                if value < 1 {
                    return 0
                }
                let again = sum
                return value + again(to: value - 1)
            }

            fn main() -> Int {
                return sum(to: 10)
            }
            ";
        assert_eq!(run(src), 55);
    }

    #[test]
    fn fn_can_use_generic_parameters() {
        let src = "
            group Box<T> {
                value: T
            }

            impl Box {
                static fn of(_ value: Self.T) -> Self {
                    return Self { value: value }
                }

                fn mapped(with function: Fn<(_: Self.T), Self.T>) -> Self {
                    return Self { value: function(self.value) }
                }
            }

            fn doubled(_ value: Int) -> Int {
                return value * 2
            }

            fn inverted(_ value: Bool) -> Bool {
                return !value
            }

            fn main() -> Int {
                let flag = Box<Bool>.of(false).mapped(with: inverted)
                if !flag.value {
                    return 0
                }
                return Box<Int>.of(21).mapped(with: doubled).value
            }
            ";
        assert_eq!(run(src), 42);
    }

    #[test]
    fn top_level_values_can_hold_functions() {
        let src = "
            let divider = divide
            let result = divider(10, by: 2)

            fn divide(_ value: Int, by divisor: Int) -> Int {
                return value / divisor
            }

            fn main() -> Int {
                return 0
            }
            ";
        assert_eq!(run(src), 0);
        // The call is really made, since dividing by zero stops the program.
        assert!(traps("fn_global_trap", &src.replace("by: 2", "by: 0")));
    }

    #[test]
    fn exit_stops_the_program_as_a_value() {
        let src = "
            fn main() -> Int {
                let stop = exit
                stop()
            }
            ";
        assert_eq!(run_executable("exit_as_value", src).code(), Some(1));
    }

    #[test]
    fn trait_functions_are_called_like_any_other() {
        let src = "
            group Counter {
                count: Int
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
    fn pointer_sized_integers_are_as_wide_as_pointers() {
        let bits = pointer_width(&machine()).bits();
        let src = "
            fn f(_ a: IntU, _ b: IntS) -> Int {
                return b
            }

            fn main() -> Int {
                return 0
            }
            ";
        let context = Context::create();
        let module = compile_files(&context, &machine(), &[("/main", src)]);
        let f = module
            .get_function("/main::f")
            .expect("the function should be compiled")
            .print_to_string()
            .to_string();
        let int = format!("i{bits}");
        assert!(
            f.contains(&format!("{int} @\"/main::f\"({int} %0, {int} %1)")),
            "{f}"
        );

        // So they overflow at that width.
        let max = u64::MAX >> (64 - bits);
        let adding = |value: u64| {
            format!("fn main() -> Int {{\nlet x: IntU = {value}\nlet y = x + 1\nreturn 0\n}}")
        };
        assert!(traps("intu_max", &adding(max)));
        assert!(!traps("intu_below_max", &adding(max - 1)));
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
