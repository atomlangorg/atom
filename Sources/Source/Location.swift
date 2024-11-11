//
//  Location.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct SourceLocation {
    let index: RawCode.Index
    let x: UInt
    let y: UInt

    var line: UInt {
        y + 1
    }

    var column: UInt {
        x + 1
    }

    func right() -> SourceLocation {
        SourceLocation(index: index, x: x + 1, y: y)
    }
}

extension SourceLocation: CustomDebugStringConvertible {
    var debugDescription: String {
        "(\(line):\(column))"
    }
}
