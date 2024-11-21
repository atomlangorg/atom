//
//  Diagnostic.swift
//  atom
//
//  Created by George Elsham on 19/10/2024.
//

struct Diagnostic {
    let start: SourceLocation
    let end: SourceLocation
    let error: GrammarError

    init(start: SourceLocation, end: SourceLocation, error: GrammarError) {
        guard start < end else {
            fatalError("Invalid source location range of \(start) to \(end)")
        }

        self.start = start
        self.end = end
        self.error = error
    }
}

extension Diagnostic {
    func formattedLine() -> String {
        "Error at \(start) to \(end): \(error.reason)"
    }

    func formattedInCode(_ program: Program) -> String {
        let isMultiline = start.y != end.y

        if isMultiline {
            var endInsertIndex: String.Index!
            return program.source.formattedAsCodeBlock(preformatting: { code in
                do {
                    let insertIndex = code.base.endOfLine(containing: start.index)
                    let leftPadding = String(repeating: PrettyPrint.sectionLineHorizontal, count: Int(start.x))
                    let underline = PrettyPrint.underline
                    let text = "\n\(leftPadding)\(underline)"
                    code.insert(text, at: insertIndex)
                }
                do {
                    let insertIndex = code.base.endOfLine(containing: end.index)
                    endInsertIndex = insertIndex
                    let leftPadding = String(repeating: PrettyPrint.sectionLineHorizontal, count: Int(end.x - 1))
                    let underline = PrettyPrint.underline
                    let reason = error.reason
                    let text = "\n\(leftPadding)\(underline) \(reason)"
                    code.insert(text, at: insertIndex)
                }
            }, postformatting: { code, indent in
                let lineCount = end.y - start.y + 1
                var i: UInt = 0
                code.modify(each: { insertion in
                    guard start.index < insertion.index && insertion.index <= endInsertIndex else {
                        return
                    }

                    let char = switch lineCount - i {
                    case 0: PrettyPrint.sectionCornerTopLeft
                    case lineCount: PrettyPrint.sectionCornerBottomLeft
                    default: PrettyPrint.sectionLineVertical
                    }

                    let indexOffset = insertion.string.first == "\n" ? 1 : 0
                    let index = insertion.string.index(insertion.string.startIndex, offsetBy: indent - 1 + indexOffset)
                    insertion.string.replaceSubrange(index ... index, with: String(char))

                    i += 1
                })
            })
        } else {
            return program.source.formattedAsCodeBlock(preformatting: { code in
                let insertIndex = code.base.endOfLine(containing: start.index)
                let leftPadding = String(repeating: " ", count: Int(start.x))
                let underlineCount = Int(end.x - start.x)
                let underline = String(repeating: PrettyPrint.underline, count: underlineCount)
                let reason = error.reason
                let text = "\n\(leftPadding)\(underline) \(reason)"
                code.insert(text, at: insertIndex)
            })
        }
    }
}
