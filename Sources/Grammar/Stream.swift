//
//  Stream.swift
//  atom
//
//  Created by George Elsham on 19/07/2024.
//

struct Stream {
    let string: String
    var index: String.Index

    init(string: String) {
        self.string = string
        index = string.startIndex
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

    mutating func nextIf(char: Character) -> StreamState<NeverIr> {
        guard let c = topChar() else {
            return .end
        }
        if char != c {
            return .dontConsume
        }
        string.formIndex(after: &index)
        return .doConsume(NeverIr())
    }
}

enum StreamState<T: IR> {
    case dontConsume
    case doConsume(T)
    case end
}
