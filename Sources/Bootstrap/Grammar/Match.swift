//
//  Match.swift
//  atom
//
//  Created by George Elsham on 29/09/2024.
//

enum Match {
    enum WhitespaceZeroOrMore: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: ()
            ),
            GrammarPattern(
                parts: (Literal.Space.self, WhitespaceZeroOrMore.self)
            )
        ]
    }

    enum WhitespaceOneOrMore: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.Space.self)
            ),
            GrammarPattern(
                parts: (Literal.Space.self, WhitespaceOneOrMore.self)
            )
        ]
    }

    enum LineSeparator: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LineFeed.self)
            ),
            GrammarPattern(
                parts: (Literal.CarriageReturnLineFeed.self)
            )
        ]
    }

    enum LetKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.CharL.self, Literal.CharE.self, Literal.CharT.self)
            )
        ]
    }

    enum Variable: GrammarMatch {
        typealias Output = VariableIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.CharX.self),
                gen: { _ in VariableIr(name: "x") }
            ),
            GrammarPattern(
                parts: (Literal.CharY.self),
                gen: { _ in VariableIr(name: "y") }
            ),
            GrammarPattern(
                parts: (Literal.CharZ.self),
                gen: { _ in VariableIr(name: "z") }
            )
        ]
    }

    enum Integer: GrammarMatch {
        typealias Output = IntegerIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.CharThree.self),
                gen: { _ in IntegerIr(value: 3) }
            )
        ]
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
                parts: (IntegerExpr.self, WhitespaceZeroOrMore.self, Literal.CharPlus.self, WhitespaceZeroOrMore.self, IntegerExpr.self),
                gen: { lhs, _, _, _, rhs in
                    let expr = IntegerAddExprIr(lhs: lhs, rhs: rhs)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .addition, associativity: .left)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, WhitespaceZeroOrMore.self, Literal.CharMultiply.self, WhitespaceZeroOrMore.self, IntegerExpr.self),
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
                parts: (LetKeyword.self, WhitespaceOneOrMore.self, Variable.self, WhitespaceZeroOrMore.self, Literal.CharEq.self, WhitespaceZeroOrMore.self, IntegerExpr.self),
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
}
