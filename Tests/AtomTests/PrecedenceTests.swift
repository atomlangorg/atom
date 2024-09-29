//
//  PrecedenceTests.swift
//  atom
//
//  Created by George Elsham on 29/09/2024.
//

import Testing
@testable import atom

@Suite("Precedence")
struct PrecedenceTests {
    @Test("Check precedence of one addition")
    func oneAddition() {
        let input = "3 + 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "(3 + 3)")
    }

    @Test("Check precedence of two additions")
    func twoAdditions() {
        let input = "3 + 3 + 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "((3 + 3) + 3)")
    }

    @Test("Check precedence of three additions")
    func threeAdditions() {
        let input = "3 + 3 + 3 + 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "(((3 + 3) + 3) + 3)")
    }

    @Test("Check precedence of one multiplication")
    func oneMultiplication() {
        let input = "3 * 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "(3 * 3)")
    }

    @Test("Check precedence of two multiplications")
    func twoMultiplications() {
        let input = "3 * 3 * 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "((3 * 3) * 3)")
    }

    @Test("Check precedence of three multiplications")
    func threeMultiplications() {
        let input = "3 * 3 * 3 * 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "(((3 * 3) * 3) * 3)")
    }

    @Test("Check precedence for a mix of additions and multiplications (1)")
    func additionMultiplicationMix1() {
        let input = "3 + 3 * 3 + 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "((3 + (3 * 3)) + 3)")
    }

    @Test("Check precedence for a mix of additions and multiplications (2)")
    func additionMultiplicationMix2() {
        let input = "3 + 3 + 3 * 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "((3 + 3) + (3 * 3))")
    }

    @Test("Check precedence for a mix of additions and multiplications (3)")
    func additionMultiplicationMix3() {
        let input = "3 * 3 + 3 + 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "(((3 * 3) + 3) + 3)")
    }

    @Test("Check precedence for a mix of additions and multiplications (4)")
    func additionMultiplicationMix4() {
        let input = "3 * 3 + 3 * 3"
        var stream = Stream(string: input)
        let result = IntegerExpr.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "((3 * 3) + (3 * 3))")
    }
}
