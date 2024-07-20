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

    mutating func nextIf(char: Character) -> StreamState {
        guard let c = topChar() else {
            return .end
        }
        if char == c {
            string.formIndex(after: &index)
            return .doConsume(nil)
        }
        return .dontConsume
    }
}

enum StreamState {
    case dontConsume
    case doConsume(String?)
    case end
}
