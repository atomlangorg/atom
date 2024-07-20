//
//  IR.swift
//  atom
//
//  Created by George Elsham on 20/07/2024.
//

protocol IR {
    func swift() -> String
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

struct AssignmentIr: IR {
    let variable: VariableIr
    let integer: IntegerIr

    func swift() -> String {
        "let \(variable.swift()) = \(integer.swift())"
    }
}
