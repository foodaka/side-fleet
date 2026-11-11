import SidefleetKit
import SwiftUI

/// Chat apps: connect the assistant you already chat with (Hermes, OpenClaw, Claude Code) so it can
/// run and check tasks on the fleet. Three steps on one card: pick the app, pick what it may do, add
/// the setup it gets; then the screen waits until the app first calls in, and offers updates in chat.
/// Each connection is a personal API token.
struct ChatAppsScreen: View {
    @Environment(Stores.self) private var stores
    @State private var disconnecting: ApiToken?

    private var store: ChatAppsStore { stores.chatApps }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                ScreenHeader(
                    title: "Chat apps",
                    detail: "Run and check tasks from Telegram, WhatsApp, Discord or wherever you chat, through the AI assistant you already use. Tell it \u{201C}fix the failing test in my web repo\u{201D} and it starts the task on one of your machines, then tells you how it went."
                )
                if store.tokens == nil {
                    Text("Loading…").font(SF.font(13)).foregroundStyle(SF.muted)
                } else {
                    if !store.listed.isEmpty { connected }
                    if store.showSetup { setup }
                }
            }
            .column()
            .padding(.horizontal, 28)
            .padding(.top, 20)
            .padding(.bottom, 28)
            .animation(.easeOut(duration: 0.2), value: store.showSetup)
        }
        .background(SF.background)
        // The connections are per org. While one is being set up, ask again until the app first calls in.
        .task(id: stores.fleet.org?.id) {
            guard stores.fleet.org != nil else { return }
            store.reset()
            await store.load()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(3))
                if store.waiting { await store.load() }
            }
        }
        .alert("Disconnect \(disconnecting?.name ?? "it")?", isPresented: Binding(get: { disconnecting != nil }, set: { if !$0 { disconnecting = nil } })) {
            Button("Disconnect", role: .destructive) {
                if let t = disconnecting { Task { await store.disconnect(t) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("It can't reach your fleet from its next message. Tasks it started keep running.")
        }
    }

    // MARK: Connected

    private var connected: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .bottom) {
                Text("Your chat apps").font(SF.font(15, .semibold))
                Spacer()
                if !store.showSetup {
                    Button("Connect another") { store.adding = true }.buttonStyle(SecondaryButtonStyle())
                }
            }
            VStack(spacing: 0) {
                ForEach(Array(store.listed.enumerated()), id: \.element.id) { i, t in
                    if i > 0 { Divider() }
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 16) {
                            VStack(alignment: .leading, spacing: 1) {
                                Text(t.name).font(SF.font(13, .medium)).lineLimit(1)
                                Text(ChatApps.access(of: t.scope).short).font(SF.font(12)).foregroundStyle(SF.muted)
                            }
                            Spacer(minLength: 0)
                            Text(t.lastUsedAt != nil ? "Last active \(Format.rel(t.lastUsedAt, now: stores.fleet.now))" : "Not connected yet")
                                .font(SF.font(12))
                                .foregroundStyle(SF.muted)
                            Button("Disconnect") { disconnecting = t }
                                .buttonStyle(LinkButtonStyle(color: SF.errInk, hoverColor: SF.err))
                                .font(SF.font(12))
                        }
                        let app = ChatApps.appOf(name: t.name)
                        if ChatApps.hasUpdates(app) {
                            ChatUpdatesView(token: t, app: app, open: false).id(t.id)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                }
            }
            .card()
        }
    }

    // MARK: Setup

    private var setup: some View {
        VStack(alignment: .leading, spacing: 24) {
            HStack(alignment: .top) {
                Text("Connect a chat app").font(SF.font(15, .semibold))
                Spacer()
                if !store.live.isEmpty, store.fresh == nil {
                    Button("Cancel") { store.reset() }.buttonStyle(LinkButtonStyle()).font(SF.font(12))
                }
            }
            step(1, on: true, title: "Which assistant do you chat with?") {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 136), spacing: 8)], spacing: 8) {
                    ForEach(ChatApps.apps) { a in
                        Tile(on: store.app == a.id, disabled: store.fresh != nil, action: { store.pick(a.id) }) {
                            Text(a.name).font(SF.font(13, .medium))
                            Text(a.hint).font(SF.font(12)).foregroundStyle(SF.muted).multilineTextAlignment(.leading)
                        }
                    }
                }
            }
            step(2, on: store.app != nil, title: "What can it do?") {
                if store.app != nil {
                    VStack(spacing: 8) {
                        ForEach(ChatApps.access) { a in
                            Tile(on: store.scope == a.scope, disabled: store.fresh != nil, action: { store.scope = a.scope }) {
                                HStack(spacing: 8) {
                                    Text(a.name).font(SF.font(13, .medium))
                                    if a.scope == "tasks" {
                                        Text("Recommended")
                                            .font(SF.font(11))
                                            .foregroundStyle(SF.okInk)
                                            .padding(.horizontal, 8)
                                            .padding(.vertical, 2)
                                            .background(SF.okSoft, in: Capsule())
                                    }
                                }
                                Text(a.hint).font(SF.font(12)).foregroundStyle(SF.muted).multilineTextAlignment(.leading).fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                    if store.fresh == nil, let chosen = store.chosen {
                        Button {
                            Task { await store.connect() }
                        } label: {
                            HStack(spacing: 6) {
                                if store.busy { ProgressView().controlSize(.small) }
                                Text("Connect \(chosen.id == .other ? "it" : chosen.name)")
                            }
                        }
                        .buttonStyle(PrimaryButtonStyle(height: 34))
                        .disabled(store.busy)
                    }
                }
            }
            step(3, on: store.fresh != nil, title: "Add Sidefleet to \(store.chosen.map { $0.id == .other ? "your app" : $0.name } ?? "your app")") {
                add
            }
            if let chosen = store.chosen, ChatApps.hasUpdates(chosen.id), let current = store.current {
                step(4, on: store.connected, title: "Let \(chosen.id == .other ? "it" : chosen.name) message you", note: "Optional") {
                    if store.connected {
                        ChatUpdatesView(token: current, app: chosen.id, open: true).id(current.id)
                        Button(current.updates != nil ? "Done" : "Skip for now") { store.reset() }
                            .buttonStyle(LinkButtonStyle())
                            .font(SF.font(12))
                    } else {
                        Text("Once it's connected, it can also tell you when a task finishes or needs your OK.")
                            .font(SF.font(13))
                            .foregroundStyle(SF.muted)
                    }
                }
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SF.card.opacity(0.6), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(SF.border))
    }

    @ViewBuilder private var add: some View {
        if let fresh = store.fresh, let token = fresh.token, let chosen = store.chosen, !store.connected {
            if chosen.canSelfSetup {
                HowTabs(how: Binding(get: { store.how }, set: { store.how = $0 }), name: chosen.name)
            }
            if store.how == .ask, chosen.canSelfSetup {
                Text("Send \(chosen.name) this message. It adds Sidefleet to its own settings.").font(SF.font(13)).foregroundStyle(SF.muted)
                CopyBlock(shown: ChatApps.setupMessage(chosen.id, token: token, control: stores.controlURL), label: "Copy message")
            } else {
                let steps = ChatApps.setupSteps(chosen.id)
                Text(steps.before).font(SF.font(13)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
                CopyBlock(shown: ChatApps.setupSnippet(chosen.id, token: token, control: stores.controlURL))
                if !steps.after.isEmpty {
                    Text(steps.after).font(SF.font(13)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
                }
            }
            Text("This includes a private key that lets the app use your fleet as you. It's shown only now: if you lose it, connect again. You can disconnect the app here at any time.")
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        if let fresh = store.fresh, let chosen = store.chosen {
            if store.connected {
                VStack(alignment: .leading, spacing: 8) {
                    Label("\(fresh.name) is connected.", systemImage: "checkmark").font(SF.font(13, .medium))
                    Text("Try asking it:").font(SF.font(13)).foregroundStyle(SF.muted)
                    FlowRow(spacing: 6, lineSpacing: 6) {
                        ForEach(ChatApps.examples, id: \.self) { e in
                            Text("\u{201C}\(e)\u{201D}")
                                .font(SF.font(12.5))
                                .padding(.horizontal, 12)
                                .padding(.vertical, 4)
                                .background(SF.card, in: Capsule())
                                .overlay(Capsule().strokeBorder(SF.border))
                        }
                    }
                    if !ChatApps.hasUpdates(chosen.id) {
                        Button("Done") { store.reset() }.buttonStyle(LinkButtonStyle()).font(SF.font(12)).padding(.top, 2)
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(SF.okSoft.opacity(0.5), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.ok.opacity(0.4)))
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for \(chosen.id == .other ? "your app" : chosen.name) to connect…").font(SF.font(13)).foregroundStyle(SF.muted)
                    Spacer(minLength: 0)
                    Button("Finish later") { store.reset() }.buttonStyle(LinkButtonStyle()).font(SF.font(12))
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            }
        }
    }

    private func step<Content: View>(_ n: Int, on: Bool, title: String, note: String? = nil, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                Text("\(n)")
                    .font(SF.font(12, .semibold))
                    .foregroundStyle(on ? SF.inkInverse : SF.muted)
                    .frame(width: 24, height: 24)
                    .background(on ? SF.ink : SF.fill, in: Circle())
                Text(title).font(SF.font(14, .medium))
                if let note { Text(note).font(SF.font(12)).foregroundStyle(SF.muted) }
            }
            VStack(alignment: .leading, spacing: 12) { content() }
                .padding(.leading, 36)
        }
        .opacity(on ? 1 : 0.5)
    }
}

/// One choice among a few, as a card: selected is ringed in ink.
private struct Tile<Content: View>: View {
    var on: Bool
    var disabled: Bool
    var action: () -> Void
    @ViewBuilder var content: () -> Content
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 2) { content() }
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, minHeight: 56, alignment: .topLeading)
                .background(SF.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(on ? SF.ink : hovering ? SF.ink.opacity(0.4) : SF.border, lineWidth: on ? 2 : 1))
                .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                .onHover { hovering = $0 }
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.6 : 1)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// "Ask it to do it" or "Do it yourself".
private struct HowTabs: View {
    @Binding var how: ChatAppsStore.How
    var name: String

    var body: some View {
        HStack(spacing: 0) {
            tab("Ask \(name) to do it", .ask)
            tab("Do it yourself", .yourself)
        }
        .padding(2)
        .background(SF.fill, in: Capsule())
        .fixedSize()
    }

    private func tab(_ title: String, _ value: ChatAppsStore.How) -> some View {
        Button {
            how = value
        } label: {
            Text(title)
                .font(SF.font(12.5, how == value ? .medium : .regular))
                .foregroundStyle(how == value ? SF.ink : SF.muted)
                .padding(.horizontal, 12)
                .padding(.vertical, 4)
                .background(how == value ? SF.background : .clear, in: Capsule())
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

/// Updates in chat for one connected app: Sidefleet tells the app when a task finishes, fails, needs
/// an OK or gets a pull request, and the app messages you about it. Off: an offer to set it up.
/// Setting up: where the app listens, then what to add on its side, then a test. On: how the last
/// update went, with Send a test, Change and Turn off.
private struct ChatUpdatesView: View {
    var token: ApiToken
    var app: ChatAppID
    /// In the setup steps: opened for you once, and the step's own Done finishes.
    var open: Bool
    @Environment(Stores.self) private var stores
    @State private var model: ChatUpdatesModel?
    @State private var turningOff = false

    var body: some View {
        // A stack, not a Group: modifiers on an empty Group never run, and the model is made in `.task`.
        VStack(alignment: .leading, spacing: 12) {
            if let model { content(model) }
        }
        .task(id: token.id) {
            let m = ChatUpdatesModel(tokenID: token.id, app: app, fleet: stores.fleet)
            if open { m.openOnce(on: token.updates) }
            model = m
        }
        .alert("Stop updates in chat?", isPresented: $turningOff) {
            Button("Turn off", role: .destructive) {
                Task {
                    if await model?.turnOff() == true { await stores.chatApps.load() }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("\(model?.capitalName ?? "It") won't hear about tasks from Sidefleet any more. It can still run and check them.")
        }
    }

    private func test(_ model: ChatUpdatesModel) {
        Task {
            await model.sendTest()
            await stores.chatApps.load()
        }
    }

    @ViewBuilder private func content(_ model: ChatUpdatesModel) -> some View {
        if model.stage == .idle {
            if let on = token.updates {
                FlowRow(spacing: 12, lineSpacing: 4) {
                    if let error = on.lastError {
                        Text("Updates in chat: the last one didn't arrive. \(error)").foregroundStyle(SF.errInk)
                    } else {
                        HStack(spacing: 4) {
                            Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold)).foregroundStyle(SF.okInk)
                            Text("Updates in chat are on" + (on.lastAt != nil ? ", last sent \(Format.rel(on.lastAt, now: stores.fleet.now))" : ""))
                        }
                        .foregroundStyle(SF.muted)
                    }
                    Button("Send a test") { test(model) }.buttonStyle(LinkButtonStyle()).disabled(model.busy)
                    Button("Change") { model.edit(on: token.updates) }.buttonStyle(LinkButtonStyle())
                    Button("Turn off") { turningOff = true }.buttonStyle(LinkButtonStyle())
                    if let t = model.test { Text(t.text).foregroundStyle(t.ok ? SF.okInk : SF.errInk) }
                }
                .font(SF.font(12))
            } else {
                Button("Get updates in chat: \(model.name) tells you when a task finishes or needs your OK →") { model.edit(on: nil) }
                    .buttonStyle(LinkButtonStyle())
                    .font(SF.font(12))
            }
        } else {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Get updates in chat").font(SF.font(13, .medium))
                    Text("\(model.capitalName) messages you when a task finishes, fails or needs your OK to run a command, and you answer right there: \u{201C}yes, open a PR\u{201D}.")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if model.stage == .form {
                    form(model)
                } else {
                    instructions(model)
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .card()
        }
    }

    @ViewBuilder private func form(_ model: ChatUpdatesModel) -> some View {
        @Bindable var model = model
        let hosted = stores.fleet.setup?.hosted ?? false
        VStack(alignment: .leading, spacing: 6) {
            Text(app == .other ? "Address to send updates to" : "Address of the computer running \(model.capitalName)").font(SF.font(13))
            TextField(ChatApps.updatesPlaceholder(app), text: $model.address)
                .font(SF.font(13))
                .modifier(BoxedField())
                .autocorrectionDisabled()
                .accessibilityLabel("Address")
            Text((app == .hermes ? "Hermes listens for updates on port 8644. " : app == .openclaw ? "OpenClaw listens on its gateway's port, 18789 unless you changed it. " : "")
                + (hosted
                    ? "Sidefleet needs to reach it from the internet. If it runs at home, a tunnel such as Tailscale Funnel or Cloudflare Tunnel gives it an address."
                    : "Sidefleet needs to reach it: a local or Tailscale address is fine if both are on the same network."))
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        if app == .hermes {
            VStack(alignment: .leading, spacing: 6) {
                Text("Where do you chat with Hermes?").font(SF.font(13))
                FlowRow(spacing: 6, lineSpacing: 6) {
                    ForEach(ChatApps.platforms, id: \.id) { p in
                        Button {
                            model.platform = p.id
                        } label: {
                            Text(p.name)
                                .font(SF.font(12.5))
                                .foregroundStyle(model.platform == p.id ? SF.inkInverse : SF.ink)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 4)
                                .background(model.platform == p.id ? SF.ink : .clear, in: Capsule())
                                .overlay(Capsule().strokeBorder(model.platform == p.id ? SF.ink : SF.border))
                                .contentShape(Capsule())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        Toggle("Every task, not only the ones \(model.name) started or replied to", isOn: $model.allTasks)
            .toggleStyle(.checkbox)
            .font(SF.font(13))
        HStack(spacing: 8) {
            Button {
                Task {
                    await model.save()
                    await stores.chatApps.load()
                }
            } label: {
                HStack(spacing: 6) {
                    if model.busy { ProgressView().controlSize(.small) }
                    Text("Next")
                }
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(model.busy || model.address.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("Cancel") { model.stage = .idle }.buttonStyle(SecondaryButtonStyle())
        }
    }

    @ViewBuilder private func instructions(_ model: ChatUpdatesModel) -> some View {
        if app != .other {
            HowTabs(how: Binding(get: { model.how }, set: { model.how = $0 }), name: model.capitalName)
        }
        if model.how == .ask, app != .other {
            Text("Send \(model.capitalName) this message:").font(SF.font(13)).foregroundStyle(SF.muted)
            CopyBlock(shown: model.message, label: "Copy message")
        } else {
            let setup = model.setup
            Text(setup.before).font(SF.font(13)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
            CopyBlock(shown: setup.snippet)
            if !setup.after.isEmpty { Text(setup.after).font(SF.font(13)).foregroundStyle(SF.muted) }
        }
        HStack(spacing: 12) {
            Button {
                test(model)
            } label: {
                HStack(spacing: 6) {
                    if model.busy { ProgressView().controlSize(.small) }
                    Text("Send a test update")
                }
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(model.busy)
            if !open {
                Button(model.test?.ok == true ? "Done" : "Finish later") { model.stage = .idle }
                    .buttonStyle(LinkButtonStyle())
                    .font(SF.font(12))
            }
        }
        if let t = model.test {
            Text(t.text).font(SF.font(13)).foregroundStyle(t.ok ? SF.okInk : SF.errInk).fixedSize(horizontal: false, vertical: true)
        }
    }
}
