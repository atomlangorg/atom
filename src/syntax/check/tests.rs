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

use super::*;
use crate::syntax::parser::parse;

/// The standard library type called `name`.
fn ty(name: &str) -> Type {
    let mut checker = Checker::default();
    checker.add_stdlib();
    checker.check_files();
    checker.std_type(name)
}

/// Parses each `(path, src)` pair as a file, with paths like
/// `/dir/other`.
fn parsed_files(files: &[(&str, &str)]) -> Vec<File> {
    files
        .iter()
        .map(|(path, src)| File {
            path: path[1..].split('/').map(str::to_string).collect(),
            program: parse(src).expect("source should parse"),
        })
        .collect()
}

/// Checks each `(path, src)` pair as a file, with paths like
/// `/dir/other`.
fn check_files(files: &[(&str, &str)]) -> Result<(), Vec<FileError>> {
    let result = check(&parsed_files(files)).map(|_| ());
    // Every error must point at something in the file it's reported in.
    for err in result.as_ref().err().into_iter().flatten() {
        let found = pointed_at(files, err);
        assert!(!found.is_empty(), "`{err}` points at nothing: {err:?}");
    }
    result
}

/// The source that `err`, from checking `files`, points at.
fn pointed_at<'a>(files: &[(&'a str, &'a str)], err: &FileError) -> &'a str {
    let src = files
        .iter()
        .find(|(path, _)| *path == err.file)
        .map(|(_, src)| *src)
        .or_else(|| stdlib_source(&err.file))
        .unwrap_or_else(|| panic!("`{}` should be a file", err.file));
    src.get(err.span.start..err.span.end)
        .unwrap_or_else(|| panic!("`{err}` points outside its file: {err:?}"))
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

/// Checks `src` as the only file, `/main`.
fn check_top(src: &str) -> Result<(), Vec<CheckError>> {
    check_files(&[("/main", src)]).map_err(|errs| errs.into_iter().map(|err| err.error).collect())
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

/// The error for the binary operator `op` used on a `Bool`, which doesn't
/// implement the trait it calls.
fn not_implemented(op: &'static str) -> CheckError {
    let (trait_name, _) = BinOp::ALL
        .into_iter()
        .find(|found| found.symbol() == op)
        .expect("`op` is a binary operator")
        .trait_function();
    CheckError::NotImplemented {
        op,
        trait_name,
        found: ty("Bool"),
    }
}

/// The error for `-` used on a `Bool`, which can't be negated.
fn not_negatable() -> CheckError {
    CheckError::NotImplemented {
        op: "-",
        trait_name: "Negate",
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
                return Self { bits: .!self.bits }
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
                return !()
            }
            "
        ),
        [
            "operator `!` expects a type that implements `Not`, found `Flags`",
            "operator `!` expects a type that implements `Not`, found `()`",
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
fn boolean_operators_do_not_need_their_implementation_imported() {
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
        check_files(&[
            ("/flags", flags),
            ("/and", and),
            (
                "/main",
                "use /flags\nfn f(a: Flags) -> Bool {\nreturn a & a\n}"
            ),
        ]),
        Ok(())
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
        Err(vec![not_implemented("+")])
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
    assert_eq!(check_body("let x = -!true"), Err(vec![not_negatable()]));
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
        Err(vec![not_implemented("+")])
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
        check_body("let x = () == ()"),
        Err(vec![incomparable("==")])
    );
    assert_eq!(check_body("let x = () != 1"), Err(vec![incomparable("!=")]));
    assert_eq!(
        messages("let x = () == ()"),
        vec!["operator `==` expects a type that implements `Equal`, found `()`"]
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
        Err(vec![not_implemented("<")])
    );
    assert_eq!(
        check_body("let x = 1 <= true"),
        Err(vec![invalid_operand("<=")])
    );
    assert_eq!(
        check_body("let x = () > ()"),
        Err(vec![CheckError::NotImplemented {
            op: ">",
            trait_name: "Order",
            found: ty("Void"),
        }])
    );
    assert_eq!(
        messages("let x = true >= false"),
        vec!["operator `>=` expects a type that implements `Order`, found `Bool`"]
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
            let e: IntU8 = 1 .< amount
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
        Err(vec![not_implemented(".&")])
    );
    assert_eq!(
        check_body("let x = 1 .| false"),
        Err(vec![invalid_operand(".|")])
    );
    assert_eq!(
        check_body("let x = true .^ true"),
        Err(vec![not_implemented(".^")])
    );
    assert_eq!(
        messages("let x = .!false"),
        vec!["operator `.!` expects a type that implements `BitNot`, found `Bool`"]
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
                let f = byte .< byte
            }
            "
        ),
        [
            "operator `.<` expects a type that implements `ShiftLeft`, found `IntS8`",
            "operator `.>` expects a type that implements `ShiftRight`, found `Int`",
            "operator `.<` expects a type that implements `ShiftLeft`, found `Bool`",
            "operator `.>` expects `Int`, found `Bool`",
            "operator `.<` expects a type that implements `ShiftLeft`, found `IntS8`",
            "operator `.<` expects `Int`, found `IntU8`",
        ]
    );
}

