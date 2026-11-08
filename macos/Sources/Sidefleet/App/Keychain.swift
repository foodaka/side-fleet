import Foundation
import Security
import SidefleetKit

/// The credential lives in the Keychain, never in preferences.
enum Keychain {
    private static let service = "xyz.sidefleet.mac"
    private static let account = "control-plane"

    private static var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
    }

    static func load() -> Credential? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data, let s = String(data: data, encoding: .utf8) else { return nil }
        if s.hasPrefix("key:") { return .devKey(String(s.dropFirst(4))) }
        if s.hasPrefix("session:") { return .session(String(s.dropFirst(8))) }
        return nil
    }

    static func save(_ credential: Credential) {
        clear()
        let s: String
        switch credential {
        case .devKey(let k): s = "key:\(k)"
        case .session(let v): s = "session:\(v)"
        }
        var q = query
        q[kSecValueData as String] = Data(s.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(q as CFDictionary, nil)
    }

    static func clear() { SecItemDelete(query as CFDictionary) }
}
