//
//  RawCode.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

struct RawCode {
    typealias Index = String.Index

    let string: String

    init(_ string: String) {
        self.string = string
    }

    func sourceLocation(at index: Index) -> SourceLocation {
        var line: UInt = 0
        var column: UInt = 0

        for i in string.indices {
            if i == index {
                return SourceLocation(index: index, line: line, column: column)
            }

            let char = string[i]
            let isLineSep = char == "\n" || char == "\r\n"
            if isLineSep {
                line += 1
                column = 0
            } else {
                column += 1
            }
        }

        // One character past the end
        return SourceLocation(index: index, line: line, column: column)
    }
}

extension RawCode: CustomStringConvertible {
    var description: String {
        string
    }
}