#[test]
fn shifts_have_the_type_of_what_is_shifted() {
    assert_eq!(
        messages_top(
            "
            fn f(byte: IntU8, amount: Int) {
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
        check_body("let x = () == y"),
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
    assert_eq!(
        check_body("let x = true + 1"),
        Err(vec![not_implemented("+")])
    );
    assert_eq!(
        check_body("let x = 1 - false"),
        Err(vec![invalid_operand("-")])
    );
    assert_eq!(check_body("let x = -true"), Err(vec![not_negatable()]));
    assert_eq!(
        check_body("let x = 4 / (true)"),
        Err(vec![invalid_operand("/")])
    );
    assert_eq!(
        check_body("let b = true\nlet x = b * b"),
        Err(vec![not_implemented("*")])
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
        Err(vec![not_negatable()])
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
        ["operator `+` expects a type that implements `Add`, found `Bool`"]
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
    // `/main` and `stdlib/int` are different files, so both can declare
    // `Int`, and in `/main` a bare `Int` means its own.
    assert_eq!(
        messages_top(
            "
            group Int {}
            let a: Int = 1
            let b: int::Int = 1
            fn f(x: main::Int) -> bool::Bool {
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
fn errors_are_in_the_order_they_are_written() {
    // Types are declared before function bodies are checked, but their
    // errors still come after the errors written before them.
    let src = "
        fn f() {
            let x: Missing = y
        }
        group Twice {}
        group Twice {}
        ";
    assert_eq!(
        messages_top(src),
        [
            "unknown type `Missing`",
            "`y` is not declared",
            "`Twice` is already declared",
        ]
    );
}

#[test]
fn reports_every_error() {
    assert_eq!(
        check_body("let x = 1\nx = 2\ny = z"),
        Err(vec![
            CheckError::AssignToConstant("x".to_string()),
            CheckError::Undeclared("y".to_string()),
            CheckError::Undeclared("z".to_string()),
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
            "functions can't use `total`, because it is declared outside the function",
            "functions can't use `step`, because it is declared outside the function",
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
            CheckError::WrongArgumentCount {
                function: "f".to_string(),
                expected: 0,
                found: 1,
            },
            CheckError::Undeclared("x".to_string()),
            CheckError::NotCallable {
                name: "y".to_string(),
                ty: ty("Int"),
            },
            CheckError::Undeclared("z".to_string()),
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
            "operator `+` expects a type that implements `Add`, found `Bool`",
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
            yes()
            no()
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
                Answer.yes() => {}
                Answer.no() => {}
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
            fn nothing() {}
            fn h() -> Int {
                nothing()
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
            "operator `+` expects a type that implements `Add`, found `User`",
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
    check(&parsed_files(&[(path, src)]))
        .expect("source should check")
        .main
        .is_some()
}

#[test]
fn main_must_return_int_from_the_main_file() {
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
        ["/dir/a: cannot find file `./dir/b`"]
    );
}

#[test]
fn parent_paths_start_above_the_current_directory() {
    assert_eq!(
        check_files(&[
            (
                "/a/b/main",
                "
                use ../sibling
                use ../../root
                use ../other/nested
                let x: Int = sibling::s + root::r + nested::n
                ",
            ),
            ("/a/sibling", "let s = 1"),
            ("/root", "let r = 2"),
            ("/a/other/nested", "let n = 3"),
        ]),
        Ok(())
    );
}

#[test]
fn parent_paths_can_have_aliases() {
    assert_eq!(
        check_files(&[
            (
                "/dir/main",
                "use ../lib as renamed\nuse ./lib\nlet a: Int = renamed::x\nlet b: Bool = lib::x"
            ),
            ("/lib", "let x = 1"),
            ("/dir/lib", "let x = true"),
        ]),
        Ok(())
    );
}

#[test]
fn parent_paths_can_import_modules() {
    let files = |main| {
        [
            ("/a/b/main", main),
            ("/a/b/own", "let o = 1"),
            ("/a/sibling", "let s = 2"),
            ("/a/other/nested", "let n = 3"),
            ("/outside", "let t = 4"),
        ]
    };
    // `../` is everything in `/a/`, including the current directory.
    assert_eq!(check_files(&files("use ../\nlet x = o + s + n")), Ok(()));
    assert_eq!(
        file_messages(&files("use ../\nlet x = t")),
        ["/a/b/main: `t` is not declared"]
    );
    // The root of the project is still in the project.
    assert_eq!(
        check_files(&files("use ../../\nlet x = o + s + n + t")),
        Ok(())
    );
    assert_eq!(check_files(&files("use ../other/\nlet x = n")), Ok(()));
}

#[test]
fn parent_paths_can_not_go_above_the_root() {
    // Even if what they name exists, so they aren't stopped at the root.
    for (file, main, path) in [
        ("/main", "use ../lib", "../lib"),
        ("/main", "use ../", "../"),
        ("/main", "use ../dir/", "../dir/"),
        ("/main", "use ../../lib as x", "../../lib"),
        ("/dir/main", "use ../../lib", "../../lib"),
        ("/dir/main", "use ../../", "../../"),
        (
            "/dir/main",
            "use ../../../../../../lib",
            "../../../../../../lib",
        ),
        ("/dir/main", "use ../../dir/lib", "../../dir/lib"),
    ] {
        let files = [(file, main), ("/lib", ""), ("/dir/lib", "")];
        assert_eq!(
            file_messages(&files),
            [format!("{file}: `{path}` goes above the root directory")]
        );
        let errs = check_files(&files).expect_err("the path goes above the root");
        assert_eq!(pointed_at(&files, &errs[0]), main);
    }
}

#[test]
fn parent_paths_can_not_reach_the_stdlib_from_the_project() {
    assert_eq!(
        file_messages(&[("/main", "use ../stdlib/int")]),
        ["/main: `../stdlib/int` goes above the root directory"]
    );
    assert_eq!(
        file_messages(&[("/dir/main", "use ../stdlib/int")]),
        ["/dir/main: cannot find file `../stdlib/int`"]
    );
}

#[test]
fn parent_paths_can_not_leave_the_stdlib() {
    assert_eq!(
        messages_in_stdlib(&["extra"], "use ../secret"),
        ["stdlib/extra: `../secret` goes above the root directory"]
    );
    // Going up inside it stays inside it.
    assert_eq!(
        messages_in_stdlib(&["sub", "extra"], "use ../secret"),
        ["stdlib/sub/extra: cannot find file `../secret`"]
    );
    assert!(messages_in_stdlib(&["sub", "extra"], "use ../int").is_empty());
}

#[test]
fn absolute_paths_in_the_stdlib_start_from_its_root() {
    assert!(messages_in_stdlib(&["sub", "extra"], "use /int").is_empty());
    assert!(messages_in_stdlib(&["sub", "extra"], "use /\nlet a: int::Int = 1").is_empty());
    // Which means they can't reach the project's files.
    assert_eq!(
        messages_in_stdlib(&["extra"], "use /secret"),
        ["stdlib/extra: cannot find file `/secret`"]
    );
    assert_eq!(
        messages_in_stdlib(&["extra"], "use /\nlet a = hidden"),
        ["stdlib/extra: `hidden` is not declared"]
    );
}

/// The errors from checking `src` as a file of the standard library at
/// `path`, in a project that has a file called `/secret`.
fn messages_in_stdlib(path: &[&str], src: &str) -> Vec<String> {
    let stdlib = File {
        path: path.iter().map(|s| s.to_string()).collect(),
        program: parse(src).expect("source should parse"),
    };
    let project = parsed_files(&[("/secret", "let hidden = 1")]);
    let mut checker = Checker::default();
    checker.add_stdlib();
    checker.add_file(Some(STDLIB_PACKAGE), &stdlib);
    checker.add_file(None, &project[0]);
    checker.check_files();
    checker.errors.iter().map(ToString::to_string).collect()
}

#[test]
fn parent_paths_get_the_same_advice_as_other_paths() {
    let files = |main| [("/dir/main", main), ("/lib", ""), ("/other/x", "")];
    assert_eq!(
        file_messages(&files("use ../other")),
        ["/dir/main: `../other` is a module, so use `../other/` to import every file in it"]
    );
    assert_eq!(
        file_messages(&files("use ../lib/")),
        ["/dir/main: `../lib` is a file, so use it without the trailing slash"]
    );
    assert_eq!(
        file_messages(&files("use ../missing")),
        ["/dir/main: cannot find file `../missing`"]
    );
    assert_eq!(
        file_messages(&files("use ../missing/")),
        ["/dir/main: cannot find module `../missing/`"]
    );
}

#[test]
fn rejects_unknown_files() {
    assert_eq!(
        file_messages(&[("/main", "use /missing\nuse ./missing\nuse other")]),
        [
            "/main: cannot find file `/missing`",
            "/main: cannot find file `./missing`",
            "/main: cannot find file `other`",
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
            "/main: `lib` is not an imported file",
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
                use stdlib/int
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
fn names_from_several_files_must_be_qualified() {
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
fn files_with_the_same_name_need_aliases() {
    // Only if the name is used.
    assert_eq!(
        check_files(&[
            ("/main", "use /a/other\nuse /b/other"),
            ("/a/other", "let value = 1"),
            ("/b/other", "let value = true"),
        ]),
        Ok(())
    );
    assert_eq!(
        file_messages(&[
            ("/main", "use /a/other\nuse /b/other\nlet x = other::value"),
            ("/a/other", "let value = 1"),
            ("/b/other", "let value = true"),
        ]),
        [
            "/main: `other` is the name of `/a/other` and `/b/other`, so use `as` to give them different names"
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
fn top_level_values_can_be_qualified_by_file_name_or_alias() {
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
fn names_from_files_with_the_same_name_only_clash_if_they_are_used() {
    let files = |main| {
        [
            ("/main", main),
            (
                "/a/other",
                "fn foo() -> Int {\nreturn 1\n}\nfn bar() -> Int {\nreturn 2\n}",
            ),
            ("/b/other", "fn foo() -> Bool {\nreturn true\n}"),
        ]
    };
    // `bar` only has one definition, so it doesn't need qualifying.
    assert_eq!(
        check_files(&files("use /a/other\nuse /b/other\nlet x: Int = bar()")),
        Ok(())
    );
    assert_eq!(
        file_messages(&files("use /a/other\nuse /b/other\nlet x = foo()")),
        ["/main: `foo` is declared in `/a/other` and `/b/other`, so it must be qualified"]
    );
    // A qualifier that could be either file is never allowed, so that
    // adding a name to a file can't break a file that doesn't use it.
    assert_eq!(
        file_messages(&files("use /a/other\nuse /b/other\nlet x = other::bar()")),
        [
            "/main: `other` is the name of `/a/other` and `/b/other`, so use `as` to give them different names"
        ]
    );
    assert_eq!(
        check_files(&files(
            "use /a/other as first\nuse /b/other\nlet x: Int = first::foo()\nlet y: Bool = other::foo()"
        )),
        Ok(())
    );
}

#[test]
fn aliases_replace_the_names_of_files() {
    assert_eq!(
        file_messages(&[
            (
                "/main",
                "use /lib as renamed\nlet x = renamed::y\nlet z = lib::y"
            ),
            ("/lib", "let y = 1"),
        ]),
        ["/main: `lib` is not an imported file"]
    );
}

#[test]
fn aliased_files_are_still_imported() {
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
                        return Self { age: 1 }
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
fn aliases_are_only_visible_in_their_own_file() {
    assert_eq!(
        file_messages(&[
            ("/main", "use /a\nlet x = renamed::y"),
            ("/a", "use /lib as renamed"),
            ("/lib", "let y = 1"),
        ]),
        ["/main: `renamed` is not an imported file"]
    );
}

#[test]
fn aliases_replace_a_file_s_name_wherever_it_s_imported() {
    for main in [
        "use /lib\nuse /lib as x\nlet a = x::z\nlet b = lib::z",
        "use /lib as x\nuse /lib\nlet a = x::z\nlet b = lib::z",
    ] {
        assert_eq!(
            file_messages(&[("/main", main), ("/lib", "let z = 1")]),
            ["/main: `lib` is not an imported file"]
        );
    }
}

#[test]
fn files_can_only_have_one_alias() {
    assert_eq!(
        file_messages(&[
            ("/main", "use /lib as x\nuse /lib as y"),
            ("/lib", "let z = 1"),
        ]),
        ["/main: `/lib` is already imported as `x`"]
    );
    // Giving it the same one again is harmless.
    assert_eq!(
        check_files(&[
            ("/main", "use /lib as x\nuse ./lib as x\nlet a = x::z"),
            ("/lib", "let z = 1"),
        ]),
        Ok(())
    );
}

#[test]
fn files_can_alias_themselves() {
    assert_eq!(
        check_files(&[("/main", "use /main as me\nlet a = 1\nlet b = me::a")]),
        Ok(())
    );
    assert_eq!(
        file_messages(&[("/main", "use /main as me\nlet a = 1\nlet b = main::a")]),
        ["/main: `main` is not an imported file"]
    );
}

#[test]
fn rejects_the_same_alias_for_two_files() {
    // Even if it's never used, and only once if it is.
    for main in [
        "use /a as x\nuse /b as x",
        "use /a as x\nuse /b as x\nlet y = x::value\nlet z = x::missing",
    ] {
        assert_eq!(
            file_messages(&[
                ("/main", main),
                ("/a", "let value = 1"),
                ("/b", "let value = 2"),
            ]),
            ["/main: `x` is the name of `/a` and `/b`, so use `as` to give them different names"]
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
fn rejects_aliases_that_match_another_file_s_name() {
    for (main, names) in [
        ("use /a as b\nuse /b", "`/a` and `/b`"),
        ("use /b\nuse /a as b", "`/b` and `/a`"),
    ] {
        let files = [("/main", main), ("/a", ""), ("/b", "")];
        assert_eq!(
            file_messages(&files),
            [format!(
                "/main: `b` is the name of {names}, so use `as` to give them different names"
            )]
        );
        let errs = check_files(&files).expect_err("the alias should be rejected");
        assert_eq!(pointed_at(&files, &errs[0]), "use /a as b");
    }
    // Unless that file has an alias of its own.
    assert_eq!(
        check_files(&[
            (
                "/main",
                "use /a as b\nuse /b as c\nlet x: Int = b::y\nlet z: Bool = c::y"
            ),
            ("/a", "let y = 1"),
            ("/b", "let y = true"),
        ]),
        Ok(())
    );
}

#[test]
fn imports_can_share_their_own_file_s_name_unless_it_s_used() {
    assert_eq!(
        check_files(&[
            ("/main", "use /dir/main\nlet a = b"),
            ("/dir/main", "let b = 1")
        ]),
        Ok(())
    );
    assert_eq!(
        file_messages(&[
            ("/main", "use /dir/main\nlet a = main::b"),
            ("/dir/main", "let b = 1")
        ]),
        [
            "/main: `main` is the name of `/main` and `/dir/main`, so use `as` to give them different names"
        ]
    );
    assert_eq!(
        file_messages(&[("/main", "use /dir as main"), ("/dir", "")]),
        [
            "/main: `main` is the name of `/main` and `/dir`, so use `as` to give them different names"
        ]
    );
    assert_eq!(
        check_files(&[
            (
                "/main",
                "use /dir/main as other\nlet a = other::b\nlet c = main::a"
            ),
            ("/dir/main", "let b = 1"),
        ]),
        Ok(())
    );
}

#[test]
fn module_imports_can_import_files_with_the_same_name() {
    let files = |main| {
        [
            ("/main", main),
            ("/a/b/x", "let p = 1"),
            ("/a/x", "let q = 2"),
        ]
    };
    assert_eq!(check_files(&files("use /a/")), Ok(()));
    assert_eq!(check_files(&files("use /a/\nlet z = p + q")), Ok(()));
    assert_eq!(
        file_messages(&files("use /a/\nlet z = x::p")),
        ["/main: `x` is the name of `/a/b/x` and `/a/x`, so use `as` to give them different names"]
    );
}

#[test]
fn aliases_fix_module_imports_of_files_with_the_same_name() {
    for main in [
        "use ./\nuse ./a/other as first\nlet z = first::x + other::y + x + y",
        "use ./a/other as first\nuse ./\nlet z = first::x + other::y + x + y",
    ] {
        assert_eq!(
            check_files(&[
                ("/main", main),
                ("/a/other", "let x = 1"),
                ("/b/other", "let y = 2"),
            ]),
            Ok(())
        );
    }
}

#[test]
fn unknown_files_can_not_be_aliased() {
    assert_eq!(
        file_messages(&[("/main", "use /missing as x\nlet y = x::z")]),
        [
            "/main: cannot find file `/missing`",
            "/main: `x` is not an imported file",
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
        ["/main: `renamed` is a file, not a value, so use `renamed::y` to refer to a name in it"]
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
        check_top("let x: int::Int = 1\nlet y: Int = x\nlet z: bool::Bool = true"),
        Ok(())
    );
}

#[test]
fn files_can_import_each_other() {
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
fn groups_cannot_contain_themselves_across_files() {
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
        ["operator `+` expects a type that implements `Add`, found `Bool`"]
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
fn rejects_redeclaration_within_a_file() {
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
                yes()
                no()
            }
            fn f(answer: Answer) {
                match answer {
                    Answer.yes() => {}
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
    assert_eq!(pointed_at_top(&in_impl("Self { x: 1, y: true }")), ["true"]);
    assert_eq!(pointed_at_top(&in_impl("Self { x: 1, z: 2 }")), ["z: 2"]);
    assert_eq!(pointed_at_top(&in_impl("Self { x: 1 }")), ["Self { x: 1 }"]);
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
    assert!(errs.iter().all(|err| err.file == "/named"), "{errs:?}");
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
fn stdlib_sources_are_found_by_file() {
    assert!(stdlib_source("stdlib/int").is_some());
    assert_eq!(stdlib_source("stdlib/missing"), None);
    assert_eq!(stdlib_source("/int"), None);
}

#[test]
fn errors_name_their_file() {
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
            return Self { age: age, isAdmin: isAdmin }
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
                    return Self {{ lead: User.new(age: 30, isAdmin: false), size: 1 + 2 }}
                }}
                fn withLead(lead: User) -> Team {{
                    return Self {{ lead: lead, size: 1 }}
                }}
            }}
            impl Empty {{
                static fn new() -> Empty {{
                    let empty = Self {{}}
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
                    let x: Int = Self {{ age: 1, isAdmin: true }}
                    let y = Self {{ age: 1, isAdmin: true }} + 1
                }}
            }}
            "
        )),
        [
            "cannot assign `User` to `x`, which has type `Int`",
            "operator `+` expects a type that implements `Add`, found `User`",
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
                    let a = Self {{ age: 1 }}
                    let b = Self {{ age: 1, isAdmin: true, extra: 2 }}
                    let c = Self {{}}
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
                    let a = Self {{ isAdmin: true, age: 1 }}
                    let b = Self {{ age: 1, admin: true }}
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
                    let a = Self {{ age: true, isAdmin: 1 }}
                    let b = Self {{ age: -true, isAdmin: false }}
                }}
            }}
            "
        )),
        [
            "field `age` of `User` expects `Int`, found `Bool`",
            "field `isAdmin` of `User` expects `Bool`, found `Int`",
            "operator `-` expects a type that implements `Negate`, found `Bool`",
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
                    return Self { value: 1 }
                }
            }
            "
        ),
        ["unknown type `Missing`"]
    );
}

#[test]
fn groups_are_only_constructed_in_their_own_file() {
    // Even in an `impl` of the type, another file can't construct it,
    // but it can use the type's own functions.
    assert_eq!(
        file_messages(&[
            (
                "/main",
                "
                use ./users/user
                impl User {
                    static fn admin(age: Int) -> User {
                        return Self { age: age, isAdmin: true }
                    }
                    static fn guest() -> User {
                        return User.new(age: 3, isAdmin: false)
                    }
                }
                let b: User = User.guest()
                ",
            ),
            ("/users/user", USER),
        ]),
        ["/main: `User` can only be constructed in `/users/user`, which declares it"]
    );
}

#[test]
fn only_literals_create_ints_and_bools() {
    assert_eq!(
        check_body("let v: Void = ()\nlet w: void::Void = ()"),
        Ok(())
    );
    assert_eq!(
        messages_top(
            "
            impl Int {
                static fn made() -> Int {
                    return Self {}
                }
            }
            impl Bool {
                static fn made() -> Bool {
                    return Self { value: c }
                }
            }
            fn f() {
                let d = 1.value
            }
            "
        ),
        [
            "`Int` is a primitive, so it can only be created by the compiler, such as with a literal",
            "`Bool` is a primitive, so it can only be created by the compiler, such as with a literal",
            "`c` is not declared",
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
                        return Self {{ age: age, isAdmin: true }}
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
            "/main: `x` is not a file, so it can't be followed by `::`",
            "/main: `Point` is not a file, so it can't be followed by `::`",
            "/main: `helper` is not a file, so it can't be followed by `::`",
            "/main: `missing` is not an imported file",
            "/main: `local` is not a file, so it can't be followed by `::`",
            "/main: `user` is not a file, so it can't be followed by `::`",
            // Even though functions can't use `x`, it's still declared.
            "/main: `x` is not a file, so it can't be followed by `::`",
        ]
    );
}

#[test]
fn files_and_identifiers_can_share_names() {
    // `::` always follows a file, so `user::age` is the file even
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
            return Self {}
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
        ["operator `+` expects a type that implements `Add`, found `Bool`"]
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
            "`a` must return `Int`, found `Bool`",
            "`b` must return a value of type `Int` on every path",
            "unknown type `Missing`",
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
            impl bool::Bool {
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
fn impls_work_across_files() {
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
                "group User {}\nimpl User {\nstatic fn new() -> User {\nreturn Self {}\n}\n}"
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
fn methods_do_not_need_their_file_imported() {
    let score = "
        use /user
        impl User {
            fn score() -> Int {
                return 1
            }
        }
    ";
    assert_eq!(
        check_files(&[
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
                "group User {}\nimpl User {\nstatic fn new() -> User {\nreturn Self {}\n}\n}"
            ),
            ("/score", score),
        ]),
        Ok(())
    );
}

#[test]
fn methods_on_std_types_do_not_need_their_file_imported() {
    assert_eq!(
        check_files(&[
            ("/main", "let a = 1.doubled()"),
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
        Ok(())
    );
}

#[test]
fn methods_are_unique_across_files_that_do_not_import_each_other() {
    let user = "group User {}";
    let score = "
        use /user
        impl User {
            fn score() -> Int {
                return 1
            }
        }
    ";
    assert_eq!(
        file_messages(&[("/user", user), ("/a", score), ("/b", score)]),
        ["/b: `User` already has a method `score`"]
    );
}

#[test]
fn functions_need_their_file_imported() {
    assert_eq!(
        file_messages(&[
            ("/main", "use /a\nlet x = f()\nlet y = b::f()"),
            ("/a", "use /b"),
            ("/b", "fn f() -> Int {\nreturn 1\n}"),
        ]),
        [
            "/main: `f` is not declared",
            "/main: `b` is not an imported file",
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
                    return Self {}
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
            return Self { age: age, isAdmin: isAdmin }
        }
    }
    group Team {
        let lead: User
        let size: Int
    }
    impl Team {
        static fn new(lead: User, size: Int) -> Team {
            return Self { lead: lead, size: size }
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
            "operator `+` expects a type that implements `Add`, found `Bool`",
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
        ["unknown type `Missing`"]
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
            "`isAdmin` is `Bool`, which doesn't implement `Call`, so it can't be called",
            "`missing` is not declared",
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
    // without importing the file that declares it.
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
                        return Self { age: age }
                    }
                }
                ",
            ),
        ]),
        Ok(())
    );
}

#[test]
fn suggests_colons_after_files() {
    assert_eq!(
        file_messages(&[
            (
                "/main",
                "
                use /user
                let a = user.User
                let b = user.make()
                // A value with the file's name takes precedence.
                fn f(user: Int) -> Int {
                    return user.age
                }
                ",
            ),
            ("/user", "group User {}\nfn make() {}"),
        ]),
        [
            "/main: `user` is a file, not a value, so use `user::User` to refer to a name in it",
            "/main: `user` is a file, not a value, so use `user::make` to refer to a name in it",
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
            return Self { x: Int.zero() }
        }
        static fn at(x: Int) -> Point {
            return Self { x: x }
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
            let alsoZero: Int = int::Int.zero()
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
fn static_functions_on_types_from_other_files() {
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
                        return Self {}
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
                    return Self { x: 0 }
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
fn static_functions_do_not_need_their_file_imported() {
    let ints = "
        impl Int {
            static fn zero() -> Int {
                return 0
            }
        }
    ";
    assert_eq!(
        check_files(&[("/main", "let a = Int.zero()"), ("/ints", ints)]),
        Ok(())
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
                    return Self {}
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
            use stdlib/int
            use stdlib/missing
            let a: stdlib::Int = 1
            ",
        )]),
        [
            "/main: `stdlib` is a package, so use one of its files, such as `stdlib/bool`",
            "/main: cannot find file `stdlib/missing`",
            "/main: `stdlib` is a package, so use one of its files, such as `stdlib/bool`",
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
            circle(radius: Int)
            dot()
        }

        impl Point {
            static fn origin() -> Self {
                return Self { x: 0 }
            }

            fn moved(by amount: Int) -> Self {
                let moved: Self = Self.origin()
                return Self { x: self.x + moved.x + amount }
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
                    Self.circle(radius: let radius) => {
                        return radius
                    }
                    Self.dot() => {
                        return 0
                    }
                }
            }

            static fn unit() -> Self {
                return Self.circle(radius: 1)
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
                return Self {{ value: 0 }}
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
                return Self { bits: .!self.bits }
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
            "`Copy` is not declared by trait `Combine`".to_string(),
            inferred("Rhs"),
            inferred("Rhs"),
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
            "`Extra` is not declared by trait `Combine`",
            "`Rhs` is already declared",
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

/// `/main`, with a `User` that `impl`s `Greet` from `/greet`, and `extra`.
fn main_with_greet(extra: &str) -> String {
    format!(
        "
        use /greet
        group User {{}}
        impl User with Greet {{}}
        {extra}
        "
    )
}

#[test]
fn default_trait_functions_use_their_own_file_s_names() {
    let greet = "
        trait Greet {
            fn greet() -> Int {
                return helper()
            }
        }
        fn helper() -> Int {
            return 1
        }
        ";
    assert_eq!(
        check_files(&[("/greet", greet), ("/main", &main_with_greet(""))]),
        Ok(())
    );
    // A name in the `impl`'s file doesn't take its place.
    let helper = "
        fn helper() -> Bool {
            return true
        }
        ";
    assert_eq!(
        check_files(&[("/greet", greet), ("/main", &main_with_greet(helper))]),
        Ok(())
    );
}

#[test]
fn default_trait_functions_use_their_own_file_s_imports() {
    let greet = "
        use /helpers
        trait Greet {
            fn greet() -> Count {
                return helper()
            }
        }
        ";
    let helpers = "
        alias Count = Int
        fn helper() -> Count {
            return 1
        }
        ";
    assert_eq!(
        check_files(&[
            ("/greet", greet),
            ("/helpers", helpers),
            ("/main", &main_with_greet("")),
        ]),
        Ok(())
    );
}

#[test]
fn default_trait_functions_use_their_own_file_s_qualifiers() {
    let greet = "
        trait Greet {
            fn greet() -> Int {
                return greet::helper()
            }
        }
        fn helper() -> Int {
            return 1
        }
        ";
    let main = "
        use /greet as renamed
        group User {}
        impl User with renamed::Greet {}
        ";
    assert_eq!(check_files(&[("/greet", greet), ("/main", main)]), Ok(()));
}

#[test]
fn default_trait_functions_can_not_use_names_from_the_impl_s_file() {
    let greet = "
        trait Greet {
            fn greet() -> Int {
                return helper()
            }
        }
        ";
    let helper = "
        fn helper() -> Int {
            return 1
        }
        ";
    assert_eq!(
        file_messages(&[("/greet", greet), ("/main", &main_with_greet(helper))]),
        ["/greet: `helper` is not declared"]
    );
}

#[test]
fn default_trait_functions_can_call_methods_from_the_impl_s_file() {
    let greet = "
        trait Greet {
            fn name() -> Int
            fn greet() -> Int {
                return self.name() + self.extra()
            }
        }
        ";
    let main = "
        use /greet
        group User {}
        impl User {
            fn extra() -> Int {
                return 1
            }
        }
        impl User with Greet {
            fn name() -> Int {
                return 2
            }
        }
        ";
    // `extra` isn't part of `Greet`, but each `impl` checks its own copy,
    // where `Self` is known.
    assert_eq!(check_files(&[("/greet", greet), ("/main", main)]), Ok(()));
}

#[test]
fn default_trait_functions_can_only_construct_groups_from_their_own_file() {
    let make = "
        trait Make {
            static fn make() -> Self {
                return Self {}
            }
        }
        ";
    let main = "
        use /make
        group User {}
        impl User with Make {}
        ";
    assert_eq!(
        file_messages(&[("/make", make), ("/main", main)]),
        ["/make: `User` can only be constructed in `/main`, which declares it"]
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
        ["operator `+` expects a type that implements `Add`, found `Bool`"]
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
fn traits_can_be_implemented_from_other_files() {
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
            "`impl Custom with And` is missing `static fn zero() -> Self`",
            "`or` is not declared by trait `And`",
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
            "unknown type `Missing`",
            "trait `Broken` already declares `f`",
            "`Twice` is already declared",
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
    // `./int` from the project looks in the project, not the standard
    // library.
    assert_eq!(
        file_messages(&[("/main", "use ./int")]),
        ["/main: cannot find file `./int`"]
    );
    assert_eq!(
        check_files(&[("/main", "use ./int"), ("/int", "group Int {}")]),
        Ok(())
    );
}

#[test]
fn project_files_can_share_names_with_stdlib_files() {
    let clash =
        "`int` is the name of `stdlib/int` and `/int`, so use `as` to give them different names";
    assert_eq!(
        check_files(&[
            ("/main", "use /int\nlet a: Bool = flag"),
            ("/int", "let flag = true"),
        ]),
        Ok(())
    );
    assert_eq!(
        file_messages(&[
            ("/main", "use /int\nlet a = int::flag"),
            ("/int", "let flag = true"),
        ]),
        [format!("/main: {clash}")]
    );
    // Aliasing the standard library's leaves the name for the project's.
    assert_eq!(
        check_files(&[
            (
                "/main",
                "
                use /int
                use stdlib/int as std
                let a: Bool = int::flag
                let b: std::Int = 1
                ",
            ),
            ("/int", "let flag = true"),
        ]),
        Ok(())
    );
    // It's the same in `/int` itself.
    assert_eq!(
        file_messages(&[("/int", "let a = 1\nlet b = int::a")]),
        [
            "/int: `int` is the name of `/int` and `stdlib/int`, so use `as` to give them different names"
        ]
    );
    assert_eq!(
        check_files(&[(
            "/int",
            "use stdlib/int as std\nlet a = 1\nlet b: std::Int = int::a"
        )]),
        Ok(())
    );
    assert_eq!(
        check_files(&[
            (
                "/main",
                "
                use /int as mine
                let a: Bool = flag
                let b: Bool = mine::flag
                let c: Int = int::Int.zero()
                ",
            ),
            (
                "/int",
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
fn programs_import_the_stdlib_without_a_use() {
    assert_eq!(
        check_top("let x: Void = ()\nlet y: Bool = true\nlet z: int::Int = 1"),
        Ok(())
    );
}

#[test]
fn programs_import_the_whole_stdlib_without_a_use() {
    // Including a file added to it, as well as its own.
    let stdlib = parsed_files(&[("/extra", "let fromExtra = 1")]);
    let project = parsed_files(&[(
        "/main",
        "let a: Int = fromExtra\nlet b: Int = extra::fromExtra",
    )]);
    let mut checker = Checker::default();
    checker.add_stdlib();
    checker.add_file(Some(STDLIB_PACKAGE), &stdlib[0]);
    checker.add_file(None, &project[0]);
    checker.check_files();
    assert_eq!(checker.errors, []);
}

#[test]
fn modules_need_a_slash_to_be_imported() {
    // Whichever way the path is written, including when its files are
    // only in its subdirectories.
    for (main, path) in [
        ("use ./dir", "./dir"),
        ("use /dir", "/dir"),
        ("use ./dir/sub", "./dir/sub"),
        ("use /dir/sub", "/dir/sub"),
    ] {
        let files = [("/main", main), ("/dir/sub/other", "let x = 1")];
        assert_eq!(
            file_messages(&files),
            [format!(
                "/main: `{path}` is a module, so use `{path}/` to import every file in it"
            )]
        );
        let errs = check_files(&files).expect_err("the slash is missing");
        assert_eq!(pointed_at(&files, &errs[0]), main);

        // Following the advice works.
        let fixed = format!("{main}/\nlet y = x");
        assert_eq!(
            check_files(&[("/main", &fixed), ("/dir/sub/other", "let x = 1")]),
            Ok(())
        );
    }
}

#[test]
fn files_can_not_be_imported_with_a_slash() {
    for (main, path) in [
        ("use ./lib/", "./lib"),
        ("use /lib/", "/lib"),
        ("use stdlib/int/", "stdlib/int"),
    ] {
        let files = [("/main", main), ("/lib", "let x = 1")];
        assert_eq!(
            file_messages(&files),
            [format!(
                "/main: `{path}` is a file, so use it without the trailing slash"
            )]
        );
        let errs = check_files(&files).expect_err("the slash shouldn't be there");
        assert_eq!(pointed_at(&files, &errs[0]), main);
    }
    // Without any segments, it's not a file's path, even if `./` is
    // `/dir/`, which has the same path as the file `/dir`.
    assert_eq!(
        file_messages(&[("/dir/main", "use ./"), ("/dir", "")]),
        ["/dir/main: `./` doesn't contain any files that can be imported"]
    );
}

#[test]
fn paths_without_a_start_suggest_what_exists() {
    let message = |paths: &str| format!("/main: paths start with `./` or `/`, so use {paths}");
    let files = |main| [("/main", main), ("/dir/sub/other", ""), ("/lib", "")];
    assert_eq!(
        file_messages(&files("use dir")),
        [message("`./dir/` or `/dir/`")]
    );
    assert_eq!(
        file_messages(&files("use dir/")),
        [message("`./dir/` or `/dir/`")]
    );
    assert_eq!(
        file_messages(&files("use dir/sub")),
        [message("`./dir/sub/` or `/dir/sub/`")]
    );
    assert_eq!(
        file_messages(&files("use dir/sub/other")),
        [message("`./dir/sub/other` or `/dir/sub/other`")]
    );
    assert_eq!(
        file_messages(&files("use lib")),
        [message("`./lib` or `/lib`")]
    );
    assert_eq!(
        file_messages(&files("use lib/")),
        [message("`./lib` or `/lib`")]
    );
    // Only `/` leads to it from `/a/main`.
    assert_eq!(
        file_messages(&[("/a/main", "use lib"), ("/lib", "")]),
        ["/a/main: paths start with `./` or `/`, so use `/lib`"]
    );
    // Each is what it looks like when it could be either.
    let both = |main| [("/main", main), ("/dir", ""), ("/dir/other", "")];
    assert_eq!(
        file_messages(&both("use dir")),
        [message("`./dir` or `/dir`")]
    );
    assert_eq!(
        file_messages(&both("use dir/")),
        [message("`./dir/` or `/dir/`")]
    );
}

#[test]
fn paths_without_a_start_are_unknown_if_nothing_exists() {
    assert_eq!(
        file_messages(&[("/main", "use missing\nuse missing/\nuse missing/file")]),
        [
            "/main: cannot find file `missing`",
            "/main: cannot find module `missing/`",
            "/main: cannot find file `missing/file`",
        ]
    );
}

#[test]
fn modules_with_aliases_are_told_they_can_not_have_one() {
    assert_eq!(
        file_messages(&[("/main", "use ./dir as x"), ("/dir/other", "let y = 1")]),
        [
            "/main: `./dir` is a module, so it can't have an alias, but `./dir/` imports every file in it"
        ]
    );
}

#[test]
fn modules_are_only_suggested_if_they_have_files_to_import() {
    // `/dir/main` itself is the only file in `/dir`.
    assert_eq!(
        file_messages(&[("/dir/main", "use /dir")]),
        ["/dir/main: cannot find file `/dir`"]
    );
    assert_eq!(
        file_messages(&[("/main", "use ./missing"), ("/dir/other", "")]),
        ["/main: cannot find file `./missing`"]
    );
    // `/dir/other` is in the module `/dir/`, but `/di/` doesn't exist.
    assert_eq!(
        file_messages(&[("/main", "use /di"), ("/dir/other", "")]),
        ["/main: cannot find file `/di`"]
    );
}

#[test]
fn files_with_the_same_name_as_a_module_are_imported_as_files() {
    assert_eq!(
        check_files(&[
            ("/main", "use /dir\nlet a: Int = x"),
            ("/dir", "let x = 1"),
            ("/dir/other", "let y = true"),
        ]),
        Ok(())
    );
}

#[test]
fn imports_every_file_in_a_module() {
    assert_eq!(
        check_files(&[
            (
                "/main",
                "
                use ./users/
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
                        return Self { age: age }
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
                        return Self {}
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
                "use /users/\nlet a = top\nlet b = nested\nlet c = deeper\nlet d = outside",
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
            ("/main", "use /\nlet x = a + b + c"),
            ("/a", "let a = 1"),
            ("/dir/b", "let b = 2"),
            ("/dir/sub/c", "let c = 3"),
        ]),
        Ok(())
    );
    // `./` from inside a directory doesn't reach outside it.
    assert_eq!(
        file_messages(&[
            ("/dir/main", "use ./\nlet x = b + a"),
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
            ("/dir/main", "use ./\nuse /\nlet x = sibling + root"),
            ("/dir/sibling", "let sibling = 1"),
            ("/root", "let root = 2"),
        ]),
        Ok(())
    );
}

#[test]
fn modules_that_do_not_exist_can_not_be_found() {
    for (main, path) in [
        ("use /missing/", "/missing/"),
        ("use ./missing/", "./missing/"),
        ("use /dir/missing/", "/dir/missing/"),
        ("use ./dir/missing/", "./dir/missing/"),
        ("use stdlib/missing/", "stdlib/missing/"),
    ] {
        let files = [("/main", main), ("/dir/other", "")];
        assert_eq!(
            file_messages(&files),
            [format!("/main: cannot find module `{path}`")]
        );
        let errs = check_files(&files).expect_err("the module doesn't exist");
        assert_eq!(pointed_at(&files, &errs[0]), main);
    }
}

#[test]
fn modules_with_only_the_importing_file_have_nothing_to_import() {
    let message = "doesn't contain any files that can be imported";
    assert_eq!(
        file_messages(&[("/dir/main", "use /dir/")]),
        [format!("/dir/main: `/dir/` {message}")]
    );
    assert_eq!(
        file_messages(&[("/dir/sub/main", "use /dir/")]),
        [format!("/dir/sub/main: `/dir/` {message}")]
    );
    assert_eq!(
        file_messages(&[("/dir/main", "use ./")]),
        [format!("/dir/main: `./` {message}")]
    );
    assert_eq!(
        file_messages(&[("/main", "use /")]),
        [format!("/main: `/` {message}")]
    );
    // It exists even if there's also a file with its path.
    assert_eq!(
        file_messages(&[("/dir/main", "use /dir/"), ("/dir", "")]),
        [format!("/dir/main: `/dir/` {message}")]
    );
}

#[test]
fn module_imports_skip_the_importing_file() {
    // Only `/main` is in the root, so `/` has nothing else to import.
    assert_eq!(
        file_messages(&[("/main", "use ./\nuse /missing/")]),
        [
            "/main: `./` doesn't contain any files that can be imported",
            "/main: cannot find module `/missing/`",
        ]
    );
}

#[test]
fn module_imports_can_import_the_stdlib() {
    // The standard library is imported anyway, so this changes nothing.
    assert_eq!(
        check_files(&[("/main", "use stdlib/\nlet a: int::Int = 1")]),
        Ok(())
    );
}

#[test]
fn names_from_module_imports_can_be_ambiguous() {
    assert_eq!(
        file_messages(&[
            (
                "/main",
                "
                use ./shapes/
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
fn importing_a_module_and_its_files_is_harmless() {
    assert_eq!(
        check_files(&[
            (
                "/main",
                "use ./lib/\nuse ./lib/a\nuse ./lib/\nlet x = a + b"
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
            let a: Void = ()
            fn nothing() {}
            let b: Void = nothing()
            let c: () = nothing()
            fn f() -> void::Void {
                return ()
            }
            "
        ),
        Ok(())
    );
    assert_eq!(
        messages_top("let a: Int = ()\nlet b: Int = Void.new()"),
        [
            "cannot assign `()` to `a`, which has type `Int`",
            "`Void` has no static function `new`",
        ]
    );
}

#[test]
fn self_constructs_the_type_of_its_impl() {
    // So it's only in an `impl`, and never constructs another type. Its
    // values are still checked outside one.
    assert_eq!(
        messages_top(
            "
            group Point {}
            group Other {}
            let a = Self { value: y }
            fn f() -> Point {
                return Self {}
            }
            impl Other {
                static fn make() -> Point {
                    return Self {}
                }
            }
            "
        ),
        [
            "`Self` can only be used inside an `impl` or a `trait`",
            "`y` is not declared",
            "`Self` can only be used inside an `impl` or a `trait`",
            "`make` must return `Point`, found `Other`",
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
                    return Self {}
                }
                fn copy() -> Point {
                    fn make() -> Point {
                        return Self {}
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
                    return Self {}
                }
            }
            "
        ),
        ["unknown type `Missing`"]
    );
}

/// A choice for tests, with options holding values of different types, and
/// two holding none.
const SHAPE: &str = "
    choice Shape {
        circle(radius: Int)
        empty()
        nothing()
    }
    ";

#[test]
fn accepts_choices() {
    let src = format!(
        "
        {SHAPE}
        let a: Shape = Shape.circle(radius: 3)
        let b: Shape = Shape.empty()
        let c: Shape = Shape.nothing()
        fn f() -> Shape {{
            choice Inner {{
                only()
            }}
            let inner: Inner = Inner.only()
            return Shape.circle(radius: 1 + 2)
        }}
        "
    );
    assert_eq!(check_top(&src), Ok(()));
}

#[test]
fn options_can_be_used_anywhere() {
    // Unlike groups, choices can be created outside their `impl` and
    // their file.
    assert_eq!(
        check_files(&[
            (
                "/main",
                "
                use /shapes
                let a = Shape.circle(radius: 1)
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
                return Shape.circle(radius: 1)
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
            let a = Shape.circle(radius: true)
            let b = Shape.circle(1)
            let c = Shape.circle(size: 1)
            let d = Shape.circle()
            let e = Shape.circle(radius: 1, 2)
            let f = Shape.empty(1)
            let g = Shape.empty(())
            "
        )),
        [
            "argument `radius` of `Shape.circle` expects `Int`, found `Bool`",
            "argument to `Shape.circle` is missing the label `radius`",
            "argument to `Shape.circle` should be labelled `radius`, found `size`",
            "`Shape.circle` takes 1 argument, found 0",
            "`Shape.circle` takes 1 argument, found 2",
            "`Shape.empty` takes 0 arguments, found 1",
            "`Shape.empty` takes 0 arguments, found 1",
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
            "cannot assign `ChoiceOption<Shape, (radius: Int)>` to `a`, which has type `Shape`",
            "cannot assign `ChoiceOption<Shape, ()>` to `b`, which has type `Shape`",
            "cannot assign `ChoiceOption<Shape, (radius: Int)>` to `c`, which has type `ChoiceOption<Shape, Bool>`",
        ]
    );
}

#[test]
fn options_are_values_with_tags() {
    let src = "
        choice Shape {
            square(size: Int)
            circle(filled: Bool)
            other()
        }

        let a: ChoiceOption<Shape, (size: Int)> = Shape.square
        let b: ChoiceTag<Shape> = Shape.square.tag()
        let c: Shape = Shape.square(size: 5)
        let d: ChoiceTag<Shape> = ChoiceTag.new(of: Shape.square(size: 5))

        let n: ChoiceOption<Shape, Void> = Shape.other
        let o: ChoiceTag<Shape> = Shape.other.tag()
        let p: Shape = Shape.other()
        let q: ChoiceTag<Shape> = ChoiceTag.new(of: Shape.other())
        ";
    assert_eq!(check_top(src), Ok(()));
}

#[test]
fn options_can_be_called_like_other_values() {
    let src = format!(
        "
        {SHAPE}
        group Maker {{
            let make: ChoiceOption<Shape, (radius: Int)>
        }}

        fn make(with option: ChoiceOption<Shape, (radius: Int)>) -> Shape {{
            return option(radius: 1)
        }}

        fn f(maker: Maker) {{
            let circle = Shape.circle
            let a: Shape = circle(radius: 1)
            let b: Shape = circle.call(input: (radius: 1))
            let c: Shape = Shape.circle.call(input: (radius: 1))
            let d: Shape = make(with: Shape.circle)
            let e: Shape = maker.make(radius: 1)
            let empty = Shape.empty
            let g: Shape = empty()
            let h: Shape = empty.call(input: ())
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
                let a = circle(radius: true)
                let b = circle(1)
                let c = circle()
                let d = circle(radius: 1, 2)
                let empty = Shape.empty
                let e = empty(1)
                let f = empty(())
            }}
            "
        )),
        [
            "`circle` is called with `(radius: Int)`, but was given `(radius: Bool)`",
            "`circle` is called with `(radius: Int)`, but was given `(_: Int)`",
            "`circle` is called with `(radius: Int)`, but was given `()`",
            "`circle` is called with `(radius: Int)`, but was given `(radius: Int, _: Int)`",
            "`empty` is called with `()`, but was given `(_: Int)`",
            "`empty` is called with `()`, but was given `(_: ())`",
        ]
    );
}

#[test]
fn options_take_each_of_their_values_as_an_argument() {
    // Their value is built from the arguments, so a tuple isn't unpacked
    // into them.
    let pair = "
        choice Shape {
            pair(a: Int, b: Int)
        }
        ";
    let src = format!(
        "
        {pair}
        fn f() {{
            let a: Shape = Shape.pair(a: 1, b: 2)
            let pair = Shape.pair
            let b: Shape = pair(a: 1, b: 2)
            let c: Shape = pair.call(input: (a: 1, b: 2))
        }}
        "
    );
    assert_eq!(check_top(&src), Ok(()));
    assert_eq!(
        messages_top(&format!(
            "
            {pair}
            fn f() {{
                let a = Shape.pair((a: 1, b: 2))
                let pair = Shape.pair
                let b = pair((a: 1, b: 2))
            }}
            "
        )),
        [
            "`Shape.pair` takes 2 arguments, found 1",
            "`pair` is called with `(a: Int, b: Int)`, but was given `(_: (a: Int, b: Int))`",
        ]
    );
}

#[test]
fn values_taking_void_are_called_without_arguments() {
    // The arguments are built into `()`, which is `Void`.
    let runner = "
        group Runner {}

        impl Runner with Call {
            fn call(input: Void) -> Int {
                return 1
            }
        }
        ";
    assert_eq!(
        check_top(&format!(
            "
            {runner}
            fn f(runner: Runner) -> Int {{
                return runner() + runner.call(input: ())
            }}
            "
        )),
        Ok(())
    );
    assert_eq!(
        messages_top(&format!(
            "
            {runner}
            fn f(runner: Runner) -> Int {{
                return runner(1)
            }}
            "
        )),
        ["`runner` is called with `Void`, but was given `(_: Int)`"]
    );
}

#[test]
fn options_of_generic_choices_infer_their_arguments() {
    let src = format!(
        "
        {GENERICS}
        let a: ChoiceOption<Optional<Int>, Void> = Optional.none
        let b: ChoiceOption<Optional<Int>, (value: Int)> = Optional.some
        let c: Optional<Int> = Optional.none()
        let d: Optional<Int> = Optional<Int>.some(value: 1)
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
            "cannot assign `ChoiceOption<Optional<Int>, ()>` to `a`, which has type `Optional<Int>`",
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
                only()
            }}
            let a = Shape.empty.tag() == Other.only.tag()
            let b = Shape.empty.tag() == Shape.empty
            let c: ChoiceTag<Shape> = 0
            let d: ChoiceTag<Shape> = Other.only.tag()
            "
        )),
        [
            "operator `==` expects `ChoiceTag<Shape>`, found `ChoiceTag<Other>`",
            "operator `==` expects `ChoiceTag<Shape>`, found `ChoiceOption<Shape, ()>`",
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
                    Shape.empty() => {{}}
                    _ => {{}}
                }}
                match Shape.empty {{
                    Shape.empty() => {{}}
                    _ => {{}}
                }}
            }}
            "
        )),
        [
            "`match` is on `ChoiceTag<Shape>`, so it can't match an option of `Shape`",
            "`match` is on `ChoiceOption<Shape, ()>`, so it can't match an option of `Shape`",
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
        let b: Shape = Shape.circle.make(with: (radius: 1))
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
            tag(value: ChoiceTag<Nested>)
            other(value: ChoiceTag<Later>)
        }

        choice Generic<T> {
            value(value: T)
            tag(value: ChoiceTag<Generic<T>>)
        }

        let a: ShapeTag = Shape.empty.tag()

        fn f() {
            choice Local {
                only()
                tag(value: ChoiceTag<Local>)
            }
            let tag: ChoiceTag<Local> = Local.only.tag()
            let b: ChoiceTag<Generic<Int>> = Generic<Int>.value.tag()
        }

        choice Shape {
            empty()
        }

        choice Later {
            empty()
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
                only()
            }}
            let a = ChoiceTag<Shape>.new(of: Other.only())
            let b = ChoiceTag<Shape>.new(Shape.empty())
            let c = ChoiceTag<Shape>.new()
            let d = ChoiceTag.new()
            "
        )),
        [
            "argument `of` of `ChoiceTag<Shape>.new` expects `Shape`, found `Other`",
            "argument to `ChoiceTag<Shape>.new` is missing the label `of`",
            "`ChoiceTag<Shape>.new` takes 1 argument, found 0",
            "the arguments of `ChoiceTag` can't be inferred here, so they must be given, as in `ChoiceTag<...>`",
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
            "`ChoiceTag` can only be used with a choice, found `Int`",
            // Generic types are checked before top-level values.
            "`ChoiceOption` can only be used with a choice, found `Point`",
            "`ChoiceTag` can only be used with a choice, found `Int`",
            "unknown type `Missing`",
            "`ChoiceTag` can only be used with a choice, found `Bool`",
            "`ChoiceTag` can only be used with a choice, found `T`",
        ]
    );
}

#[test]
fn tags_are_available_from_any_file() {
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
            tag(id: Int)
            Tag()
        }

        impl Shape {
            fn tag() -> Int {
                return 1
            }
        }

        trait Choice {}

        let a: Int = Shape.tag(id: 1).tag()
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
            let a = Shape.square(size: 1)
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
            let b = shape.circle(radius: 1)
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
                    return Self {{}}
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
                circle(radius: Int)
                circle(filled: Bool)
            }
            impl Shape {
                static fn circle() -> Shape {
                    return Shape.circle(radius: 1)
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
                empty()
                node(next: List)
            }
            group Node {
                let next: Link
            }
            choice Link {
                none()
                some(node: Node)
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
                circle(radius: Circle)
            }
            let a = Shape.circle(radius: 1)
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
                Shape.circle(radius: let radius) => {{
                    area = radius * radius * 3
                }}
                // The value doesn't have to be matched.
                Shape.empty() => {{}}
                Shape.nothing() => {{}}
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
                    Shape.circle(radius: let value) => {{
                        let wrong: Bool = value
                    }}
                    Shape.empty() => {{}}
                    Shape.nothing() => {{}}
                }}
            }}
            "
        )),
        ["cannot assign `Int` to `wrong`, which has type `Bool`"]
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
                    Shape.circle(radius: let radius) => {{
                        radius = 1
                    }}
                    Shape.empty() => {{}}
                    Shape.nothing() => {{
                        let x = radius
                    }}
                }}
                match shape {{
                    Shape.circle(radius: let taken) => {{}}
                    _ => {{}}
                }}
                let y = radius
            }}
            "
        )),
        [
            "cannot assign to `radius` because it was declared with `let`",
            "`radius` is not declared",
            "`taken` is already declared",
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
                line(ends: (from: Line, to: Line))
                none()
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
            "`Node` contains itself",
            "`Self` can't be used inside another type in a trait's signatures, since it's a different type in each `impl`",
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
                (Shape.circle(radius: 0), _) => {{}}
                (Shape.circle(radius: let radius), let scale) => {{
                    return radius * scale
                }}
                (Shape.empty(), _) => {{}}
                (Shape.nothing(), 3) => {{}}
                (Shape.nothing(), _) => {{}}
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
        choice Switch {{
            set(isOn: Bool, count: Int)
            unset()
        }}
        fn f(pair: (a: Bool, b: Bool), shape: (_: Shape, _: Int), n: IntU8) {{
            match pair {{
                (a: true, b: true) => {{}}
            }}
            match shape {{
                (Shape.circle(radius: 1), _) => {{}}
                (_, 0) => {{}}
            }}
            match n {{
                0 => {{}}
            }}
            match pair {{
                (a: true, b: _) => {{}}
            }}
        }}
        fn g(switch: Switch) {{
            match switch {{
                Switch.set(isOn: true, count: _) => {{}}
            }}
        }}
        "
    );
    assert_eq!(
        messages_top(&src),
        [
            "`match` is missing the patterns `(a: true, b: false)` and `(a: false, b: _)`",
            "`match` is missing the patterns `(Shape.circle(radius: _), _)`, `(Shape.empty(), _)` and `(Shape.nothing(), _)`",
            "`match` is missing the pattern `_`",
            "`match` is missing the pattern `(a: false, b: _)`",
            "`match` is missing the patterns `Switch.set(isOn: false, count: _)` and `Switch.unset()`",
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
                Shape.circle(radius: _) => {{}}
                Shape.circle(radius: 1) => {{}}
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
            "pattern `Shape.circle(radius: 1)` can't be reached, since the patterns before it match everything it does",
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
                Shape.empty() => {{}}
            }}
            match shape {{
                Shape.circle(radius: true) => {{}}
                Shape.circle(x: _) => {{}}
                Shape.empty(_) => {{}}
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
            "pattern `(x: _)` can't match `(radius: Int)`",
            "pattern `(_)` can't match `()`",
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
            (Shape.circle(radius: let r)) = shape
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
            "pattern `(Shape.circle(radius: let r))` doesn't match every value, such as `(Shape.empty())`, so it can only be used in a `match`",
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

    group Negator {}

    impl Negator with Call {
        fn call(input: (_ value: Int)) -> Int {
            return -input.value
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
        let negate: Negator
    }
    ";

#[test]
fn values_implementing_call_can_be_called() {
    // By name or as a field, with the input written like a tuple without
    // its brackets, or by calling `call` itself.
    let src = format!(
        "
        {CALLABLES}
        fn f(adder: Adder, negate: Negator, counter: Counter, tools: Tools) -> Int {{
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
        fn f(adder: Adder, negate: Negator, tools: Tools, count: Int) {{
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
            "`negate` is called with `(_ value: Int)`, but was given `(_: Bool)`",
            "`negate` is called with `(_ value: Int)`, but was given `()`",
            "`missing` is not declared",
            "`unknown` is not declared",
        ]
    );
}

#[test]
fn functions_are_values_of_type_fn() {
    // The `Input` is a tuple of the parameters, by their labels.
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
            let b: Fn<(_: Int), Int> = negate
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
            "argument `a` of `small` expects `IntU8`, found `Int`",
            "operator `+` expects `Int`, found `Bool`",
            "argument to `small` should be labelled `b`, found `c`",
            "`small` takes 2 arguments, found 1",
            "`missing` is not declared",
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
            let of: Fn<(_: Int), Box<Int>> = Box<Int>.of
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
fn static_functions_as_values_do_not_need_their_file_imported() {
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
    assert_eq!(check_files(&[("/ints", ints), ("/other", other)]), Ok(()));
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
fn functions_from_other_files_are_values_once_imported() {
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
        ["/main: `other` is not an imported file"]
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
        none()
        some(value: T)
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
    }
    ";

#[test]
fn generic_arguments_are_inferred_from_calls() {
    let src = format!(
        "
        {GENERICS}
        choice Shape {{
            empty()
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
            boxed(value: Box<T>)
            pair(a: T, b: Bool)
        }}

        let a: Box<Int> = Box.of(1)
        let b: Bool = Box.of(true).get()
        let c: Optional<Int> = Optional<Int>.none()
        let d: Int = Box.of(Box.of(1)).get().get()
        let e: Optional<Bool> = Optional.some(value: true)
        let f = Optional.some(value: 1)
        let g: Optional<Int> = f
        let h: Pair<Int, Bool> = Pair.of(first: 1, second: true)
        let i: Pair<Int, Bool> = Pair.tupled(pair: (a: 1, b: true))
        let j: Box<Int> = Box.unwrapping(Box.of(1))
        let k: Pair<Int, Bool> = Pair.copy(of: h)
        let l: Nested<Int> = Nested.boxed(value: Box.of(1))
        let m: Nested<Int> = Nested.pair(a: 1, b: true)
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
            none()
            some(value: T)
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
            let b: Box<IntU8> = Box.with(value: small, fallback: Optional.some(value: 1))
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
                empty()
            }}

            fn f() {{
                let a = Optional.none()
                let b = Optional.none(())
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
fn inferred_static_functions_do_not_need_their_file_imported() {
    let checked = check_files(&[
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
    assert_eq!(checked, Ok(()));
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
                Optional.some(value: (x: let x, y: true)) => {{
                    return x + first.get()
                }}
                Optional<(x: Int, y: Bool)>.some(value: _) => {{}}
                Optional.none() => {{}}
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
                return Self {{ value: self.value }}
            }}
        }}

        fn none() -> Optional<Int> {{
            return Optional.none()
        }}

        fn f(_ value: Optional<Box<Bool>>) -> Box<Int> {{
            let some: Optional<Int> = Optional.some(value: 1)
            let boxed: Box<Int> = Box.of(2)
            match value {{
                Optional.some(value: let found) => {{
                    let flag: Bool = found.get()
                }}
                Optional.none() => {{}}
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
                    Optional.none() => {{}}
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
    assert_eq!(
        messages_top(&src),
        [
            "`Box<T>` has no generic parameter `U`",
            "unknown type `T`",
            "`Self.T` can only be used in the `impl` of a generic type",
            "`Self.T` can only be used in the `impl` of a generic type",
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
            "operator `+` expects a type that implements `Add`, found `T`",
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
            none()
            some(value: T)
        }

        group Grow<T> {
            let next: Optional<Grow<(_: T)>>
        }

        fn f() {
            group Local<T> {}
        }
        ";
    let messages = messages_top(src);
    assert_eq!(messages[0], "`Wrap<T>` contains itself");
    assert!(
        messages[1].contains("Grow<(_: (_: ")
            && messages[1].ends_with(
                "nests generic types too deeply, which usually means a generic type uses itself with different arguments"
            ),
        "{messages:?}"
    );
    assert_eq!(
        messages[2..],
        ["`Local` can't be generic, since only types at the top level can be"]
    );
}

#[test]
fn generic_functions_cannot_use_themselves_with_larger_arguments() {
    // Each type made from `Box` would need its own `nested`, forever.
    let src = "
        group Box<T> {
            let value: T
        }

        impl Box {
            static fn of(_ value: Self.T) -> Self {
                return Self { value: value }
            }

            fn nested() -> Int {
                return Box<Self>.of(self).nested()
            }
        }

        fn f() -> Int {
            return Box<Int>.of(1).nested()
        }
        ";
    let errors = check_files(&[("/main", src)]).expect_err("`nested` never ends");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(&errors[0].error, CheckError::GenericTooDeep(ty) if ty.starts_with("Box<Box<")),
        "{errors:?}"
    );
    assert_eq!(
        pointed_at(&[("/main", src)], &errors[0]),
        "Box<Self>.of(self)"
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
                circle(radius: Int)
            }}
            fn f(shape: Shape) {{
                match shape {{
                    Other.circle(radius: _) => {{}}
                    Shape.square(size: _) => {{}}
                    Missing.circle(radius: _) => {{}}
                    Shape.circle(radius: _) => {{}}
                    Shape.circle(radius: let radius) => {{}}
                    Shape.empty() => {{}}
                    Shape.nothing() => {{}}
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
                    Shape.empty() => {{}}
                }}
                match shape {{
                    Shape.empty() => {{}}
                    Shape.circle(radius: _) => {{}}
                }}
            }}
            "
        )),
        [
            "`match` is missing the patterns `Shape.circle(radius: _)` and `Shape.nothing()`",
            "`match` is missing the pattern `Shape.nothing()`",
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
                Shape.circle(radius: let radius) => {{
                    return radius
                }}
                Shape.empty() => {{
                    if true {{
                        return 1
                    }} else {{
                        return 2
                    }}
                }}
                Shape.nothing() => {{
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
                    Shape.circle(radius: let radius) => {{
                        return radius
                    }}
                    Shape.empty() => {{}}
                    Shape.nothing() => {{
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
                    Shape.circle(radius: let radius) => {{
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
                    return Self {}
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
                        return Self { x: x }
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
                Figure.circle(radius: let radius) => {{
                    return radius
                }}
                Shape.empty() => {{
                    return 0
                }}
                Figure.nothing() => {{
                    return 0
                }}
            }}
        }}
        let a = size(of: Figure.circle(radius: 2))
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
                    Figure.circle(radius: _) => {{}}
                    Shape.circle(radius: _) => {{}}
                    Shape.empty() => {{}}
                    Shape.nothing() => {{}}
                }}
            }}
            "
        )),
        [
            "pattern `Shape.circle(radius: _)` can't be reached, since the patterns before it match everything it does"
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
    // A file's own names come before the standard library's.
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
                return Self { red: 200 }
            }

            fn brighter(by amount: IntU8) -> IntU8 {
                return self.red + amount
            }
        }

        choice Level {
            exact(level: IntU8)
            none()
        }

        fn half(of byte: IntU8) -> IntU8 {
            return byte / 2
        }

        fn f() -> IntU8 {
            var byte: IntU8 = 250
            byte = 255
            let level = Level.exact(level: 255)
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
        ["operator `-` expects a type that implements `Negate`, found `IntU8`"]
    );
}
