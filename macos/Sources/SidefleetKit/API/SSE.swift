import Foundation

public struct SSEFrame: Sendable, Hashable {
    public var event: String
    public var data: String
    public var id: String?

    public init(event: String = "message", data: String = "", id: String? = nil) {
        self.event = event
        self.data = data
        self.id = id
    }
}

/// Splits a byte stream into server-sent events. Chunks can end anywhere, even inside a character.
public struct SSEParser: Sendable {
    private var buffer = [UInt8]()

    public init() {}

    public mutating func feed(_ bytes: some Sequence<UInt8>) -> [SSEFrame] {
        buffer.append(contentsOf: bytes)
        var frames: [SSEFrame] = []
        var start = 0
        var i = 0
        while i < buffer.count {
            // A frame ends at a blank line: \n\n or \r\n\r\n.
            if buffer[i] == 10, i + 1 < buffer.count {
                var next = i + 1
                if buffer[next] == 13, next + 1 < buffer.count { next += 1 }
                if buffer[next] == 10 {
                    if let frame = Self.parse(buffer[start..<i]) { frames.append(frame) }
                    i = next + 1
                    start = i
                    continue
                }
            }
            i += 1
        }
        if start > 0 { buffer.removeFirst(start) }
        return frames
    }

    private static func parse(_ raw: ArraySlice<UInt8>) -> SSEFrame? {
        var frame = SSEFrame()
        var sawField = false
        var dataLines: [String] = []
        for line in String(decoding: raw, as: UTF8.self).split(omittingEmptySubsequences: true, whereSeparator: { $0.isNewline }) {
            if line.first == ":" { continue } // a comment: keep-alive
            let parts = line.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false)
            var value = parts.count > 1 ? String(parts[1]) : ""
            if value.first == " " { value.removeFirst() }
            switch parts[0] {
            case "event":
                frame.event = value
                sawField = true
            case "data":
                dataLines.append(value)
                sawField = true
            case "id":
                frame.id = value
                sawField = true
            default:
                continue
            }
        }
        frame.data = dataLines.joined(separator: "\n")
        return sawField ? frame : nil
    }
}
