import Foundation

/// RFC 3339 times, as Rust's `DateTime<Utc>` writes them: `2026-11-07T07:24:01.123456Z`, with any
/// number of fractional digits, or an offset instead of `Z`. Foundation's formatters are fussy about
/// the digits, and this runs for every row of every list.
public enum Timestamp {
    public static func parse(_ s: String) -> Date? {
        let u = Array(s.utf8)
        func digits(_ at: Int, _ n: Int) -> Int? {
            guard at + n <= u.count else { return nil }
            var v = 0
            for b in u[at..<at + n] {
                guard b >= 48, b <= 57 else { return nil }
                v = v * 10 + Int(b - 48)
            }
            return v
        }
        guard u.count >= 19, u[4] == 45, u[7] == 45, u[10] == 84 || u[10] == 116 || u[10] == 32, u[13] == 58, u[16] == 58,
              let year = digits(0, 4), let month = digits(5, 2), let day = digits(8, 2),
              let hour = digits(11, 2), let minute = digits(14, 2), let second = digits(17, 2),
              (1...12).contains(month), (1...31).contains(day), hour < 24, minute < 60, second <= 60
        else { return nil }

        var i = 19
        var fraction = 0.0
        if i < u.count, u[i] == 46 {
            i += 1
            var scale = 0.1
            let start = i
            while i < u.count, u[i] >= 48, u[i] <= 57 {
                fraction += Double(u[i] - 48) * scale
                scale /= 10
                i += 1
            }
            if i == start { return nil }
        }

        var offset = 0
        if i < u.count {
            switch u[i] {
            case 90, 122: // Z
                i += 1
            case 43, 45: // + or -
                let sign = u[i] == 43 ? 1 : -1
                guard let h = digits(i + 1, 2) else { return nil }
                var m = 0
                if i + 3 < u.count {
                    let at = u[i + 3] == 58 ? i + 4 : i + 3
                    guard let mm = digits(at, 2) else { return nil }
                    m = mm
                    i = at + 2
                } else {
                    i += 3
                }
                offset = sign * (h * 3600 + m * 60)
            default:
                return nil
            }
        } else {
            return nil // no zone: refuse to guess
        }
        guard i == u.count else { return nil }

        let days = daysFromCivil(year, month, day)
        let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset
        return Date(timeIntervalSince1970: Double(secs) + fraction)
    }

    /// The form the control plane takes: whole seconds in UTC.
    public static func format(_ date: Date) -> String {
        let secs = Int(date.timeIntervalSince1970.rounded(.down))
        let days = secs >= 0 ? secs / 86_400 : (secs - 86_399) / 86_400
        let rem = secs - days * 86_400
        let (y, m, d) = civilFromDays(days)
        func p(_ n: Int, _ w: Int = 2) -> String {
            let s = String(n)
            return String(repeating: "0", count: max(0, w - s.count)) + s
        }
        return "\(p(y, 4))-\(p(m))-\(p(d))T\(p(rem / 3600)):\(p(rem % 3600 / 60)):\(p(rem % 60))Z"
    }

    // Howard Hinnant's civil calendar algorithms.
    private static func daysFromCivil(_ year: Int, _ m: Int, _ d: Int) -> Int {
        let y = m <= 2 ? year - 1 : year
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        return era * 146_097 + doe - 719_468
    }

    private static func civilFromDays(_ z0: Int) -> (Int, Int, Int) {
        let z = z0 + 719_468
        let era = (z >= 0 ? z : z - 146_096) / 146_097
        let doe = z - era * 146_097
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365
        let y = yoe + era * 400
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
        let mp = (5 * doy + 2) / 153
        let d = doy - (153 * mp + 2) / 5 + 1
        let m = mp + (mp < 10 ? 3 : -9)
        return (m <= 2 ? y + 1 : y, m, d)
    }
}
