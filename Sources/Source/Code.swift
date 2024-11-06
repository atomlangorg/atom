//
//  Code.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

protocol Code: CustomStringConvertible {
    static var languageName: StaticString { get }

    var code: RawCode { get }
    var isSource: Bool { get }

    init(_ code: RawCode, isSource: Bool)
}

extension Code {
    init(_ string: String, isSource: Bool = false) {
        self.init(RawCode(string), isSource: isSource)
    }

    var description: String {
        code.string
    }

    func formattedAsCodeBlock(_ preformatting: (RawCode) -> String = { $0.string }) -> String {
        let formatted: String
        if isSource {
            var preformatted = ""
            for (lineNumber, lineContent) in zip(1..., code.lines()) {
                // TODO: fix for when line number has different number of characters
                preformatted.append("\(lineNumber) | \(lineContent)\n")
            }
            _ = preformatted.popLast()
            formatted = preformatted
        } else {
            formatted = preformatting(code)
        }
        return "```\(Self.languageName)\n\(formatted)\n```"
    }
}

struct AtomCode: Code {
    static let languageName: StaticString = "atom"

    let code: RawCode
    let isSource: Bool

    init(_ code: RawCode, isSource: Bool) {
        self.code = code
        self.isSource = isSource
    }
}

struct SwiftCode: Code {
    static let languageName: StaticString = "swift"

    let code: RawCode
    let isSource: Bool

    init(_ code: RawCode, isSource: Bool) {
        self.code = code
        self.isSource = isSource
    }
}
