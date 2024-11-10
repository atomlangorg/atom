//
//  Code.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

protocol Code: CustomStringConvertible {
    static var languageName: StaticString { get }

    var raw: RawCode { get }
    var isSource: Bool { get }

    init(_ raw: RawCode, isSource: Bool)
}

extension Code {
    init(_ string: String, isSource: Bool = false) {
        self.init(RawCode(string), isSource: isSource)
    }

    var description: String {
        raw.string
    }

    func formattedAsCodeBlock(_ preformatting: (inout ModifiedRawCode) -> Void = { _ in }) -> String {
        var formatted = ModifiedRawCode(base: raw)
        preformatting(&formatted)

        if isSource {
            // Line number preparation
            let digitCount = String(raw.lineCount()).count
            func preLineText(lineNumber: UInt) -> String {
                let paddingCount = digitCount - String(lineNumber).count
                let padding = String(String(repeating: " ", count: paddingCount))
                return "\(padding)\(lineNumber) | "
            }

            // Insert line numbers
            let initial = preLineText(lineNumber: 1)
            formatted.insert(initial, at: raw.string.startIndex)
            for (lineNumber, separatorIndex) in zip((2 as UInt)..., raw.lineSeparators) {
                let nextIndex = raw.string.index(after: separatorIndex)
                let text = preLineText(lineNumber: lineNumber)
                formatted.insert(text, at: nextIndex)
            }

            // Indent on new modified lines
            formatted.modify(each: { insertion in
                if insertion.string.contains("\n") {
                    insertion.string = insertion.string.replacingOccurrences(of: "\n", with: "\n    ")
                }
            })
        }

        let formattedStr = formatted.string()
        return "```\(Self.languageName)\n\(formattedStr)\n```"
    }

    func unformatted() -> String {
        raw.string
    }
}

struct AtomCode: Code {
    static let languageName: StaticString = "atom"

    let raw: RawCode
    let isSource: Bool

    init(_ raw: RawCode, isSource: Bool) {
        self.raw = raw
        self.isSource = isSource
    }
}

struct SwiftCode: Code {
    static let languageName: StaticString = "swift"

    let raw: RawCode
    let isSource: Bool

    init(_ raw: RawCode, isSource: Bool) {
        self.raw = raw
        self.isSource = isSource
    }
}
