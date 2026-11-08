import AppKit
import SidefleetKit
import SwiftUI
import WebKit

/// Shown when nobody is signed in: GitHub sign-in on a hosted control plane, the developer key on a
/// self-hosted one (both when both are configured).
struct SignInScreen: View {
    var session: AppSession
    @State private var address = ""
    @State private var methods: AuthMethods?
    @State private var checked: URL?
    @State private var checking = false
    @State private var error: String?
    @State private var key = ""
    @State private var showKey = false
    @State private var busy = false
    @State private var signingInWithGitHub = false
    @FocusState private var addressFocused: Bool

    private static let hosted = "https://app.sidefleet.xyz"

    private var parsedURL: URL? {
        var s = address.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty else { return nil }
        if !s.contains("://") { s = (s.hasPrefix("localhost") || s.hasPrefix("127.") || s.hasPrefix("100.") ? "http://" : "https://") + s }
        while s.hasSuffix("/") { s.removeLast() }
        guard let url = URL(string: s), url.host != nil, ["http", "https"].contains(url.scheme ?? "") else { return nil }
        return url
    }

    /// Plain HTTP is normal on a tailnet or at home; on the open internet your key would travel in the clear.
    private var insecure: Bool {
        guard let url = parsedURL, url.scheme == "http", let host = url.host else { return false }
        let local = host == "localhost" || host.hasPrefix("127.") || host.hasPrefix("10.") || host.hasPrefix("192.168.") || host.hasPrefix("100.") || host.hasSuffix(".local") || host.hasSuffix(".ts.net")
        return !local
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(spacing: 8) {
                LogoMark(size: 20, animated: checking || busy)
                Text("Sidefleet").font(SF.font(15, .semibold)).tracking(-0.2)
            }
            VStack(alignment: .leading, spacing: 8) {
                Text(methods?.github == true ? "Sign in to Sidefleet" : "Connect to your control plane")
                    .font(SF.font(24, .medium))
                    .tracking(-0.3)
                Text(methods?.github == true
                    ? "Run coding agents on your own machines, from this Mac. Your machines keep their own Claude Code sign-in; Sidefleet never sees an API key."
                    : "Where your Sidefleet runs: the hosted one, or the address of your own `sidefleet serve`.")
                    .font(SF.font(13))
                    .lineSpacing(3)
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            addressField
            if let methods, checked == parsedURL {
                if methods.github { githubSection(methods) }
                if methods.devKey, !methods.github || showKey { keySection(primary: !methods.github) }
                if !methods.github, !methods.devKey {
                    Text("This control plane has no way in configured.").font(SF.font(12)).foregroundStyle(SF.errInk)
                }
            }
            if let error {
                Text(error).font(SF.font(12)).foregroundStyle(SF.errInk).fixedSize(horizontal: false, vertical: true).accessibilityAddTraits(.isStaticText)
            }
        }
        .frame(width: 400)
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SF.background)
        .onAppear {
            address = session.savedURL.isEmpty ? Self.hosted : session.savedURL
            addressFocused = true
            check()
        }
        .sheet(isPresented: $signingInWithGitHub) {
            if let url = checked {
                GitHubSignInSheet(base: url) { cookie in
                    signingInWithGitHub = false
                    finish(url: url, credential: .session(cookie))
                }
            }
        }
    }

    private var addressField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Control plane").font(SF.font(12, .medium))
            HStack(spacing: 8) {
                TextField("https://sidefleet.example.com", text: $address)
                    .textFieldStyle(.roundedBorder)
                    .font(SF.mono(13))
                    .focused($addressFocused)
                    .onSubmit(check)
                Button(checking ? "Checking…" : "Continue", action: check)
                    .buttonStyle(SecondaryButtonStyle(height: 26))
                    .disabled(parsedURL == nil || checking)
            }
            if insecure {
                Label("This address isn't encrypted, so your key would travel in the clear. Use HTTPS or a private network.", systemImage: "exclamationmark.triangle")
                    .font(SF.font(12))
                    .foregroundStyle(SF.queueInk)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func githubSection(_ methods: AuthMethods) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                signingInWithGitHub = true
            } label: {
                Text("Continue with GitHub").frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryButtonStyle(height: 40))
            Text("We read your GitHub login and email to create your account and a personal org. No repository access is requested.")
                .font(SF.font(12))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            if methods.devKey, !showKey {
                Button("Use a developer key instead") { showKey = true }
                    .buttonStyle(.plain)
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
            }
        }
    }

    private func keySection(primary: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Developer key").font(SF.font(12, .medium))
            SecureField("SIDEFLEET_DEV_KEY", text: $key)
                .textFieldStyle(.roundedBorder)
                .font(SF.mono(13))
                .onSubmit(submitKey)
            Text("`sidefleet serve` printed it when it started. It is also in `~/.sidefleet/server.env` on that machine, and it stays in this Mac's Keychain.")
                .font(SF.font(12))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            Button(action: submitKey) {
                Text(busy ? "Connecting…" : "Connect").frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryButtonStyle(height: 40))
            .disabled(busy || key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }

    private func check() {
        guard let url = parsedURL else { return }
        checking = true
        error = nil
        Task {
            defer { checking = false }
            do {
                methods = try await API.authMethods(at: url)
                checked = url
            } catch {
                methods = nil
                checked = nil
                self.error = "Couldn't reach \(url.host ?? "that address"): \(error.localizedDescription)"
            }
        }
    }

    private func submitKey() {
        guard let url = checked, !key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        finish(url: url, credential: .devKey(key.trimmingCharacters(in: .whitespacesAndNewlines)))
    }

    private func finish(url: URL, credential: Credential) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await session.signIn(url: url, credential: credential)
            } catch APIError.unauthorized {
                error = "That key was not accepted. Check SIDEFLEET_DEV_KEY on the control plane."
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}

/// GitHub's sign-in in an embedded view: the control plane finishes it by setting an httpOnly cookie
/// on itself, which only a view we own can read. Its store is thrown away afterwards.
private struct GitHubSignInSheet: View {
    var base: URL
    var onSession: (String) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Sign in with GitHub").font(SF.font(13, .medium))
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle(height: 26)).keyboardShortcut(.cancelAction)
            }
            .padding(12)
            Divider()
            SignInWebView(start: base.appendingPathComponent("auth/github/login"), host: base.host ?? "", onSession: onSession)
        }
        .frame(width: 520, height: 680)
    }
}

private struct SignInWebView: NSViewRepresentable {
    var start: URL
    var host: String
    var onSession: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = context.coordinator
        view.load(URLRequest(url: start))
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {}

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var parent: SignInWebView
        private var done = false

        init(_ parent: SignInWebView) { self.parent = parent }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            guard !done else { return }
            webView.configuration.websiteDataStore.httpCookieStore.getAllCookies { [weak self] cookies in
                MainActor.assumeIsolated {
                    guard let self, !self.done else { return }
                    if let c = cookies.first(where: { $0.name == "sf_session" && $0.domain.contains(self.parent.host) }), !c.value.isEmpty {
                        self.done = true
                        self.parent.onSession(c.value)
                    }
                }
            }
        }
    }
}
