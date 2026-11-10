import AppKit
import SwiftUI

/// GitHub pictures by login, fetched once and kept while the app runs.
@MainActor
final class AvatarCache {
    static let shared = AvatarCache()

    private var images: [String: NSImage] = [:]
    private var missing: Set<String> = []
    private var loading: [String: Task<NSImage?, Never>] = [:]
    private let session: URLSession = {
        // No cookies and nothing on disk: this is a public picture, and GitHub needn't learn more than the login.
        let config = URLSessionConfiguration.ephemeral
        config.httpShouldSetCookies = false
        config.httpCookieAcceptPolicy = .never
        config.timeoutIntervalForRequest = 15
        return URLSession(configuration: config)
    }()

    func cached(_ login: String) -> NSImage? { images[login.lowercased()] }

    func image(for login: String) async -> NSImage? {
        let key = login.lowercased()
        if let image = images[key] { return image }
        if missing.contains(key) { return nil }
        if let task = loading[key] { return await task.value }
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-"))
        guard let name = login.addingPercentEncoding(withAllowedCharacters: allowed),
              let url = URL(string: "https://github.com/\(name).png?size=128") else { return nil }
        let session = session
        let task = Task<NSImage?, Never> {
            guard let (data, response) = try? await session.data(from: url),
                  (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
            return NSImage(data: data)
        }
        loading[key] = task
        let image = await task.value
        loading[key] = nil
        if let image {
            images[key] = image
        } else {
            missing.insert(key)
        }
        return image
    }
}

/// A teammate's GitHub picture, with their initial while it loads or if it can't.
struct Avatar: View {
    var login: String?
    var size: CGFloat = 20
    /// Off for made-up logins (the demo), which must not be looked up.
    var loads = true
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            Circle().fill(SF.ink.opacity(0.1))
            Text((login ?? "?").prefix(1).uppercased())
                .font(.system(size: max(9, (size * 0.45).rounded()), weight: .semibold))
                .foregroundStyle(SF.ink.opacity(0.7))
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fill)
                    .transition(.opacity)
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(SF.background, lineWidth: 1))
        .help(login.map { "@\($0)" } ?? "")
        .accessibilityLabel(login.map { "@\($0)" } ?? "Teammate")
        .task(id: login) {
            image = nil
            guard loads, let login, !login.isEmpty else { return }
            if let cached = AvatarCache.shared.cached(login) {
                image = cached
                return
            }
            let loaded = await AvatarCache.shared.image(for: login)
            withAnimation(.easeOut(duration: 0.15)) { image = loaded }
        }
    }
}

/// A command, config snippet or message to copy in full. It wraps, so the whole thing is visible.
struct CopyBlock: View {
    var shown: String
    var copy: String?
    var label = "Copy"

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text(shown)
                .font(SF.mono(12))
                .lineSpacing(4)
                .foregroundStyle(Color(rgb: 0xececE8))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            CopyButton(text: { copy ?? shown }, label: label)
                .environment(\.colorScheme, .dark)
        }
        .padding(.leading, 14)
        .padding(.trailing, 8)
        .padding(.vertical, 8)
        .background(Color.adaptive(0x191919, 0x0c0c0b), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Color.white.opacity(0.08)))
    }
}

/// A title, a line under it and whatever sits at its right: the top of a screen.
struct ScreenHeader<Trailing: View>: View {
    var title: String
    var detail: String
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack(alignment: .bottom, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(SF.font(22, .semibold)).tracking(-0.3)
                Text(detail)
                    .font(SF.font(13))
                    .lineSpacing(3)
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: 560, alignment: .leading)
            Spacer(minLength: 0)
            trailing()
        }
    }
}

extension ScreenHeader where Trailing == EmptyView {
    init(title: String, detail: String) {
        self.init(title: title, detail: detail) { EmptyView() }
    }
}

/// A plain text link in the muted ink that darkens under the pointer.
struct LinkButtonStyle: ButtonStyle {
    var color: Color = SF.muted
    var hoverColor: Color = SF.ink

    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, color: color, hoverColor: hoverColor)
    }

    private struct Face: View {
        var configuration: Configuration
        var color: Color
        var hoverColor: Color
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            configuration.label
                .foregroundStyle(hovering && isEnabled ? hoverColor : color)
                .contentShape(Rectangle())
                .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.5)
                .onHover { hovering = $0 }
        }
    }
}
