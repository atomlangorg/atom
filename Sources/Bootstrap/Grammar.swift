//
//  Grammar.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

enum Whitespace: GrammarLiteral {
    static let literal: Character = " "
}

enum WhitespaceZeroOrMore: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: []
        ),
        GrammarPattern(
            parts: [.literal(Whitespace.self), .match(WhitespaceZeroOrMore.self)]
        )
    ]
}

enum WhitespaceOneOrMore: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.literal(Whitespace.self)]
        ),
        GrammarPattern(
            parts: [.literal(Whitespace.self), .match(WhitespaceOneOrMore.self)]
        )
    ]
}

enum LineSeparator: GrammarLiteral {
    static let literal: Character = "\n"
}

enum CharL: GrammarLiteral {
    static let literal: Character = "l"
}

enum CharE: GrammarLiteral {
    static let literal: Character = "e"
}

enum CharT: GrammarLiteral {
    static let literal: Character = "t"
}

enum LetKeyword: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.literal(CharL.self), .literal(CharE.self), .literal(CharT.self)]
        )
    ]
}

enum CharEq: GrammarLiteral {
    static let literal: Character = "="
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

enum Variable: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.literal(CharX.self)],
            swift: { _ in "x" }
        ),
        GrammarPattern(
            parts: [.literal(CharY.self)],
            swift: { _ in "y" }
        ),
        GrammarPattern(
            parts: [.literal(CharZ.self)],
            swift: { _ in "z" }
        )
    ]
}

enum CharThree: GrammarLiteral {
    static let literal: Character = "3"
}

enum Assignment: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.match(LetKeyword.self), .match(WhitespaceOneOrMore.self), .match(Variable.self), .match(WhitespaceZeroOrMore.self), .literal(CharEq.self), .match(WhitespaceZeroOrMore.self), .literal(CharThree.self)],
            swift: { parts in
                "let \(parts[2]!) = \(parts[6]!)"
            }
        )
    ]
}
