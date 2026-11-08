import Foundation

/// A parsed SVG path, reduced to absolute moves, lines and cubic curves: all SwiftUI's `Path` needs.
public enum PathCommand: Sendable, Hashable {
    case move(Double, Double)
    case line(Double, Double)
    case curve(x: Double, y: Double, c1x: Double, c1y: Double, c2x: Double, c2y: Double)
    case close
}

public enum SVGPath {
    public static func parse(_ d: String) -> [PathCommand] {
        var out: [PathCommand] = []
        let chars = Array(d)
        var i = 0
        var cmd: Character = " "
        var x = 0.0, y = 0.0, startX = 0.0, startY = 0.0
        var lastC2: (Double, Double)?

        func skipSeparators() {
            while i < chars.count, chars[i] == " " || chars[i] == "," || chars[i].isNewline { i += 1 }
        }
        func number() -> Double? {
            skipSeparators()
            var s = ""
            var seenDot = false
            if i < chars.count, chars[i] == "-" || chars[i] == "+" {
                s.append(chars[i])
                i += 1
            }
            while i < chars.count {
                let c = chars[i]
                if c.isNumber {
                    s.append(c)
                } else if c == ".", !seenDot {
                    seenDot = true
                    s.append(c)
                } else if c == "e" || c == "E", !s.isEmpty {
                    s.append(c)
                    i += 1
                    if i < chars.count, chars[i] == "-" || chars[i] == "+" {
                        s.append(chars[i])
                        i += 1
                    }
                    continue
                } else {
                    break
                }
                i += 1
            }
            return Double(s)
        }

        // Arc flags are one digit each and can run straight into what follows: `a1 1 0 01.5.5`.
        func flag() -> Bool? {
            skipSeparators()
            guard i < chars.count, chars[i] == "0" || chars[i] == "1" else { return nil }
            defer { i += 1 }
            return chars[i] == "1"
        }

        while true {
            skipSeparators()
            guard i < chars.count else { break }
            if chars[i].isLetter {
                cmd = chars[i]
                i += 1
                if cmd == "z" || cmd == "Z" {
                    out.append(.close)
                    x = startX
                    y = startY
                    lastC2 = nil
                    continue
                }
            }
            let relative = cmd.isLowercase
            switch cmd.lowercased() {
            case "m":
                guard let a = number(), let b = number() else { return out }
                x = relative ? x + a : a
                y = relative ? y + b : b
                startX = x
                startY = y
                out.append(.move(x, y))
                // Further pairs after a move are lines.
                cmd = relative ? "l" : "L"
                lastC2 = nil
            case "l":
                guard let a = number(), let b = number() else { return out }
                x = relative ? x + a : a
                y = relative ? y + b : b
                out.append(.line(x, y))
                lastC2 = nil
            case "h":
                guard let a = number() else { return out }
                x = relative ? x + a : a
                out.append(.line(x, y))
                lastC2 = nil
            case "v":
                guard let a = number() else { return out }
                y = relative ? y + a : a
                out.append(.line(x, y))
                lastC2 = nil
            case "c":
                guard let a = number(), let b = number(), let c = number(), let e = number(), let f = number(), let g = number() else { return out }
                let ox = relative ? x : 0, oy = relative ? y : 0
                let c2 = (c + ox, e + oy)
                out.append(.curve(x: f + ox, y: g + oy, c1x: a + ox, c1y: b + oy, c2x: c2.0, c2y: c2.1))
                x = f + ox
                y = g + oy
                lastC2 = c2
            case "s":
                guard let c = number(), let e = number(), let f = number(), let g = number() else { return out }
                let ox = relative ? x : 0, oy = relative ? y : 0
                let c1 = lastC2.map { (2 * x - $0.0, 2 * y - $0.1) } ?? (x, y)
                let c2 = (c + ox, e + oy)
                out.append(.curve(x: f + ox, y: g + oy, c1x: c1.0, c1y: c1.1, c2x: c2.0, c2y: c2.1))
                x = f + ox
                y = g + oy
                lastC2 = c2
            case "a":
                guard let rx = number(), let ry = number(), let rot = number(), let large = flag(), let sweep = flag(),
                      let a = number(), let b = number() else { return out }
                let endX = relative ? x + a : a, endY = relative ? y + b : b
                out += arc(fromX: x, fromY: y, rx: rx, ry: ry, rotation: rot, large: large, sweep: sweep, toX: endX, toY: endY)
                x = endX
                y = endY
                lastC2 = nil
            default:
                // A command these icons don't use: stop rather than draw nonsense.
                return out
            }
        }
        return out
    }

    /// An elliptical arc as cubic curves (SVG implementation notes, F.6).
    private static func arc(fromX x1: Double, fromY y1: Double, rx rxIn: Double, ry ryIn: Double, rotation: Double, large: Bool, sweep: Bool, toX x2: Double, toY y2: Double) -> [PathCommand] {
        if x1 == x2, y1 == y2 { return [] }
        var rx = abs(rxIn), ry = abs(ryIn)
        if rx == 0 || ry == 0 { return [.line(x2, y2)] }
        let phi = rotation * .pi / 180
        let cosP = cos(phi), sinP = sin(phi)
        let dx = (x1 - x2) / 2, dy = (y1 - y2) / 2
        let xp = cosP * dx + sinP * dy, yp = -sinP * dx + cosP * dy
        let lambda = xp * xp / (rx * rx) + yp * yp / (ry * ry)
        if lambda > 1 {
            rx *= lambda.squareRoot()
            ry *= lambda.squareRoot()
        }
        let num = rx * rx * ry * ry - rx * rx * yp * yp - ry * ry * xp * xp
        let den = rx * rx * yp * yp + ry * ry * xp * xp
        let co = (large == sweep ? -1.0 : 1.0) * (max(0, num / den)).squareRoot()
        let cxp = co * rx * yp / ry, cyp = -co * ry * xp / rx
        let cx = cosP * cxp - sinP * cyp + (x1 + x2) / 2
        let cy = sinP * cxp + cosP * cyp + (y1 + y2) / 2
        func angle(_ ux: Double, _ uy: Double, _ vx: Double, _ vy: Double) -> Double { atan2(ux * vy - uy * vx, ux * vx + uy * vy) }
        let ux = (xp - cxp) / rx, uy = (yp - cyp) / ry
        let vx = (-xp - cxp) / rx, vy = (-yp - cyp) / ry
        let start = angle(1, 0, ux, uy)
        var delta = angle(ux, uy, vx, vy)
        if !sweep, delta > 0 { delta -= 2 * .pi }
        if sweep, delta < 0 { delta += 2 * .pi }

        let segments = max(1, Int((abs(delta) / (.pi / 2)).rounded(.up)))
        let step = delta / Double(segments)
        let t = 4.0 / 3.0 * tan(step / 4)
        func point(_ px: Double, _ py: Double) -> (Double, Double) {
            let ex = rx * px, ey = ry * py
            return (cosP * ex - sinP * ey + cx, sinP * ex + cosP * ey + cy)
        }
        var out: [PathCommand] = []
        var a = start
        for n in 0..<segments {
            let b = a + step
            let c1 = point(cos(a) - t * sin(a), sin(a) + t * cos(a))
            let c2 = point(cos(b) + t * sin(b), sin(b) - t * cos(b))
            // Land exactly on the end point, or the next relative command drifts.
            let end = n == segments - 1 ? (x2, y2) : point(cos(b), sin(b))
            out.append(.curve(x: end.0, y: end.1, c1x: c1.0, c1y: c1.1, c2x: c2.0, c2y: c2.1))
            a = b
        }
        return out
    }
}
