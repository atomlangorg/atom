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
            ),
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
            ),
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
            ),
        ]
    }

    enum SpaceOrLineSeparator: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.Space.self)
            ),
            GrammarPattern(
                parts: (LineSeparator.self)
            ),
        ]
    }

    enum SpaceWithPossibleLineSeparator: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: ()
            ),
            GrammarPattern(
                parts: (SpaceOrLineSeparator.self, SpaceWithPossibleLineSeparator.self)
            ),
        ]
    }

    enum SpaceWithDefiniteLineSeparator: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, LineSeparator.self, SpaceWithPossibleLineSeparator.self)
            ),
        ]
    }

    enum SymbolOpenRoundBracket: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceWithPossibleLineSeparator.self, Literal.OpenRoundBracket.self, SpaceWithPossibleLineSeparator.self)
            ),
        ]
    }

    enum SymbolCloseRoundBracket: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceWithPossibleLineSeparator.self, Literal.CloseRoundBracket.self, SpaceWithPossibleLineSeparator.self)
            ),
        ]
    }

    enum SymbolOpenCurlyBracket: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceWithPossibleLineSeparator.self, Literal.OpenCurlyBracket.self, SpaceWithPossibleLineSeparator.self)
            ),
        ]
    }

    enum SymbolCloseCurlyBracket: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceWithPossibleLineSeparator.self, Literal.CloseCurlyBracket.self, SpaceWithPossibleLineSeparator.self)
            ),
        ]
    }

    enum OperatorAdd: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Plus.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorSubtract: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Minus.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorMultiply: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Asterisk.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorDivide: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Slash.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorAnd: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Ampersand.self, Literal.Ampersand.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorOr: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.VerticalBar.self, Literal.VerticalBar.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorAssign: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Equals.self, SpaceZeroOrMore.self)
            ),
        ]
    }

    enum OperatorEqual: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceZeroOrMore.self, Literal.Equals.self, Literal.Equals.self, SpaceZeroOrMore.self)
            ),
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
            ),
        ]
    }

    enum IntegerString: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Digit.self),
                gen: { digit in
                    digit
                }
            ),
            GrammarPattern(
                parts: (Digit.self, IntegerString.self),
                gen: { digit, rest in
                    RawStringIr(string: digit.string + rest.string)
                }
            ),
        ]
    }

    enum Integer: GrammarMatch {
        typealias Output = IntegerIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (IntegerString.self),
                gen: { integer in
                    IntegerIr(value: Int(integer.string)!)
                }
            ),
        ]
    }

    enum UppercaseLetter: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.UppercaseA.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseB.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseC.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseD.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseE.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseF.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseG.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseH.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseI.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseJ.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseK.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseL.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseM.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseN.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseO.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseP.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseQ.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseR.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseS.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseT.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseU.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseV.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseW.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseX.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseY.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.UppercaseZ.self),
                gen: { l in l }
            ),
        ]
    }

    enum LowercaseLetter: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseA.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseB.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseC.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseD.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseE.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseF.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseG.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseH.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseI.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseJ.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseK.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseL.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseM.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseN.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseO.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseP.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseQ.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseR.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseS.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseT.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseU.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseV.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseW.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseX.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseY.self),
                gen: { l in l }
            ),
            GrammarPattern(
                parts: (Literal.LowercaseZ.self),
                gen: { l in l }
            ),
        ]
    }

    enum LetKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseL.self, Literal.LowercaseE.self, Literal.LowercaseT.self)
            ),
        ]
    }

    enum VarKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseV.self, Literal.LowercaseA.self, Literal.LowercaseR.self)
            ),
        ]
    }

    enum StructKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseS.self, Literal.LowercaseT.self, Literal.LowercaseR.self, Literal.LowercaseU.self, Literal.LowercaseC.self, Literal.LowercaseT.self)
            ),
        ]
    }

    enum FalseKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseF.self, Literal.LowercaseA.self, Literal.LowercaseL.self, Literal.LowercaseS.self, Literal.LowercaseE.self)
            ),
        ]
    }

    enum TrueKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseT.self, Literal.LowercaseR.self, Literal.LowercaseU.self, Literal.LowercaseE.self)
            ),
        ]
    }

    enum GrammarLiteralKeyword: GrammarMatch {
        typealias Output = NeverIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.LowercaseG.self, Literal.LowercaseR.self, Literal.LowercaseA.self, Literal.LowercaseM.self, Literal.LowercaseM.self, Literal.LowercaseA.self, Literal.LowercaseR.self, Literal.LowercaseL.self, Literal.LowercaseI.self, Literal.LowercaseT.self, Literal.LowercaseE.self, Literal.LowercaseR.self, Literal.LowercaseA.self, Literal.LowercaseL.self)
            ),
        ]
    }

    enum Identifier: GrammarMatch {
        typealias Output = IdentifierIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (IdentifierChar.self),
                gen: { c in
                    IdentifierIr(name: c.string)
                }
            ),
            GrammarPattern(
                parts: (IdentifierChar.self, Identifier.self),
                gen: { c, rest in
                    IdentifierIr(name: c.string + rest.name)
                }
            ),
        ]
    }

    enum IdentifierChar: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (LowercaseLetter.self),
                gen: { c in c }
            ),
            GrammarPattern(
                parts: (UppercaseLetter.self),
                gen: { c in c }
            ),
            GrammarPattern(
                parts: (Literal.Underscore.self),
                gen: { c in c }
            ),
        ]
    }

    enum IntegerExpr: GrammarMatch {
        typealias Output = IntegerExprIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Integer.self),
                gen: { integer in
                    IntegerExprIr(expression: integer)
                }
            ),
            GrammarPattern(
                parts: (SymbolOpenRoundBracket.self, IntegerExpr.self, SymbolCloseRoundBracket.self),
                gen: { _, expr, _ in
                    expr
                },
                options: [.resetPrecedence]
            ),
            GrammarPattern(
                parts: (Literal.Minus.self, IntegerExpr.self),
                gen: { _, integer in
                    let expr = IntegerNegateExprIr(expr: integer)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .negate, associativity: .right)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, OperatorAdd.self, IntegerExpr.self),
                gen: { lhs, _, rhs in
                    let expr = IntegerAddExprIr(lhs: lhs, rhs: rhs)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .add, associativity: .left)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, OperatorSubtract.self, IntegerExpr.self),
                gen: { lhs, _, rhs in
                    let expr = IntegerSubtractExprIr(lhs: lhs, rhs: rhs)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .add, associativity: .left)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, OperatorMultiply.self, IntegerExpr.self),
                gen: { lhs, _, rhs in
                    let expr = IntegerMultiplyExprIr(lhs: lhs, rhs: rhs)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .multiply, associativity: .left)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, OperatorDivide.self, IntegerExpr.self),
                gen: { lhs, _, rhs in
                    let expr = IntegerDivideExprIr(lhs: lhs, rhs: rhs)
                    return IntegerExprIr(expression: expr)
                },
                precedence: Precedence(priority: .multiply, associativity: .left)
            ),
        ]
    }

    enum StringChar: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.Wildcard.self),
                gen: { char in
                    char
                }
            ),
            GrammarPattern(
                parts: (LineSeparator.self),
                gen: { _ in
                    RawStringIr(string: "\n")
                }
            ),
            GrammarPattern(
                parts: (Literal.Backslash.self, LineSeparator.self),
                gen: { _, _ in
                    RawStringIr(string: "")
                }
            ),
            GrammarPattern(
                parts: (Literal.Backslash.self, Literal.Backslash.self),
                gen: { _, _ in
                    RawStringIr(string: "\\")
                }
            ),
            GrammarPattern(
                parts: (Literal.Backslash.self, Literal.LowercaseN.self),
                gen: { _, _ in
                    RawStringIr(string: "\n")
                }
            ),
            GrammarPattern(
                parts: (Literal.Backslash.self, Literal.Wildcard.self),
                gen: { _, char throws(GrammarError) in
                    throw GrammarError("invalid escape sequence '\\\(char.string)'")
                }
            ),
        ]
    }

    enum StringAfterStart: GrammarMatch {
        typealias Output = RawStringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.DoubleQuote.self),
                gen: { _ in
                    RawStringIr(string: "")
                }
            ),
            GrammarPattern(
                parts: (StringChar.self, StringAfterStart.self),
                gen: { char, rest in
                    RawStringIr(string: char.string + rest.string)
                }
            ),
        ]
    }

    enum String: GrammarMatch {
        typealias Output = StringIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Literal.DoubleQuote.self, StringAfterStart.self),
                gen: { _, string in
                    StringIr(string: string.string)
                }
            ),
        ]
    }

    enum Boolean: GrammarMatch {
        typealias Output = BooleanIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (FalseKeyword.self),
                gen: { _ in
                    BooleanIr(boolean: false)
                }
            ),
            GrammarPattern(
                parts: (TrueKeyword.self),
                gen: { _ in
                    BooleanIr(boolean: true)
                }
            ),
        ]
    }

    enum BooleanExpr: GrammarMatch {
        typealias Output = BooleanExprIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (Boolean.self),
                gen: { bool in
                    BooleanExprIr(expression: bool)
                }
            ),
            GrammarPattern(
                parts: (SymbolOpenRoundBracket.self, BooleanExpr.self, SymbolCloseRoundBracket.self),
                gen: { _, expr, _ in
                    expr
                },
                options: [.resetPrecedence]
            ),
            GrammarPattern(
                parts: (Literal.ExclamationMark.self, BooleanExpr.self),
                gen: { _, bool in
                    let expr = BooleanNotExprIr(expr: bool)
                    return BooleanExprIr(expression: expr)
                },
                precedence: Precedence(priority: .negate, associativity: .right)
            ),
            GrammarPattern(
                parts: (BooleanExpr.self, OperatorAnd.self, BooleanExpr.self),
                gen: { lhs, _, rhs in
                    let expr = BooleanAndExprIr(lhs: lhs, rhs: rhs)
                    return BooleanExprIr(expression: expr)
                },
                precedence: Precedence(priority: .logicalAnd, associativity: .left)
            ),
            GrammarPattern(
                parts: (BooleanExpr.self, OperatorOr.self, BooleanExpr.self),
                gen: { lhs, _, rhs in
                    let expr = BooleanOrExprIr(lhs: lhs, rhs: rhs)
                    return BooleanExprIr(expression: expr)
                },
                precedence: Precedence(priority: .logicalOr, associativity: .left)
            ),
            GrammarPattern(
                parts: (IntegerExpr.self, OperatorEqual.self, IntegerExpr.self),
                gen: { lhs, _, rhs in
                    let expr = BooleanIntegerEqualExprIr(lhs: lhs, rhs: rhs)
                    return BooleanExprIr(expression: expr)
                },
                precedence: Precedence(priority: .compare, associativity: .left)
            ),
            GrammarPattern(
                parts: (String.self, OperatorEqual.self, String.self),
                gen: { lhs, _, rhs in
                    let expr = BooleanStringEqualExprIr(lhs: lhs, rhs: rhs)
                    return BooleanExprIr(expression: expr)
                },
                precedence: Precedence(priority: .compare, associativity: .left)
            ),
            GrammarPattern(
                parts: (BooleanExpr.self, OperatorEqual.self, BooleanExpr.self),
                gen: { lhs, _, rhs in
                    let expr = BooleanBooleanEqualExprIr(lhs: lhs, rhs: rhs)
                    return BooleanExprIr(expression: expr)
                },
                precedence: Precedence(priority: .compare, associativity: .left)
            ),
        ]
    }

    enum Expression: GrammarMatch {
        typealias Output = ExpressionIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (IntegerExpr.self),
                gen: { expr in
                    ExpressionIr(expression: expr)
                }
            ),
            GrammarPattern(
                parts: (String.self),
                gen: { expr in
                    ExpressionIr(expression: expr)
                }
            ),
            GrammarPattern(
                parts: (BooleanExpr.self),
                gen: { expr in
                    ExpressionIr(expression: expr)
                }
            ),
        ]
    }

    enum Assignment: GrammarMatch {
        typealias Output = AssignmentIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (LetKeyword.self, SpaceOneOrMore.self, Identifier.self, OperatorAssign.self, Expression.self),
                gen: { _, _, variable, _, expr in
                    AssignmentIr(variable: variable, expression: expr)
                }
            ),
        ]
    }

    enum StructField: GrammarMatch {
        typealias Output = StructFieldIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (LetKeyword.self, SpaceOneOrMore.self, Identifier.self, SpaceZeroOrMore.self, Literal.Colon.self, SpaceZeroOrMore.self, Identifier.self),
                gen: { _, _, identifier, _, _, _, type in
                    StructFieldIr(identifier: identifier, type: type, isMutable: false)
                }
            ),
            GrammarPattern(
                parts: (VarKeyword.self, SpaceOneOrMore.self, Identifier.self, SpaceZeroOrMore.self, Literal.Colon.self, SpaceZeroOrMore.self, Identifier.self),
                gen: { _, _, identifier, _, _, _, type in
                    StructFieldIr(identifier: identifier, type: type, isMutable: true)
                }
            ),
        ]
    }

    enum StructFields: GrammarMatch {
        typealias Output = StructFieldsIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (StructField.self, SpaceWithDefiniteLineSeparator.self, StructFields.self),
                gen: { field, _, rest in
                    StructFieldsIr(fields: CollectionOfOne(field) + rest.fields)
                }
            ),
            GrammarPattern(
                parts: (StructField.self),
                gen: { field in
                    StructFieldsIr(fields: [field])
                }
            ),
            GrammarPattern(
                parts: (),
                gen: {
                    StructFieldsIr(fields: [])
                }
            ),
        ]
    }

    enum Struct: GrammarMatch {
        typealias Output = StructIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (StructKeyword.self, SpaceOneOrMore.self, Identifier.self, SymbolOpenCurlyBracket.self, StructFields.self, SymbolCloseCurlyBracket.self),
                gen: { _, _, identifier, _, fields, _ in
                    StructIr(identifier: identifier, fields: fields)
                }
            ),
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
            ),
            GrammarPattern(
                parts: (Struct.self),
                gen: { `struct` in
                    StatementIr(ir: `struct`)
                }
            ),
        ]
    }

    enum Program: GrammarMatch {
        typealias Output = ProgramIr

        static let patterns: [any GrammarPatternProtocol<Output>] = [
            GrammarPattern(
                parts: (SpaceWithPossibleLineSeparator.self, Program.self),
                gen: { _, program in
                    program
                }
            ),
            GrammarPattern(
                parts: (Statement.self, SpaceWithDefiniteLineSeparator.self, Program.self),
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
            ),
        ]
    }
}
