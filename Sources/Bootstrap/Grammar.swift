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
    typealias Output = NeverIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: ()
        ),
        GrammarPattern(
            parts: (Whitespace.self, WhitespaceZeroOrMore.self)
        )
    ]
}

enum WhitespaceOneOrMore: GrammarMatch {
    typealias Output = NeverIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (Whitespace.self)
        ),
        GrammarPattern(
            parts: (Whitespace.self, WhitespaceOneOrMore.self)
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
    typealias Output = NeverIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (CharL.self, CharE.self, CharT.self)
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
    typealias Output = VariableIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (CharX.self),
            gen: { _ in VariableIr(name: "x") }
        ),
        GrammarPattern(
            parts: (CharY.self),
            gen: { _ in VariableIr(name: "y") }
        ),
        GrammarPattern(
            parts: (CharZ.self),
            gen: { _ in VariableIr(name: "z") }
        )
    ]
}

enum CharThree: GrammarLiteral {
    static let literal: Character = "3"
}

enum Integer: GrammarMatch {
    typealias Output = IntegerIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (CharThree.self),
            gen: { _ in IntegerIr(value: 3) }
        )
    ]
}

enum CharPlus: GrammarLiteral {
    static let literal: Character = "+"
}

enum IntegerAddExpr: GrammarMatch {
    typealias Output = IntegerIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (Integer.self),
            gen: { integer in
                integer
            }
        ),
        GrammarPattern(
            parts: (Integer.self, IntegerAddPartialExpr.self),
            gen: { (integer, expr) in
                IntegerIr(value: integer.value + expr.value)
            }
        )
    ]
}

enum IntegerAddPartialExpr: GrammarMatch {
    typealias Output = IntegerIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (WhitespaceZeroOrMore.self, CharPlus.self, WhitespaceZeroOrMore.self, IntegerAddExpr.self),
            gen: { _, _, _, expr in
                expr
            }
        )
    ]
}

enum Assignment: GrammarMatch {
    typealias Output = AssignmentIr

    static let patterns: [any GrammarPatternProtocol<_>] = [
        GrammarPattern(
            parts: (LetKeyword.self, WhitespaceOneOrMore.self, Variable.self, WhitespaceZeroOrMore.self, CharEq.self, WhitespaceZeroOrMore.self, IntegerAddExpr.self),
            gen: { _, _, variable, _, _, _, integer in
                AssignmentIr(variable: variable, integer: integer)
            }
        )
    ]
}
