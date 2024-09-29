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

    static let patterns: [any GrammarPatternProtocol<Output>] = [
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

    static let patterns: [any GrammarPatternProtocol<Output>] = [
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

    static let patterns: [any GrammarPatternProtocol<Output>] = [
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

    static let patterns: [any GrammarPatternProtocol<Output>] = [
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

    static let patterns: [any GrammarPatternProtocol<Output>] = [
        GrammarPattern(
            parts: (CharThree.self),
            gen: { _ in IntegerIr(value: 3) }
        )
    ]
}

enum CharPlus: GrammarLiteral {
    static let literal: Character = "+"
}

enum CharMultiply: GrammarLiteral {
    static let literal: Character = "*"
}

struct IntegerExpr: GrammarMatch {
    typealias Output = IntegerExprIr

    static let patterns: [any GrammarPatternProtocol<Output>] = [
        GrammarPattern(
            parts: (Integer.self),
            gen: { integer in
                IntegerExprIr(expression: integer)
            }
        ),
        GrammarPattern(
            parts: (IntegerExpr.self, WhitespaceZeroOrMore.self, CharPlus.self, WhitespaceZeroOrMore.self, IntegerExpr.self),
            gen: { lhs, _, _, _, rhs in
                let expr = IntegerAddExprIr(lhs: lhs, rhs: rhs)
                return IntegerExprIr(expression: expr)
            },
            precedence: Precedence(priority: .addition, associativity: .left)
        ),
        GrammarPattern(
            parts: (IntegerExpr.self, WhitespaceZeroOrMore.self, CharMultiply.self, WhitespaceZeroOrMore.self, IntegerExpr.self),
            gen: { lhs, _, _, _, rhs in
                let expr = IntegerMultiplyExprIr(lhs: lhs, rhs: rhs)
                return IntegerExprIr(expression: expr)
            },
            precedence: Precedence(priority: .multiplication, associativity: .left)
        )
    ]
}

enum Assignment: GrammarMatch {
    typealias Output = AssignmentIr

    static let patterns: [any GrammarPatternProtocol<Output>] = [
        GrammarPattern(
            parts: (LetKeyword.self, WhitespaceOneOrMore.self, Variable.self, WhitespaceZeroOrMore.self, CharEq.self, WhitespaceZeroOrMore.self, IntegerExpr.self),
            gen: { _, _, variable, _, _, _, expr in
                AssignmentIr(variable: variable, expression: expr)
            }
        )
    ]
}

enum Statement: GrammarMatch {
    typealias Output = StatementIr

    static let patterns: [any GrammarPatternProtocol<Output>] = [
        GrammarPattern(
            parts: (Assignment.self),
            gen: { assignment in
                StatementIr(ir: assignment)
            }
        )
    ]
}

enum Program: GrammarMatch {
    typealias Output = ProgramIr

    static let patterns: [any GrammarPatternProtocol<Output>] = [
        GrammarPattern(
            parts: (LineSeparator.self, Program.self),
            gen: { _, program in
                program
            }
        ),
        GrammarPattern(
            parts: (Statement.self, LineSeparator.self, Program.self),
            gen: { statement, _, program in
                ProgramIr(statements: CollectionOfOne(statement) + program.statements)
            }
        ),
        GrammarPattern(
            parts: (Statement.self),
            gen: { statement in
                ProgramIr(statements: [statement])
            }
        ),
        GrammarPattern(
            parts: (),
            gen: {
                ProgramIr(statements: [])
            }
        )
    ]
}
