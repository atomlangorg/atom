//
//  Stream.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

struct Stream {
    private let string: String
    private var index: String.Index
    private var wildcardIndexes: [String.Index]

    init(string: String) {
        self.string = string
        index = string.startIndex
        wildcardIndexes = []
    }

    func isEnd() -> Bool {
        index >= string.endIndex
    }

    func topChar() -> Character? {
        if isEnd() {
            return nil
        }
        return string[index]
    }

    mutating func nextIf(char: Character) -> StreamStateMatch<RawStringIr> {
        guard let c = topChar() else {
            return .end
        }
        if char != c {
            return .dontConsume
        }
        string.formIndex(after: &index)
        return .doConsume(RawStringIr(string: "\(c)"))
    }

    mutating func next() -> StreamStateMatch<RawStringIr> {
        guard let c = topChar() else {
            return .end
        }
        wildcardIndexes.append(index)
        string.formIndex(after: &index)
        return .doConsume(RawStringIr(string: "\(c)"))
    }

    func sourceLocation() -> SourceLocation {
        var line: UInt = 0
        var column: UInt = 0

        for i in string.indices {
            if i == index {
                return SourceLocation(line: line, column: column)
            }

            let char = string[i]
            let isLineSep = char == "\n" || char == "\r\n"
            if isLineSep {
                line += 1
                column = 0
            } else {
                column += 1
            }
        }

        // One character past the end
        return SourceLocation(line: line, column: column)
    }

    func isEvenWith(stream: Stream) -> Bool {
        index == stream.index
    }

    func isAheadOf(stream: Stream) -> Bool {
        index > stream.index
    }

    func isLessEagerWithWildcardsThan(stream: Stream, since commonAncestorStream: Stream) -> Bool {
        if let ci = firstWildcardIndex(from: commonAncestorStream.index) {
            if let gi = stream.firstWildcardIndex(from: commonAncestorStream.index) {
                if ci < gi {
                    // Current stream had a wildcard earlier than the greediest so far
                    return false
                }
            } else {
                // Current stream had a wildcard but the greediest so far does not
                return false
            }
        }

        return true
    }

    func isGreedierThan(stream: Stream, since commonAncestorStream: Stream) -> Bool {
        guard isAheadOf(stream: stream) else {
            // Current stream did not consume more characters than the greediest so far
            if isEvenWith(stream: stream) {
                if isLessEagerWithWildcardsThan(stream: stream, since: commonAncestorStream) {
                    // Grammar was ambiguous, but with difference in wildcards
                    return true
                }

                if stream.firstWildcardIndex(from: commonAncestorStream.index) == nil {
                    // Current stream must have some wildcards, while the greediest has none left
                    return false
                }

                // Grammar is simply ambiguous
                fatalError("Ambiguous grammar")
            }
            return false
        }

        return isLessEagerWithWildcardsThan(stream: stream, since: commonAncestorStream)
    }

    private func firstWildcardIndex(from index: String.Index) -> String.Index? {
        // TODO: implement as binary search to make faster
        wildcardIndexes.first(where: { $0 >= index })
    }
}

enum StreamStateMatch<T: IR> {
    case dontConsume
    case doConsume(T)
    case end
    case error(Diagnostic)
}

enum StreamStatePattern<T: IR> {
    case dontConsume
    case doConsume(Result<T, GrammarError>)
    case end
    case error(Diagnostic)
}
