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

struct VariableIr: IR {
    let name: String

    func swift() -> String {
        name
    }
}

struct IntegerIr: IR {
    let value: Int

    func swift() -> String {
        "\(value)"
    }
}

struct AssignmentIr: IR, StatementIrProtocol {
    let variable: VariableIr
    let integer: IntegerIr

    func swift() -> String {
        "let \(variable.swift()) = \(integer.swift())"
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

protocol StatementIrProtocol: IR {}
