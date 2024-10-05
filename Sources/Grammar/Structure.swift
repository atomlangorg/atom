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

protocol GrammarLiteral: Grammar where Output == RawStringIr {
    static var literal: Character { get }
}

extension GrammarLiteral {
    static func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        if Self.self == Literal.Wildcard.self {
            return stream.next()
        }
        return stream.nextIf(char: literal)
    }
}

protocol GrammarMatch: Grammar {
    static var patterns: [any GrammarPatternProtocol<Output>] { get }
}

extension GrammarMatch {
    static func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        var greediest: (stream: Stream, ir: Output)? = nil
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
                    if s.index <= g.stream.index {
                        continue
                    }
                }
                greediest = (stream: s, ir: ir)
            case .end:
                continue
            }
        }

        if let greediest {
            stream = greediest.stream

            // Reattempt to consume this grammar itself again. This allows for controlled left recursion.
            var context = context
            context.firstIr = greediest.ir
            let state = consume(stream: &stream, context: context)
            if case .doConsume = state {
                return state
            }

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
    let precedence: Precedence?
    let options: Set<Option>

    init(parts: (repeat (each Part).Type), gen: @escaping (repeat (each Part).Output) -> Output, precedence: Precedence? = nil, options: Set<Option> = []) {
        self.parts = parts
        self.gen = gen
        self.precedence = precedence
        self.options = options
    }

    init(parts: (repeat (each Part).Type), precedence: Precedence? = nil, options: Set<Option> = []) where Output == NeverIr {
        self.parts = parts
        gen = { (_: repeat (each Part).Output) in NeverIr() }
        self.precedence = precedence
        self.options = options
    }

    func consume(stream: inout Stream, context: GrammarContext) -> StreamState<Output> {
        var context = context
        guard context.acceptPrecedence(precedence) else {
            return .dontConsume
        }

        if options.contains(.resetPrecedence) {
            context.resetPrecedence()
        }

        var s = stream
        var irPack: any IrPackProtocol = IrPack< >(irs: ())
        var index = 0

        for part in repeat each parts {
            // Use up first IR if it exists, which can only be used iff the first part is recursive
            if let firstIr = context.firstIr {
                context.firstIr = nil
                guard context.isGrammarType(part) else {
                    return .dontConsume
                }
                irPack = irPack.appending(ir: firstIr)
                index += 1
                continue
            }

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
                case .end:
                    return .end
                }
            }

            index += 1
        }

        if context.firstIr != nil {
            // No parts existed to consume but a first IR was given
            return .dontConsume
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
    private var minPrecedence: Precedence
    fileprivate var firstIr: (any IR)?

    init() {
        history = []
        grammarType = nil
        patternIndex = nil
        partIndex = nil
        minPrecedence = .default()
        firstIr = nil
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

    fileprivate func isGrammarType(_ type: any Grammar.Type) -> Bool {
        grammarType == type
    }

    fileprivate mutating func setPatternIndex(_ value: Int) {
        patternIndex = value
    }

    fileprivate mutating func setPartIndex(_ value: Int) {
        partIndex = value
    }

    fileprivate mutating func acceptPrecedence(_ value: Precedence?) -> Bool {
        guard let value else {
            // No priority so just accept
            return true
        }

        // Accept and update if new is higher than current or the priority is the same but now with right associativity
        let isAccepted = value.priority > minPrecedence.priority || (value.priority == minPrecedence.priority && value.associativity == .right)
        if isAccepted {
            minPrecedence = value
        }
        return isAccepted
    }

    fileprivate mutating func resetPrecedence() {
        minPrecedence = .default()
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

extension HistorySnapshot: CustomDebugStringConvertible {
    var debugDescription: String {
        "\(String(describing: grammarType))[\(patternIndex), \(partIndex)]"
    }
}

fileprivate enum HistoryResult {
    case cycle // Detected infinite cycle
    case changed(GrammarContext) // Continue, with new context
}
