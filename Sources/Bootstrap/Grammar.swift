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
            gen: { _ in VariableIr(name: "x") }
        ),
        GrammarPattern(
            parts: [.literal(CharY.self)],
            gen: { _ in VariableIr(name: "y") }
        ),
        GrammarPattern(
            parts: [.literal(CharZ.self)],
            gen: { _ in VariableIr(name: "z") }
        )
    ]
}

enum CharThree: GrammarLiteral {
    static let literal: Character = "3"
}

enum Integer: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.literal(CharThree.self)],
            gen: { _ in IntegerIr(value: 3) }
        )
    ]
}

enum CharPlus: GrammarLiteral {
    static let literal: Character = "+"
}

enum IntegerAddExpr: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.match(Integer.self)],
            gen: { irs in
                irs[0]!
            }
        ),
        GrammarPattern(
            parts: [.match(Integer.self), .match(IntegerAddPartialExpr.self)],
            gen: { irs in
                IntegerIr(value: (irs[0]! as! IntegerIr).value + (irs[1]! as! IntegerIr).value)
            }
        )
    ]
}

enum IntegerAddPartialExpr: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.match(WhitespaceZeroOrMore.self), .literal(CharPlus.self), .match(WhitespaceZeroOrMore.self), .match(IntegerAddExpr.self)],
            gen: { irs in
                irs[3]!
            }
        )
    ]
}

enum Assignment: GrammarMatch {
    static let patterns = [
        GrammarPattern(
            parts: [.match(LetKeyword.self), .match(WhitespaceOneOrMore.self), .match(Variable.self), .match(WhitespaceZeroOrMore.self), .literal(CharEq.self), .match(WhitespaceZeroOrMore.self), .match(IntegerAddExpr.self)],
            gen: { irs in
                AssignmentIr(variable: irs[2]! as! VariableIr, integer: irs[6]! as! IntegerIr)
            }
        )
    ]
}
