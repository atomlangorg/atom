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

    enum Three: GrammarLiteral {
        static let literal: Character = "3"
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
