//
//  Literal.swift
//  atom
//
//  Created by George Elsham on 29/09/2024.
//

enum LineFeed: GrammarLiteral {
    static let literal: Character = "\n"
}

enum CarriageReturnLineFeed: GrammarLiteral {
    static let literal: Character = "\r\n"
}

enum Whitespace: GrammarLiteral {
    static let literal: Character = " "
}

enum CharMultiply: GrammarLiteral {
    static let literal: Character = "*"
}

enum CharPlus: GrammarLiteral {
    static let literal: Character = "+"
}

enum CharThree: GrammarLiteral {
    static let literal: Character = "3"
}

enum CharEq: GrammarLiteral {
    static let literal: Character = "="
}

enum CharE: GrammarLiteral {
    static let literal: Character = "e"
}

enum CharL: GrammarLiteral {
    static let literal: Character = "l"
}

enum CharT: GrammarLiteral {
    static let literal: Character = "t"
}

enum CharX: GrammarLiteral {
    static let literal: Character = "x"
}

enum CharY: GrammarLiteral {
    static let literal: Character = "y"
}

enum CharZ: GrammarLiteral {
    static let literal: Character = "z"
}
