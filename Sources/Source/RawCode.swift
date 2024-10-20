//
//  RawCode.swift
//  atom
//
//  Created by George Elsham on 20/10/2024.
//

struct RawCode {
    typealias Index = String.Index

    let string: String

    init(_ string: String) {
        self.string = string
    }
}

extension RawCode: CustomStringConvertible {
    var description: String {
        string
    }
}
