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

    func line() -> UInt {
        y + 1
    }

    func column() -> UInt {
        x + 1
    }

    func right() -> SourceLocation {
        SourceLocation(index: index, x: x + 1, y: y)
    }
}

extension SourceLocation: Comparable {
    static func < (lhs: SourceLocation, rhs: SourceLocation) -> Bool {
        if lhs.y < rhs.y {
            return true
        }
        if lhs.y == rhs.y {
            return lhs.x < rhs.x
        }
        return false
    }
}

extension SourceLocation: CustomDebugStringConvertible {
    var debugDescription: String {
        let l = line()
        let c = column()
        return "(\(l):\(c))"
    }
}
