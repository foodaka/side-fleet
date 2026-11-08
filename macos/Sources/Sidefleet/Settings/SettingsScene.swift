import SidefleetKit
import SwiftUI

/// How the app behaves for you. Every change applies at once.
struct SettingsView: View {
    var session: AppSession

    var body: some View {
        if let stores = session.stores {
            TabView {
                NewTaskSettings().tabItem { Label("New tasks", systemImage: "plus.square") }
                SendingSettings().tabItem { Label("Sending", systemImage: "paperplane") }
                ReviewSettings().tabItem { Label("Reviewing", systemImage: "doc.text.magnifyingglass") }
                SnoozeSettings().tabItem { Label("Snooze", systemImage: "alarm") }
                GeneralSettings().tabItem { Label("General", systemImage: "gearshape") }
                AccountSettings(session: session).tabItem { Label("Account", systemImage: "person.crop.circle") }
            }
            .environment(stores)
            .frame(width: 520)
            .preferredColorScheme(stores.prefs.appearance.colorScheme)
        } else {
            Text("Sign in to change settings.").font(SF.font(13)).foregroundStyle(SF.muted).frame(width: 420, height: 160)
        }
    }
}

extension Appearance {
    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

@MainActor
private func pref<T>(_ prefs: PrefsStore, _ path: ReferenceWritableKeyPath<PrefsStore, T>) -> Binding<T> {
    Binding(get: { prefs[keyPath: path] }, set: { prefs.set(path, $0) })
}

private struct LabelledHint: View {
    var title: String
    var hint: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
            Text(hint).font(SF.font(12)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct NewTaskSettings: View {
    @Environment(Stores.self) private var stores

    var body: some View {
        Form {
            Picker(selection: pref(stores.prefs, \.permission)) {
                ForEach(Format.permissions) { Text($0.name).tag($0.id) }
            } label: {
                LabelledHint(title: "Permissions", hint: Format.permissions.first { $0.id == stores.prefs.permission }?.hint ?? "")
            }
            Picker(selection: pref(stores.prefs, \.workspace)) {
                ForEach(Format.workspaces) { Text($0.name).tag($0.id) }
            } label: {
                LabelledHint(title: "Workspace", hint: Format.workspaces.first { $0.id == stores.prefs.workspace }?.hint ?? "")
            }
            Picker(selection: pref(stores.prefs, \.roomPermission)) {
                ForEach(Format.permissions) { Text($0.name).tag($0.id) }
            } label: {
                LabelledHint(title: "In a room", hint: "A task started in a room reads teammates' notes, so it starts narrower.")
            }
            Text(stores.prefs.account ? "Kept with your account, so every device starts the same. The composer can change them for one task." : "The composer can change these for one task.")
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
        }
        .formStyle(.grouped)
        .frame(height: 280)
    }
}

private struct SendingSettings: View {
    @Environment(Stores.self) private var stores
    @State private var tryAs = MessageBox.reply
    @State private var tryText = ""
    @State private var height: CGFloat = 40
    @State private var sent: String?

    private func hint(_ key: SendKey) -> String {
        switch key {
        case .smart: "Replies send on Return. A new task needs ⌘ Return, so a long brief can’t start halfway."
        case .enter: "Return sends everywhere. Shift-Return adds a line."
        case .modEnter: "Return always adds a line. ⌘ Return sends."
        }
    }

    private func name(_ key: SendKey) -> String {
        switch key {
        case .smart: "Smart"
        case .enter: "Return"
        case .modEnter: "⌘ Return"
        }
    }

    var body: some View {
        Form {
            Picker(selection: pref(stores.prefs, \.sendKey)) {
                ForEach(SendKey.allCases) { Text(name($0)).tag($0) }
            } label: {
                LabelledHint(title: "Send with", hint: hint(stores.prefs.sendKey))
            }
            Section("Try it") {
                Picker("As", selection: Binding(get: { tryAs == .reply }, set: { tryAs = $0 ? .reply : .task })) {
                    Text("Reply").tag(true)
                    Text("New task").tag(false)
                }
                .pickerStyle(.segmented)
                ZStack(alignment: .topLeading) {
                    if tryText.isEmpty { EditorPlaceholder(text: "Type, then press the key…", fontSize: 13, inset: CGSize(width: 8, height: 8)) }
                    MessageEditor(
                        text: $tryText, height: $height, fontSize: 13, minHeight: 40, maxHeight: 80, inset: NSSize(width: 8, height: 8),
                        enterSends: { stores.prefs.enterSends(tryAs) },
                        onSend: {
                            sent = tryText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Sent (it was empty)" : "Sent"
                            tryText = ""
                            Task {
                                try? await Task.sleep(for: .seconds(1.6))
                                sent = nil
                            }
                        },
                        onPasteImages: { _ in }, accessibility: "Try the send key"
                    )
                    .frame(height: height)
                }
                .card(cornerRadius: 8)
                Text(sent ?? " ").font(SF.font(12)).foregroundStyle(SF.okInk)
            }
        }
        .formStyle(.grouped)
        .frame(height: 320)
    }
}

private struct ReviewSettings: View {
    @Environment(Stores.self) private var stores

    var body: some View {
        Form {
            Picker("Layout", selection: pref(stores.prefs, \.diffView)) {
                Text("Unified").tag(DiffLayout.unified)
                Text("Split").tag(DiffLayout.split)
            }
            .pickerStyle(.segmented)
            Toggle(isOn: pref(stores.prefs, \.diffHideWhitespace)) {
                LabelledHint(title: "Hide whitespace changes", hint: "Lines that only changed their spacing read as unchanged.")
            }
            Toggle(isOn: pref(stores.prefs, \.diffCollapsed)) {
                LabelledHint(title: "Open with files collapsed", hint: "Each file starts folded when a change has several, except the one you opened.")
            }
        }
        .formStyle(.grouped)
        .frame(height: 220)
    }
}

private struct SnoozeSettings: View {
    @Environment(Stores.self) private var stores
    @State private var drafts: [SnoozePreset] = []

    var body: some View {
        Form {
            Section {
                ForEach(Array(drafts.enumerated()), id: \.offset) { i, preset in
                    HStack(spacing: 8) {
                        TextField("Label", text: Binding(get: { preset.label }, set: { drafts[i].label = $0 }))
                            .labelsHidden()
                        whenControl(i, preset)
                        Button {
                            drafts.remove(at: i)
                        } label: {
                            Image(systemName: "minus.circle").foregroundStyle(SF.muted)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Remove \(preset.label)")
                    }
                }
            } header: {
                Text("The choices in a task's Snooze menu")
            } footer: {
                HStack {
                    Button("Add") { drafts.append(SnoozePreset(label: "Later", when: .hours(2))) }
                        .disabled(drafts.count >= SnoozePreset.maxCount)
                    Button("Reset") { drafts = SnoozePreset.defaults }
                    Spacer()
                    Button("Save") { stores.prefs.set(\.snoozePresets, SnoozePreset.clean(.array(drafts.map(\.json)))) }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .formStyle(.grouped)
        .frame(height: 400)
        .onAppear { drafts = stores.prefs.snoozePresets }
    }

    @ViewBuilder private func whenControl(_ i: Int, _ preset: SnoozePreset) -> some View {
        switch preset.when {
        case .hours(let h):
            Stepper("\(h.formatted()) h", value: Binding(get: { h }, set: { drafts[i].when = .hours(max(0.5, $0)) }), in: 0.5...(24 * 30), step: 1)
                .fixedSize()
            Button("At a time") { drafts[i].when = .at(time: "09:00", day: "tomorrow") }.buttonStyle(.link)
        case .at(let time, let day):
            Picker("Day", selection: Binding(get: { day }, set: { drafts[i].when = .at(time: time, day: $0) })) {
                ForEach(SnoozePreset.days, id: \.id) { Text($0.name).tag($0.id) }
            }
            .labelsHidden()
            .fixedSize()
            TextField("HH:mm", text: Binding(get: { time }, set: { drafts[i].when = .at(time: $0, day: day) }))
                .labelsHidden()
                .frame(width: 54)
        }
    }
}

private struct GeneralSettings: View {
    @Environment(Stores.self) private var stores

    var body: some View {
        Form {
            Picker("Appearance", selection: pref(stores.prefs, \.appearance)) {
                Text("Match system").tag(Appearance.system)
                Text("Light").tag(Appearance.light)
                Text("Dark").tag(Appearance.dark)
            }
            .pickerStyle(.segmented)
            Toggle(isOn: pref(stores.prefs, \.confirmStop)) {
                LabelledHint(title: "Ask before stopping a task", hint: "So a stray click doesn't end it.")
            }
            Toggle(isOn: Binding(get: { stores.prefs.notifications }, set: { on in
                stores.prefs.set(\.notifications, on)
                if on { stores.notifier.requestPermission() }
            })) {
                LabelledHint(title: "Notify me", hint: "When a task starts waiting on you. They carry its title and the reason, never agent output.")
            }
            Toggle(isOn: pref(stores.prefs, \.keepRunningInMenuBar)) {
                LabelledHint(title: "Keep running in the menu bar", hint: "Closing the window leaves Sidefleet running. Notifications only arrive while it runs, so with this off they stop when you quit.")
            }
        }
        .formStyle(.grouped)
        .frame(height: 320)
    }
}

private struct AccountSettings: View {
    var session: AppSession
    @Environment(Stores.self) private var stores
    @State private var newOrg = ""

    private var fleet: FleetStore { stores.fleet }

    var body: some View {
        Form {
            if let url = stores.controlURL {
                LabeledContent("Control plane") { Text(url.absoluteString).textSelection(.enabled) }
            }
            if let build = fleet.setup?.build {
                let b = Format.parseBuild(build)
                if !b.sha.isEmpty {
                    LabeledContent("Build") { Text("\(b.version), \(b.sha) from \(b.date)") }
                }
            }
            if let user = fleet.me?.user {
                LabeledContent("Signed in as") { Text(user.login) }
                if let org = fleet.org { LabeledContent("Working in") { Text(org.name) } }
                HStack {
                    TextField("New org", text: $newOrg)
                    Button("Create") {
                        Task {
                            do {
                                let org = try await stores.api.createOrg(name: newOrg.trimmingCharacters(in: .whitespaces))
                                newOrg = ""
                                await fleet.refresh()
                                stores.switchOrg(org.id)
                            } catch {
                                stores.toasts.error("Could not create the org", error)
                            }
                        }
                    }
                    .disabled(newOrg.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } else if stores.isDemo {
                LabeledContent("Mode") { Text("Demo: made-up machines and tasks, nothing real") }
            } else {
                LabeledContent("Signed in with") { Text("Developer key") }
            }
            if !stores.isDemo {
                Button(fleet.me?.user != nil ? "Sign Out" : "Change Developer Key…") {
                    Task { await session.signOut() }
                }
            }
        }
        .formStyle(.grouped)
        .frame(height: 300)
    }
}
