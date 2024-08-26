//
//  Structure.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

protocol Grammar {
    associatedtype Output: IR

    static func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output>
}

protocol GrammarLiteral: Grammar where Output == NeverIr {
    static var literal: Character { get }
}

extension GrammarLiteral {
    static func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        stream.nextIf(char: literal)
    }
}

protocol GrammarMatch: Grammar {
    static var patterns: [any GrammarPatternProtocol<Output>] { get }
}

extension GrammarMatch {
    static func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        var greediest: (index: String.Index, ir: Output)? = nil

        for pattern in patterns {
            var s = stream

            switch pattern.consumeWithContext(stream: &s, context: context) {
            case .dontConsume:
                continue
            case let .doConsume(ir):
                if let g = greediest {
                    if s.index > g.index {
                        greediest = (index: s.index, ir: ir)
                    }
                } else {
                    greediest = (index: s.index, ir: ir)
                }
            case .end:
                return .dontConsume
            }
        }

        if let greediest {
            stream.index = greediest.index
            return .doConsume(greediest.ir)
        }
        return .dontConsume
    }
}

protocol GrammarPatternProtocol<Output> {
    associatedtype Output: IR

    func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output>
}

extension GrammarPatternProtocol {
    fileprivate func consumeWithContext(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        switch context.addingToHistory(type: Self.self) {
        case .cycle:
            return .dontConsume
        case let .changed(newContext):
            return consume(stream: &stream, context: newContext)
        }
    }
}

struct GrammarPattern<each Part: Grammar, Output: IR>: GrammarPatternProtocol {
    let parts: (repeat (each Part).Type)
    let gen: (repeat (each Part).Output) -> Output

    init(parts: (repeat (each Part).Type), gen: @escaping (repeat (each Part).Output) -> Output) {
        self.parts = parts
        self.gen = gen
    }

    init(parts: (repeat (each Part).Type)) where Output == NeverIr {
        self.parts = parts
        gen = { (_: repeat (each Part).Output) in NeverIr() }
    }

    func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        var s = stream
        var irPack: any IrPackProtocol = IrPack< >(irs: ())

        for part in repeat each parts {
            switch part.consume(stream: &s, context: context) {
            case .dontConsume:
                return .dontConsume
            case let .doConsume(ir):
                irPack = irPack.appending(ir: ir)
                continue
            case .end:
                return .dontConsume
            }
        }

        stream = s
        let irPackConcrete = irPack as! IrPack<repeat (each Part).Output>
        let result = gen(repeat each irPackConcrete.irs)
        return .doConsume(result)
    }
}

fileprivate protocol IrPackProtocol {
    func appending<T: IR>(ir: T) -> any IrPackProtocol
}

fileprivate struct IrPack<each I: IR>: IrPackProtocol {
    let irs: (repeat each I)

    init(irs: (repeat each I)) {
        self.irs = irs
    }

    func appending<T: IR>(ir: T) -> any IrPackProtocol {
        IrPack<repeat each I, T>(irs: (repeat each irs, ir))
    }
}

struct GrammarContext {
    typealias Snapshot = any GrammarPatternProtocol.Type

    private var history: [Snapshot]

    init() {
        history = []
    }

    private init(history: [Snapshot]) {
        self.history = history
    }

    fileprivate func addingToHistory(type: Snapshot) -> HistoryResult {
        for historyType in history.reversed() {
            if historyType == type {
                return .cycle
            }
        }

        var history = history
        history.append(type)
        return .changed(GrammarContext(history: history))
    }
}

fileprivate enum HistoryResult {
    case cycle // Detected infinite cycle
    case changed(GrammarContext) // Continue, with new context
}
