//
//  Stream.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

struct Stream {
    private let raw: RawCode
    private var index: RawCode.Index
    private var farthestIndex: RawCode.Index
    private var wildcardIndexes: [RawCode.Index]

    init(raw: RawCode) {
        self.raw = raw
        index = raw.string.startIndex
        farthestIndex = index
        wildcardIndexes = []
    }

    func isEnd() -> Bool {
        index >= raw.string.endIndex
    }

    func isFarthestAtEnd() -> Bool {
        farthestIndex >= raw.string.endIndex
    }

    func topChar() -> Character? {
        if isEnd() {
            return nil
        }
        return raw.string[index]
    }

    mutating func nextIf(char: Character) -> StreamStateMatch<RawStringIr> {
        guard let c = topChar() else {
            return .end
        }
        if char != c {
            return .dontConsume
        }
        incrementIndex()
        return .doConsume(RawStringIr(string: "\(c)"))
    }

    mutating func next() -> StreamStateMatch<RawStringIr> {
        guard let c = topChar() else {
            return .end
        }
        wildcardIndexes.append(index)
        incrementIndex()
        return .doConsume(RawStringIr(string: "\(c)"))
    }

    func currentLocation() -> SourceLocation {
        raw.sourceLocation(at: index)
    }

    func farthestLocation() -> SourceLocation {
        raw.sourceLocation(at: farthestIndex)
    }

    mutating func updateFarthest(relativeTo stream: Stream) {
        if stream.farthestIndex > farthestIndex {
            farthestIndex = stream.farthestIndex
        }
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

    private func firstWildcardIndex(from index: RawCode.Index) -> RawCode.Index? {
        // TODO: implement as binary search to make faster
        wildcardIndexes.first(where: { $0 >= index })
    }

    private mutating func incrementIndex() {
        raw.string.formIndex(after: &index)
        farthestIndex = index
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
