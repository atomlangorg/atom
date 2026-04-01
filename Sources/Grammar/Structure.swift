//
//  Structure.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

protocol Grammar: Sendable {
    associatedtype Output: IR
}

protocol GrammarLiteral: Grammar where Output == RawStringIr {
    static var literal: Character { get }
}

protocol GrammarMatch: Grammar {
    static var patterns: [any GrammarPatternProtocol<Output>] { get }
}

protocol GrammarPatternProtocol<Output>: Sendable {
    associatedtype Output: IR

    func anyParts() -> [any Grammar.Type]
}

struct GrammarPattern<each Part: Grammar, Output: IR>: GrammarPatternProtocol {
    typealias Parts = (repeat (each Part).Type)
    typealias Gen = @Sendable (repeat (each Part).Output) throws(GrammarError) -> Output

    let parts: Parts
    let gen: Gen

    init(parts: Parts, gen: @escaping Gen) {
        self.parts = parts
        self.gen = gen
    }

    init(parts: Parts) where Output == NeverIr {
        self.parts = parts
        gen = { (_: repeat (each Part).Output) in NeverIr() }
    }

    func anyParts() -> [any Grammar.Type] {
        var anyParts = [any Grammar.Type]()
        for part in repeat each parts {
            anyParts.append(part)
        }
        return anyParts
    }
}
