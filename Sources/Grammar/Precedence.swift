//
//  Precedence.swift
//  atom
//
//  Created by George Elsham on 03/09/2024.
//

struct Precedence {
    let priority: Priority
    let associativity: Associativity
}

extension Precedence {
    static func `default`() -> Precedence {
        Precedence(priority: .lowest, associativity: .left)
    }
}

extension Precedence {
    enum Priority: Comparable {
        case lowest
        case logicalOr
        case logicalAnd
        case add
        case multiply
        case negate
    }

    enum Associativity {
        case left
        case right
    }
}
