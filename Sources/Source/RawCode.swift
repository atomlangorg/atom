//
//  RawCode.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

struct RawCode {
    typealias Index = String.Index

    let string: String
    private let lineSeparators: [Index]

    init(_ string: String) {
        self.string = string
        lineSeparators = Self.calculateLineSeparatorIndexes(string: string)
    }

    func sourceLocation(at index: Index) -> SourceLocation {
        // Calculate y
        var y: UInt = 0
        var lastLineSepIndex: Index?
        for lineSepIndex in lineSeparators {
            if lineSepIndex > index {
                break
            }
            lastLineSepIndex = lineSepIndex
            y += 1
        }

        // Calculate x
        let lineStartIndex: Index
        if let lastLineSepIndex {
            lineStartIndex = string.index(after: lastLineSepIndex)
        } else {
            lineStartIndex = string.startIndex
        }
        let x = countChars(in: lineStartIndex ..< index)

        // Return location
        return SourceLocation(index: index, x: x, y: y)
    }

    private func countChars(in range: Range<Index>) -> UInt {
        var count: UInt = 0

        var index = range.lowerBound
        while index < range.upperBound {
            string.formIndex(after: &index)
            count += 1
        }

        return count
    }

    private static func calculateLineSeparatorIndexes(string: String) -> [Index] {
        var indexes = [Index]()

        var index = string.startIndex
        while index < string.endIndex {
            if Self.isLineSeparator(string[index]) {
                indexes.append(index)
            }
            string.formIndex(after: &index)
        }

        return indexes
    }

    private static func isLineSeparator(_ char: Character) -> Bool {
        char == "\n" || char == "\r\n"
    }
}
