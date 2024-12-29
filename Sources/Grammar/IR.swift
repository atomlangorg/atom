//
//  IR.swift
//  atom
//
//  Created by George Elsham on 20/07/2024.
//

protocol IR {
    func swift() -> SwiftCode
}

struct NeverIr: IR {
    func swift() -> SwiftCode {
        fatalError()
    }
}

struct SwiftIr: IR, StatementIrProtocol {
    let code: String

    func swift() -> SwiftCode {
        SwiftCode(code)
    }
}

struct RawStringIr: IR {
    let string: String

    func swift() -> SwiftCode {
        SwiftCode(string)
    }
}

struct IdentifierIr: IR {
    let name: String

    func swift() -> SwiftCode {
        SwiftCode(name)
    }
}

struct IntegerIr: IR, IntegerExprIrProtocol {
    let value: Int

    func swift() -> SwiftCode {
        SwiftCode("\(value)")
    }
}

struct IntegerExprIr: IR, ExpressionIrProtocol {
    let expression: any IntegerExprIrProtocol

    func swift() -> SwiftCode {
        expression.swift()
    }
}

struct IntegerNegateExprIr: IR, IntegerExprIrProtocol {
    let expr: IntegerExprIr

    func swift() -> SwiftCode {
        SwiftCode("-\(expr.swift())")
    }
}

struct IntegerAddExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) + \(rhs.swift()))")
    }
}

struct IntegerSubtractExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) - \(rhs.swift()))")
    }
}

struct IntegerMultiplyExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) * \(rhs.swift()))")
    }
}

struct IntegerDivideExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) / \(rhs.swift()))")
    }
}

struct StringIr: IR, ExpressionIrProtocol {
    let string: String

    func swift() -> SwiftCode {
        var debugString = ""
        debugPrint(string, terminator: "", to: &debugString)
        return SwiftCode(debugString)
    }
}

struct BooleanIr: IR, BooleanExprIrProtocol {
    let boolean: Bool

    func swift() -> SwiftCode {
        SwiftCode("\(boolean)")
    }
}

struct BooleanExprIr: IR, ExpressionIrProtocol {
    let expression: any BooleanExprIrProtocol

    func swift() -> SwiftCode {
        expression.swift()
    }
}

struct BooleanNotExprIr: IR, BooleanExprIrProtocol {
    let expr: BooleanExprIr

    func swift() -> SwiftCode {
        SwiftCode("!\(expr.swift())")
    }
}

struct BooleanAndExprIr: IR, BooleanExprIrProtocol {
    let lhs: BooleanExprIr
    let rhs: BooleanExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) && \(rhs.swift()))")
    }
}

struct BooleanOrExprIr: IR, BooleanExprIrProtocol {
    let lhs: BooleanExprIr
    let rhs: BooleanExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) || \(rhs.swift()))")
    }
}

struct BooleanIntegerEqualExprIr: IR, BooleanExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) == \(rhs.swift()))")
    }
}

struct BooleanStringEqualExprIr: IR, BooleanExprIrProtocol {
    let lhs: StringIr
    let rhs: StringIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) == \(rhs.swift()))")
    }
}

struct BooleanBooleanEqualExprIr: IR, BooleanExprIrProtocol {
    let lhs: BooleanExprIr
    let rhs: BooleanExprIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) == \(rhs.swift()))")
    }
}

struct ExpressionIr: IR {
    let expression: any ExpressionIrProtocol

    func swift() -> SwiftCode {
        expression.swift()
    }
}

struct AssignmentIr: IR, StatementIrProtocol {
    let variable: IdentifierIr
    let expression: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("let \(variable.swift()) = \(expression.swift())")
    }
}

struct StaticAssignmentIr: IR, ImplStatementProtocol {
    let assignment: AssignmentIr

    func swift() -> SwiftCode {
        SwiftCode("static \(assignment.swift())")
    }
}

struct StructFieldIr: IR {
    let identifier: IdentifierIr
    let type: IdentifierIr
    let isMutable: Bool

    func swift() -> SwiftCode {
        SwiftCode("\(isMutable ? "var" : "let") \(identifier.name): \(type.name)")
    }
}

struct StructFieldsIr: IR {
    let fields: [StructFieldIr]

    func swift() -> SwiftCode {
        var str = ""
        for field in fields {
            str.append("\(field.swift())\n")
        }
        return SwiftCode(str)
    }
}

struct StructIr: IR, StatementIrProtocol {
    let identifier: IdentifierIr
    let fields: StructFieldsIr

    func swift() -> SwiftCode {
        SwiftCode("struct \(identifier.name) {\n\(fields.swift())}")
    }
}

struct VariantValueIr: IR {
    let identifier: IdentifierIr
    let type: IdentifierIr

    func swift() -> SwiftCode {
        SwiftCode("case \(identifier.name)(\(type.name))")
    }
}

struct VariantValuesIr: IR {
    let values: [VariantValueIr]

    func swift() -> SwiftCode {
        var str = ""
        for value in values {
            str.append("\(value.swift())\n")
        }
        return SwiftCode(str)
    }
}

struct VariantIr: IR, StatementIrProtocol {
    let identifier: IdentifierIr
    let values: VariantValuesIr

    func swift() -> SwiftCode {
        SwiftCode("enum \(identifier.name) {\n\(values.swift())}")
    }
}

struct ImplIr: IR, StatementIrProtocol {
    let typeIdentifier: IdentifierIr
    let statements: [ImplStatementIr]

    func swift() -> SwiftCode {
        let code = statements
            .map { statement in
                statement.swift().raw.string
            }
            .joined(separator: "\n")
        return SwiftCode("impl \(typeIdentifier.name) {\n\(code)}")
    }
}

struct StatementIr: IR {
    let ir: any StatementIrProtocol

    func swift() -> SwiftCode {
        ir.swift()
    }
}

struct ImplStatementIr: IR {
    let ir: any ImplStatementProtocol

    func swift() -> SwiftCode {
        ir.swift()
    }
}

struct ProgramIr: IR {
    let statements: [StatementIr]

    func swift() -> SwiftCode {
        let code = statements
            .map { statement in
                statement.swift().raw.string
            }
            .joined(separator: "\n")
        return SwiftCode(code)
    }
}

protocol IntegerExprIrProtocol: IR {}

protocol BooleanExprIrProtocol: IR {}

protocol ExpressionIrProtocol: IR {}

protocol StatementIrProtocol: IR {}

protocol ImplStatementProtocol: IR {}
