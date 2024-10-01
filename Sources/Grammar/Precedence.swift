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
    enum Priority: Comparable {
        case lowest
        case add
        case multiply
        case negate
    }

    enum Associativity {
        case left
        case right
    }
}
