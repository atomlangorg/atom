//
//  Code.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

protocol Code: CustomStringConvertible {
    static var languageName: StaticString { get }

    var code: String { get }

    init(_ code: String)
}

extension Code {
    var description: String {
        code
    }

    func formattedAsCodeBlock() -> String {
        "```\(Self.languageName)\n\(code)\n```"
    }
}

struct AtomCode: Code {
    static let languageName: StaticString = "atom"

    let code: String

    init(_ code: String) {
        self.code = code
    }
}

struct SwiftCode: Code {
    static let languageName: StaticString = "swift"

    let code: String

    init(_ code: String) {
        self.code = code
    }
}
