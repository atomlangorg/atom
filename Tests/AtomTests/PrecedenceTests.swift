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
    @Test("Check precedence of three additions")
    func threeAdditions() {
        let input = "let x = 3 + 3 + 3 + 3"
        var stream = Stream(string: input)
        let result = Program.consume(stream: &stream, context: GrammarContext())

        #expect(stream.isEnd())

        guard case let .doConsume(ir) = result else {
            Issue.record("Result was not consumed")
            return
        }

        #expect(ir.swift() == "let x = (((3 + 3) + 3) + 3)")
    }
}
