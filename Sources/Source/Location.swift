//
//  Location.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct SourceLocation {
    let line: UInt
    let column: UInt
}

extension SourceLocation: CustomDebugStringConvertible {
    var debugDescription: String {
        "(\(line), \(column))"
    }
}
