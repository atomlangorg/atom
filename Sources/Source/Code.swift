//
//  Code.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

protocol Code {
    static var languageName: StaticString { get }

    var raw: RawCode { get }
    var isSource: Bool { get }

    init(_ raw: RawCode, isSource: Bool)
}

extension Code {
    init(_ string: String, isSource: Bool = false) {
        self.init(RawCode(string), isSource: isSource)
    }

    func formattedAsCodeBlock(_ preformatting: (RawCode, Int) -> String = { c, _ in c.string }) -> String {
        let formatted: String
        if isSource {
            var preformatted = ""
            let digitCount = String(raw.lineCount()).count
            for (lineNumber, lineContent) in zip(1..., raw.lines()) {
                let paddingCount = digitCount - String(lineNumber).count
                let padding = String(String(repeating: " ", count: paddingCount))
                preformatted.append("\(padding)\(lineNumber) | \(lineContent)\n")
            }
            _ = preformatted.popLast()
            formatted = preformatting(RawCode(preformatted), digitCount + 3)
        } else {
            formatted = preformatting(raw, 0)
        }
        return "```\(Self.languageName)\n\(formatted)\n```"
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
