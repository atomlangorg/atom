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
        var context = context
        context.setGrammarType(Self.self)

        for (index, pattern) in patterns.enumerated() {
            var s = stream
            context.setPatternIndex(index)

            switch pattern.consume(stream: &s, context: context) {
            case .dontConsume:
                continue
            case let .doConsume(ir):
                if let g = greediest {
                    if s.index <= g.index {
                        continue
                    }
                }
                greediest = (index: s.index, ir: ir)
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
        var context = context
        var irPack: any IrPackProtocol = IrPack< >(irs: ())
        var index = 0

        for part in repeat each parts {
            context.setPartIndex(index)

            switch context.addingToHistory() {
            case .cycle:
                return .dontConsume
            case let .changed(newContext):
                switch part.consume(stream: &s, context: newContext) {
                case .dontConsume:
                    return .dontConsume
                case let .doConsume(ir):
                    context.resetHistory()
                    irPack = irPack.appending(ir: ir)
                    index += 1
                    continue
                case .end:
                    return .dontConsume
                }
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
    private var history: [HistorySnapshot]
    private var grammarType: (any Grammar.Type)?
    private var patternIndex: Int?
    private var partIndex: Int?

    init() {
        history = []
        grammarType = nil
        patternIndex = nil
        partIndex = nil
    }

    fileprivate func addingToHistory() -> HistoryResult {
        let snapshot = HistorySnapshot(grammarType: grammarType!, patternIndex: patternIndex!, partIndex: partIndex!)

        for historySnapshot in history.reversed() {
            if historySnapshot == snapshot {
                return .cycle
            }
        }

        var new = self
        new.history.append(snapshot)
        return .changed(new)
    }

    fileprivate mutating func resetHistory() {
        history.removeAll()
    }

    fileprivate mutating func setGrammarType(_ value: any Grammar.Type) {
        grammarType = value
    }

    fileprivate mutating func setPatternIndex(_ value: Int) {
        patternIndex = value
    }

    fileprivate mutating func setPartIndex(_ value: Int) {
        partIndex = value
    }
}

fileprivate struct HistorySnapshot: Equatable {
    private let grammarType: any Grammar.Type
    private let patternIndex: Int
    private let partIndex: Int

    init(grammarType: any Grammar.Type, patternIndex: Int, partIndex: Int) {
        self.grammarType = grammarType
        self.patternIndex = patternIndex
        self.partIndex = partIndex
    }

    static func == (lhs: HistorySnapshot, rhs: HistorySnapshot) -> Bool {
        lhs.grammarType == rhs.grammarType &&
        lhs.patternIndex == rhs.patternIndex &&
        lhs.partIndex == rhs.partIndex
    }
}

fileprivate enum HistoryResult {
    case cycle // Detected infinite cycle
    case changed(GrammarContext) // Continue, with new context
}
