//
//  Code.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

protocol Code: CustomStringConvertible {
    static var languageName: StaticString { get }

    var raw: RawCode { get }

    init(_ string: String)
}

extension Code {
    var description: String {
        raw.string
    }

    func formattedAsCodeBlock(preformatting: (inout ModifiedRawCode) -> Void = { _ in }, postformatting: (inout ModifiedRawCode) -> Void = { _ in }) -> String {
        var formatted = ModifiedRawCode(base: raw)
        preformatting(&formatted)

        if raw.isSource {
            // Line number preparation
            let digitCount = String(raw.lineCount()).count
            func preLineText(lineNumber: UInt) -> String {
                let paddingCount = digitCount - String(lineNumber).count
                let padding = String(String(repeating: " ", count: paddingCount))
                return "\(padding)\(lineNumber) \(PrettyPrint.lineNumbersBorder) "
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
                    let space = String(repeating: " ", count: digitCount)
                    insertion.string = insertion.string.replacingOccurrences(of: "\n", with: "\n\(space)   ")
                }
            })
        }

        postformatting(&formatted)
        let formattedStr = formatted.string()
        return "```\(Self.languageName)\n\(formattedStr)\n```"
    }

    func unformatted() -> String {
        raw.string
    }
}

protocol CodeFromIr where Self: Code {
    static func fromIr(_ ir: some IR) -> Self
}

struct AtomCode: Code {
    static let languageName: StaticString = "atom"

    let raw: RawCode

    init(_ string: String) {
        raw = RawCode(string, isSource: true)
    }
}

struct SwiftCode: Code, CodeFromIr {
    static let languageName: StaticString = "swift"

    let raw: RawCode

    init(_ string: String) {
        raw = RawCode(string, isSource: false)
    }

    static func fromIr(_ ir: some IR) -> SwiftCode {
        ir.swift()
    }
}
