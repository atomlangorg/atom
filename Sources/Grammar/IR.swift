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

struct IdentifierIr: IR, ExpressionIrProtocol {
    let name: String

    func swift() -> SwiftCode {
        SwiftCode(name)
    }
}

struct IntegerIr: IR, ExpressionIrProtocol {
    let value: Int

    func swift() -> SwiftCode {
        SwiftCode("\(value)")
    }
}

struct NegateExprIr: IR, ExpressionIrProtocol {
    let expr: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("-\(expr.swift())")
    }
}

struct AddExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) + \(rhs.swift()))")
    }
}

struct SubtractExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) - \(rhs.swift()))")
    }
}

struct MultiplyExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) * \(rhs.swift()))")
    }
}

struct DivideExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

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

struct BooleanIr: IR, ExpressionIrProtocol {
    let boolean: Bool

    func swift() -> SwiftCode {
        SwiftCode("\(boolean)")
    }
}

struct NotExprIr: IR, ExpressionIrProtocol {
    let expr: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("!\(expr.swift())")
    }
}

struct AndExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) && \(rhs.swift()))")
    }
}

struct OrExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

    func swift() -> SwiftCode {
        SwiftCode("(\(lhs.swift()) || \(rhs.swift()))")
    }
}

struct EqualExprIr: IR, ExpressionIrProtocol {
    let lhs: ExpressionIr
    let rhs: ExpressionIr

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

struct ImplStatementsIr: IR {
    let statements: [ImplStatementIr]

    func swift() -> SwiftCode {
        var str = ""
        for statement in statements {
            str.append("\(statement.swift())\n")
        }
        return SwiftCode(str)
    }
}

struct ImplIr: IR, StatementIrProtocol {
    let typeIdentifier: IdentifierIr
    let statements: ImplStatementsIr

    func swift() -> SwiftCode {
        SwiftCode("protocol \(typeIdentifier.name) {\n\(statements.swift())}")
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

protocol ExpressionIrProtocol: IR {}

protocol StatementIrProtocol: IR {}

protocol ImplStatementProtocol: IR {}

protocol IntermediateExprProtocol: IR {
    func with(lhs: ExpressionIr) -> ExpressionIr
}

struct IntermediateExprIR: IR {
    let expression: any IntermediateExprProtocol

    func with(lhs: ExpressionIr) -> ExpressionIr {
        expression.with(lhs: lhs)
    }

    func swift() -> SwiftCode {
        expression.swift()
    }
}

struct IntermediateHalfAddExprIr: IR, IntermediateExprProtocol {
    let rhs: ExpressionIr

    func with(lhs: ExpressionIr) -> ExpressionIr {
        let expr = AddExprIr(lhs: lhs, rhs: rhs)
        return ExpressionIr(expression: expr)
    }

    func swift() -> SwiftCode {
        fatalError()
    }
}

struct IntermediateHalfSubtractExprIr: IR, IntermediateExprProtocol {
    let rhs: ExpressionIr

    func with(lhs: ExpressionIr) -> ExpressionIr {
        let expr = SubtractExprIr(lhs: lhs, rhs: rhs)
        return ExpressionIr(expression: expr)
    }

    func swift() -> SwiftCode {
        fatalError()
    }
}

struct IntermediateHalfMultiplyExprIr: IR, IntermediateExprProtocol {
    let rhs: ExpressionIr

    func with(lhs: ExpressionIr) -> ExpressionIr {
        let expr = MultiplyExprIr(lhs: lhs, rhs: rhs)
        return ExpressionIr(expression: expr)
    }

    func swift() -> SwiftCode {
        fatalError()
    }
}

struct IntermediateHalfDivideExprIr: IR, IntermediateExprProtocol {
    let rhs: ExpressionIr

    func with(lhs: ExpressionIr) -> ExpressionIr {
        let expr = DivideExprIr(lhs: lhs, rhs: rhs)
        return ExpressionIr(expression: expr)
    }

    func swift() -> SwiftCode {
        fatalError()
    }
}
