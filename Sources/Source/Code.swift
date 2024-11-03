//
//  Code.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

protocol Code: CustomStringConvertible {
    static var languageName: StaticString { get }

    var code: RawCode { get }

    init(_ code: RawCode)
}

extension Code {
    init(_ string: String) {
        self.init(RawCode(string))
    }

    var description: String {
        code.string
    }

    func formattedAsCodeBlock(_ preformatting: (RawCode) -> String = { $0.string }) -> String {
        let preformatted = preformatting(code)
        return "```\(Self.languageName)\n\(preformatted)\n```"
    }
}

struct AtomCode: Code {
    static let languageName: StaticString = "atom"

    let code: RawCode

    init(_ code: RawCode) {
        self.code = code
    }
}

struct SwiftCode: Code {
    static let languageName: StaticString = "swift"

    let code: RawCode

    init(_ code: RawCode) {
        self.code = code
    }
}
