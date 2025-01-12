//
//  Automaton.swift
//  atom
//
//  Created by George Elsham on 04/01/2025.
//

// This is a Deterministic Pushdown Automaton (DPDA). The wildcard symbol is
// still deterministic because it is only consumed in the case that no other
// symbol can be consumed.

class State {
    var transitions: [Transition]

    init() {
        transitions = []
    }

    func consumeImmediate(char: Character) -> Transition? {
        var result: Transition?
        for transition in transitions {
            switch transition.consume(char: char) {
            case .none:
                continue
            case .char:
                return transition
            case .wildcard:
                result = transition
            }
        }
        return result
    }

    func consumeAll(stream: MyStream) throws(GrammarError) -> any IR {
        var stream = stream
        var state = self
        var irList = [any IR]()
        var genLater = [Run.Generate]()

        func generateIr() throws(GrammarError) {
            while true {
                guard let last = genLater.last else {
                    // Gen later list is empty
                    return
                }
                guard last.count == 1 else {
                    // Last isn't ready yet
                    return
                }

                // Run last generator
                genLater.removeLast()
                try last.generator(&irList)

                // Get the now new last, previously the second last
                guard let last = genLater.last else {
                    return
                }

                // Decrement the last's count
                let new = Run.Generate(generator: last.generator, count: last.count - 1)
                genLater[genLater.count - 1] = new
            }
        }

        while let char = stream.topChar() {
            guard let transition = state.consumeImmediate(char: char) else {
                throw GrammarError("unexpected grammar")
            }
            let ir = RawStringIr(string: "\(char)")

            switch transition.run {
            case .nothing:
                irList.append(ir)
            case let .generate(current, previous):
                if let previous {
                    genLater.removeLast()
                    genLater.append(previous)
                }
                try generateIr()
                irList.append(ir)
                genLater.append(current)
            }

            state = transition.next
            stream.next()
        }

        // Try generate final IR
        try generateIr()

        // Return result
        guard irList.count == 1 else {
            fatalError()
        }
        return irList[0]
    }
}

struct Transition {
    let input: Input
    let next: State
    let run: Run

    init(input: Input, next: State) {
        self.input = input
        self.next = next
        run = .nothing
    }

    init<each I1: IR, O1: IR, each I2: IR, O2: IR>(input: Input, next: State, gen: @escaping Run.GenerateGeneric<repeat each I1, O1>, genPrev: @escaping Run.GenerateGeneric<repeat each I2, O2>) {
        self.input = input
        self.next = next

        let current = Run.Generate(generator: gen)
        let previous = Run.Generate(generator: genPrev)
        run = .generate(current: current, previous: previous)
    }

    init<each I1: IR, O1: IR>(input: Input, next: State, gen: @escaping Run.GenerateGeneric<repeat each I1, O1>) {
        self.input = input
        self.next = next

        let current = Run.Generate(generator: gen)
        run = .generate(current: current, previous: nil)
    }

    func consume(char: Character) -> ConsumeResult {
        switch input {
        case .char(let c):
            if c == char {
                return ConsumeResult.char
            } else {
                return ConsumeResult.none
            }
        case .wildcard:
            return ConsumeResult.wildcard
        }
    }
}

extension Transition {
    enum Input {
        case char(Character)
        case wildcard
    }
}

enum ConsumeResult {
    case none
    case char
    case wildcard
}

enum Run {
    case nothing
    case generate(current: Generate, previous: Generate?)
}

extension Run {
    typealias GenerateAny = (inout [any IR]) throws(GrammarError) -> Void
    typealias GenerateGeneric<each Input: IR, Output: IR> = @Sendable (repeat each Input) throws(GrammarError) -> Output

    struct Generate {
        let generator: GenerateAny
        let count: Int

        init(generator: @escaping GenerateAny, count: Int) {
            self.generator = generator
            self.count = count
        }

        init<each Input: IR, Output: IR>(generator: @escaping GenerateGeneric<repeat each Input, Output>) {
            let count = CountGenerics<repeat each Input>.count()
            let generator: GenerateAny = { irs throws(GrammarError) in
                guard irs.count >= count else {
                    fatalError("IRs list is too short")
                }

                let lastIrs = irs.suffix(count)
                irs.removeLast(count)

                var irPack: any IrPackProtocol = IrPack< >(irs: ())
                for ir in lastIrs {
                    irPack = irPack.appending(ir: ir)
                }
                let irPackConcrete = irPack as! IrPack<repeat each Input>
                let new = try generator(repeat each irPackConcrete.irs)
                irs.append(new)
            }
            self.init(generator: generator, count: count)
        }
    }
}

struct CountGenerics<each T> {
    @available(*, unavailable)
    init() {}

    static func count() -> Int {
        var count = 0
        _ = (repeat ((each T).self, count += 1))
        return count
    }
}

struct MyStream {
    private let raw: RawCode
    private var index: RawCode.Index

    init(raw: RawCode) {
        self.raw = raw
        index = raw.string.startIndex
    }

    func isEnd() -> Bool {
        index >= raw.string.endIndex
    }

    func topChar() -> Character? {
        if isEnd() {
            return nil
        }
        return raw.string[index]
    }

    mutating func next() {
        raw.string.formIndex(after: &index)
    }

    func currentLocation() -> SourceLocation {
        raw.sourceLocation(at: index)
    }

    func isEvenWith(stream: MyStream) -> Bool {
        index == stream.index
    }

    func isAheadOf(stream: MyStream) -> Bool {
        index > stream.index
    }
}

func temp1() {
    let s1 = State()
    let s2 = State()
    let s3 = State()

    let t1 = Transition(input: .char("<"), next: s2, gen: { (lhs: RawStringIr, rhs: RawStringIr) in
        RawStringIr(string: "<" + rhs.string)
    })
    s1.transitions.append(t1)

    let t2 = Transition(input: .char(">"), next: s3, gen: { (char: RawStringIr) in
        char
    })
    s2.transitions.append(t2)

    let t3 = Transition(input: .char("<"), next: s2, gen: { (lhs: RawStringIr, rhs: RawStringIr) in
        RawStringIr(string: "<" + rhs.string)
    }, genPrev: { (lhs: RawStringIr, rhs: RawStringIr) in
        RawStringIr(string: ">" + rhs.string)
    })
    s3.transitions.append(t3)

    let stream = MyStream(raw: RawCode("<><><>", isSource: true))
    let ir = try! s1.consumeAll(stream: stream)
    print("ir result = \(ir)")
}
