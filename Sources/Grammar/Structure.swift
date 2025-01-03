//
//  Structure.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

protocol Grammar {
    associatedtype Output: IR

    static func consume(stream: inout Stream, context: GrammarContext) -> StreamStateMatch<Output>

    static func initialChars() -> Set<Character>
}

protocol GrammarLiteral: Grammar where Output == RawStringIr {
    static var literal: Character { get }
}

extension GrammarLiteral {
    static func consume(stream: inout Stream, context: GrammarContext) -> StreamStateMatch<Output> {
        stream.nextIf(char: literal)
    }

    static func initialChars() -> Set<Character> {
        [literal]
    }
}

protocol GrammarMatch: Grammar {
    static var patterns: [any GrammarPatternProtocol<Output>] { get }
}

extension GrammarMatch {
    static func consume(stream: inout Stream, context: GrammarContext) -> StreamStateMatch<Output> {
        var greediest: (stream: Stream, result: Result<Output, GrammarError>)? = nil
        var context = context
        context.setGrammarType(Self.self)

        for (index, pattern) in patterns.enumerated() {
            var s = stream
            context.setPatternIndex(index)

            let state = pattern.consume(stream: &s, context: context)
            stream.updateFarthest(relativeTo: s)
            switch state {
            case .dontConsume:
                continue
            case let .doConsume(result):
                if let g = greediest {
                    guard s.isGreedierThan(stream: g.stream, since: stream) else {
                        continue
                    }
                }
                greediest = (stream: s, result: result)
            case .end:
                continue
            case let .error(diagnostic):
                return .error(diagnostic)
            }
        }

        guard let greediest else {
            // Nothing was able to be consumed
            return .dontConsume
        }

        switch greediest.result {
        case let .success(ir):
            stream = greediest.stream

            // Reattempt to consume this grammar itself again. This allows for controlled left recursion.
            var context = context
            context.firstIr = ir
            switch consume(stream: &stream, context: context) {
            case .dontConsume:
                break
            case let .doConsume(newIr):
                return .doConsume(newIr)
            case .end:
                break
            case let .error(diagnostic):
                return .error(diagnostic)
            }

            return .doConsume(ir)
        case let .failure(error):
            let diagnostic = Diagnostic(start: stream.currentLocation(), end: greediest.stream.currentLocation(), error: error)
            return .error(diagnostic)
        }
    }
}

fileprivate var cachedInitialChars: [ObjectIdentifier: Set<Character>] = [:]

extension GrammarMatch {
    static func initialChars() -> Set<Character> {
        let selfId = ObjectIdentifier(self)

        if let chars = cachedInitialChars[selfId] {
            return chars
        }

        var chars = Set<Character>()

        for pattern in patterns {
            if let ty = pattern.initialType(), ty != Self.self {
                chars.formUnion(ty.initialChars())
            }
        }

        cachedInitialChars[selfId] = chars
        return chars
    }
}

protocol GrammarPatternProtocol<Output> {
    associatedtype Output: IR

    func consume(stream: inout Stream, context: GrammarContext) -> StreamStatePattern<Output>

    func initialType() -> (any Grammar.Type)?
}

struct GrammarPattern<each Part: Grammar, Output: IR>: GrammarPatternProtocol {
    typealias Parts = (repeat (each Part).Type)
    typealias Gen = (repeat (each Part).Output) throws(GrammarError) -> Output

    let parts: Parts
    let gen: Gen
    let precedence: Precedence?
    let options: Set<Option>

    init(parts: Parts, gen: @escaping Gen, precedence: Precedence? = nil, options: Set<Option> = []) {
        self.parts = parts
        self.gen = gen
        self.precedence = precedence
        self.options = options
    }

    init(parts: Parts, precedence: Precedence? = nil, options: Set<Option> = []) where Output == NeverIr {
        self.parts = parts
        gen = { (_: repeat (each Part).Output) in NeverIr() }
        self.precedence = precedence
        self.options = options
    }

    func consume(stream: inout Stream, context: GrammarContext) -> StreamStatePattern<Output> {
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
                let saved = s
                let state = part.consume(stream: &s, context: newContext)
                stream.updateFarthest(relativeTo: s)
                switch state {
                case .dontConsume:
                    return .dontConsume
                case let .doConsume(ir):
                    if s.isAheadOf(stream: saved) {
                        context.resetHistory()
                    }
                    irPack = irPack.appending(ir: ir)
                case .end:
                    return .end
                case let .error(diagnostic):
                    return .error(diagnostic)
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
        let result = Result(catching: { () throws(GrammarError) in
            try gen(repeat each irPackConcrete.irs)
        })
        return .doConsume(result)
    }

    func initialType() -> (any Grammar.Type)? {
        for part in repeat each parts {
            return part
        }
        return nil
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
        let snapshot = snapshot()

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

    private func snapshot() -> HistorySnapshot {
        HistorySnapshot(grammarType: grammarType!, patternIndex: patternIndex!, partIndex: partIndex!)
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
