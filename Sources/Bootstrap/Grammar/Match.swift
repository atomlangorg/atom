//
//  Match.swift
//  atom
//
//  Created by George Elsham on 29/09/2024.
//

enum Match {
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

    enum SpaceZeroOrMore: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: ()
            ),
            GrammarPattern(
                parts: (Literal.Space.self, SpaceZeroOrMore.self)
            )
        ]
    }

    enum SpaceOneOrMore: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.Space.self)
            ),
            GrammarPattern(
                parts: (Literal.Space.self, SpaceOneOrMore.self)
            )
        ]
    }

    enum Digit: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.Zero.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.One.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Two.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Three.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Four.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Five.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Six.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Seven.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Eight.self),
                gen: { d in d }
            ),
            GrammarPattern(
                parts: (Literal.Nine.self),
                gen: { d in d }
            )
        ]
    }

    enum Integer: GrammarMatch {
        typealias Output = IntegerIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.Three.self),
                gen: { _ in IntegerIr(value: 3) }
            )
        ]
    }

    enum LetKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseL.self, Literal.LowercaseE.self, Literal.LowercaseT.self)
            )
        ]
    }

    enum Variable: GrammarMatch {
        typealias Output = VariableIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseX.self),
                gen: { _ in VariableIr(name: "x") }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseY.self),
                gen: { _ in VariableIr(name: "y") }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseZ.self),
                gen: { _ in VariableIr(name: "z") }
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
                parts: (IntegerExpr.self, SpaceZeroOrMore.self, Literal.Plus.self, SpaceZeroOrMore.self, IntegerExpr.self),
                gen: { lhs, _, _, _, rhs in
                    let expr = IntegerAddExprIr(lhs: lhs, rhs: rhs)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .addition, associativity: .left)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, SpaceZeroOrMore.self, Literal.Asterisk.self, SpaceZeroOrMore.self, IntegerExpr.self),
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
                parts: (LetKeyword.self, SpaceOneOrMore.self, Variable.self, SpaceZeroOrMore.self, Literal.Equals.self, SpaceZeroOrMore.self, IntegerExpr.self),
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
