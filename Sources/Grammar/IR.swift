//
//  IR.swift
//  atom
//
//  Created by George Elsham on 20/07/2024.
//

protocol IR {
    func swift() -> String
}

struct NeverIr: IR {
    func swift() -> String {
        fatalError()
    }
}

struct RawStringIr: IR {
    let string: String

    func swift() -> String {
        string
    }
}

struct VariableIr: IR {
    let name: String

    func swift() -> String {
        name
    }
}

struct IntegerIr: IR, IntegerExprIrProtocol {
    let value: Int

    func swift() -> String {
        "\(value)"
    }
}

struct IntegerExprIr: IR, ExpressionIrProtocol {
    let expression: any IntegerExprIrProtocol

    func swift() -> String {
        expression.swift()
    }
}

struct IntegerNegateExprIr: IR, IntegerExprIrProtocol {
    let expr: IntegerExprIr

    func swift() -> String {
        "-\(expr.swift())"
    }
}

struct IntegerAddExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> String {
        "(\(lhs.swift()) + \(rhs.swift()))"
    }
}

struct IntegerSubtractExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> String {
        "(\(lhs.swift()) - \(rhs.swift()))"
    }
}

struct IntegerMultiplyExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> String {
        "(\(lhs.swift()) * \(rhs.swift()))"
    }
}

struct IntegerDivideExprIr: IR, IntegerExprIrProtocol {
    let lhs: IntegerExprIr
    let rhs: IntegerExprIr

    func swift() -> String {
        "(\(lhs.swift()) / \(rhs.swift()))"
    }
}

struct StringIr: IR, ExpressionIrProtocol {
    let string: String

    func swift() -> String {
        "\"\(string)\""
    }
}

struct AssignmentIr: IR, StatementIrProtocol {
    let variable: VariableIr
    let expression: any ExpressionIrProtocol

    func swift() -> String {
        "let \(variable.swift()) = \(expression.swift())"
    }
}

struct StatementIr: IR {
    let ir: any StatementIrProtocol

    func swift() -> String {
        ir.swift()
    }
}

struct ProgramIr: IR {
    let statements: [StatementIr]

    func swift() -> String {
        statements
            .map { statement in
                statement.swift()
            }
            .joined(separator: "\n")
    }
}

protocol IntegerExprIrProtocol: IR {}

protocol ExpressionIrProtocol: IR {}

protocol StatementIrProtocol: IR {}
