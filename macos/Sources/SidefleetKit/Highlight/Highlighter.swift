import Foundation

// Syntax colours for diffs and code blocks. A small tokenizer: strings, comments, numbers, keywords,
// types and calls for the languages that come up most. Less exact than the page's Shiki on hard cases
// (regex literals, nested template strings), and unknown languages stay plain.

public enum TokenKind: Sendable, Hashable { case plain, keyword, string, comment, number, type, function }

public struct Token: Sendable, Hashable {
    public var text: String
    public var kind: TokenKind

    public init(text: String, kind: TokenKind) {
        self.text = text
        self.kind = kind
    }
}

public struct Language: Sendable {
    var keywords: Set<String>
    var lineComments: [String]
    var blockComment: (open: String, close: String)?
    var stringQuotes: Set<Character>
    /// `Capitalised` words read as types.
    var capitalizedTypes = true
    var hashComment = false
}

public enum Highlighter {
    private static let cLike = "if else for while do switch case default break continue return try catch finally throw new delete this super class struct enum interface extends implements import export from as const let var function static public private protected void null true false"

    private static func kw(_ s: String) -> Set<String> { Set(s.split(separator: " ").map(String.init)) }

    static let typescript = Language(keywords: kw(cLike + " async await yield typeof instanceof in of type declare namespace readonly undefined abstract get set"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\"", "'", "`"])
    static let rust = Language(keywords: kw("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\""])
    static let python = Language(keywords: kw("and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"])
    static let go = Language(keywords: kw("break case chan const continue default defer else fallthrough false for func go goto if import interface map nil package range return select struct switch true type var"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\"", "`"])
    static let swift = Language(keywords: kw("actor any as associatedtype async await break case catch class continue default defer deinit do else enum extension fallthrough false fileprivate final for func guard if import in init inout internal is let nil nonisolated open override private protocol public repeat rethrows return self Self some static struct subscript super switch throw throws true try typealias var weak where while"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\""])
    static let cFamily = Language(keywords: kw(cLike + " auto bool char double float inline int long namespace short signed sizeof template typedef typename union unsigned using virtual"), lineComments: ["//"], blockComment: ("/*", "*/"), stringQuotes: ["\"", "'"])
    static let shell = Language(keywords: kw("if then else elif fi for while do done case esac function in return exit export local readonly set unset echo cd"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let json = Language(keywords: kw("true false null"), lineComments: [], blockComment: nil, stringQuotes: ["\""], capitalizedTypes: false)
    static let yaml = Language(keywords: kw("true false null yes no"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let css = Language(keywords: kw("important media import supports keyframes"), lineComments: [], blockComment: ("/*", "*/"), stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let html = Language(keywords: [], lineComments: [], blockComment: ("<!--", "-->"), stringQuotes: ["\"", "'"], capitalizedTypes: false)
    static let sql = Language(keywords: kw("select from where insert into update delete create table alter drop join left right inner outer on as and or not null primary key references index values set group by order having limit returning default SELECT FROM WHERE INSERT INTO UPDATE DELETE CREATE TABLE ALTER DROP JOIN LEFT RIGHT INNER OUTER ON AS AND OR NOT NULL PRIMARY KEY REFERENCES INDEX VALUES SET GROUP BY ORDER HAVING LIMIT RETURNING DEFAULT"), lineComments: ["--"], blockComment: ("/*", "*/"), stringQuotes: ["'"], capitalizedTypes: false)
    static let ruby = Language(keywords: kw("alias and begin break case class def do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true unless until when while yield"), lineComments: ["#"], blockComment: nil, stringQuotes: ["\"", "'"])

    /// The language for a file path, or a code fence's name.
    public static func language(for name: String?) -> Language? {
        guard let name else { return nil }
        let ext = (name.split(separator: ".").last.map(String.init) ?? name).lowercased()
        switch ext {
        case "ts", "tsx", "mts", "cts", "typescript", "js", "jsx", "mjs", "cjs", "javascript", "svelte", "vue", "kt", "java", "dart", "php", "cs": return typescript
        case "rs", "rust": return rust
        case "py", "python": return python
        case "go", "golang": return go
        case "swift": return swift
        case "c", "h", "cc", "cpp", "hpp", "cxx": return cFamily
        case "sh", "bash", "zsh", "shell", "console": return shell
        case "json", "jsonc": return json
        case "yaml", "yml", "toml": return yaml
        case "css", "scss": return css
        case "html", "xml": return html
        case "sql": return sql
        case "rb", "ruby": return ruby
        default: return nil
        }
    }

    /// Lines in order, carrying an open block comment from one to the next.
    public static func highlight(lines: [String], language: Language?) -> [[Token]] {
        guard let language else { return lines.map { [Token(text: $0, kind: .plain)] } }
        var inBlock = false
        return lines.map { tokenize($0, language, &inBlock) }
    }

    public static func tokenize(_ line: String, _ lang: Language, _ inBlock: inout Bool) -> [Token] {
        var out: [Token] = []
        let chars = Array(line)
        var i = 0
        var plain = ""

        func flush() {
            if !plain.isEmpty {
                out.append(Token(text: plain, kind: .plain))
                plain = ""
            }
        }
        func matches(_ s: String, at at: Int) -> Bool {
            let c = Array(s)
            guard at + c.count <= chars.count else { return false }
            for k in c.indices where chars[at + k] != c[k] { return false }
            return true
        }

        while i < chars.count {
            if inBlock, let close = lang.blockComment?.close {
                var j = i
                while j < chars.count, !matches(close, at: j) { j += 1 }
                let end = min(chars.count, j + (j < chars.count ? close.count : 0))
                out.append(Token(text: String(chars[i..<end]), kind: .comment))
                if j < chars.count { inBlock = false }
                i = end
                continue
            }
            if let open = lang.blockComment?.open, matches(open, at: i) {
                flush()
                inBlock = true
                continue
            }
            if lang.lineComments.contains(where: { matches($0, at: i) }) {
                flush()
                out.append(Token(text: String(chars[i...]), kind: .comment))
                return out
            }
            let c = chars[i]
            if lang.stringQuotes.contains(c) {
                flush()
                var j = i + 1
                while j < chars.count, chars[j] != c {
                    if chars[j] == "\\" { j += 1 }
                    j += 1
                }
                let end = min(chars.count, j + 1)
                out.append(Token(text: String(chars[i..<end]), kind: .string))
                i = end
                continue
            }
            if c.isNumber, plain.last.map({ !($0.isLetter || $0.isNumber || $0 == "_") }) ?? true {
                flush()
                var j = i
                while j < chars.count, chars[j].isHexDigit || chars[j] == "." || chars[j] == "_" || chars[j] == "x" { j += 1 }
                out.append(Token(text: String(chars[i..<j]), kind: .number))
                i = j
                continue
            }
            if c.isLetter || c == "_" {
                var j = i
                while j < chars.count, chars[j].isLetter || chars[j].isNumber || chars[j] == "_" { j += 1 }
                let word = String(chars[i..<j])
                let next = j < chars.count ? chars[j] : " "
                let kind: TokenKind =
                    lang.keywords.contains(word) ? .keyword
                    : next == "(" ? .function
                    : lang.capitalizedTypes && c.isUppercase ? .type
                    : .plain
                if kind == .plain {
                    plain += word
                } else {
                    flush()
                    out.append(Token(text: word, kind: kind))
                }
                i = j
                continue
            }
            plain.append(c)
            i += 1
        }
        flush()
        return out
    }
}
