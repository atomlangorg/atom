//
//  Literal.swift
//  atom
//
//  Created by George Elsham on 29/09/2024.
//

enum Literal {
    enum LineFeed: GrammarLiteral {
        static let literal: Character = "\n"
    }

    enum CarriageReturnLineFeed: GrammarLiteral {
        static let literal: Character = "\r\n"
    }

    enum Space: GrammarLiteral {
        static let literal: Character = " "
    }

    enum Asterisk: GrammarLiteral {
        static let literal: Character = "*"
    }

    enum Plus: GrammarLiteral {
        static let literal: Character = "+"
    }

    enum Zero: GrammarLiteral {
        static let literal: Character = "0"
    }

    enum One: GrammarLiteral {
        static let literal: Character = "1"
    }

    enum Two: GrammarLiteral {
        static let literal: Character = "2"
    }

    enum Three: GrammarLiteral {
        static let literal: Character = "3"
    }

    enum Four: GrammarLiteral {
        static let literal: Character = "4"
    }

    enum Five: GrammarLiteral {
        static let literal: Character = "5"
    }

    enum Six: GrammarLiteral {
        static let literal: Character = "6"
    }

    enum Seven: GrammarLiteral {
        static let literal: Character = "7"
    }

    enum Eight: GrammarLiteral {
        static let literal: Character = "8"
    }

    enum Nine: GrammarLiteral {
        static let literal: Character = "9"
    }

    enum Equals: GrammarLiteral {
        static let literal: Character = "="
    }

    enum LowercaseE: GrammarLiteral {
        static let literal: Character = "e"
    }

    enum LowercaseL: GrammarLiteral {
        static let literal: Character = "l"
    }

    enum LowercaseT: GrammarLiteral {
        static let literal: Character = "t"
    }

    enum LowercaseX: GrammarLiteral {
        static let literal: Character = "x"
    }

    enum LowercaseY: GrammarLiteral {
        static let literal: Character = "y"
    }

    enum LowercaseZ: GrammarLiteral {
        static let literal: Character = "z"
    }
}
