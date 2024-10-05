//
//  Stream.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

struct Stream {
    let string: String
    var index: String.Index
    var wildcardIndexes: [String.Index]

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

    mutating func nextIf(char: Character) -> StreamState<RawStringIr> {
        guard let c = topChar() else {
            return .end
        }
        if char != c {
            return .dontConsume
        }
        string.formIndex(after: &index)
        return .doConsume(RawStringIr(string: "\(c)"))
    }

    mutating func next() -> StreamState<RawStringIr> {
        guard let c = topChar() else {
            return .end
        }
        wildcardIndexes.append(index)
        string.formIndex(after: &index)
        return .doConsume(RawStringIr(string: "\(c)"))
    }

    func firstWildcardIndex(from index: String.Index) -> String.Index? {
        // TODO: implement as binary search to make faster
        wildcardIndexes.first(where: { $0 >= index })
    }
}

enum StreamState<T: IR> {
    case dontConsume
    case doConsume(T)
    case end
}
