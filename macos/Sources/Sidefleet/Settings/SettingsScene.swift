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

    /// A menu can't carry each choice's second line, so the one chosen is spelled out under the row's own hint.
    private func chosen(_ hint: String?) -> String { hint.map { "\n\($0)." } ?? "" }

    /// Where these are kept: with your account when you signed in with GitHub, else on this Mac.
    private var kept: String {
        guard stores.prefs.account else { return "How Sidefleet behaves for you. Changes apply right away. They’re kept on this Mac." }
        let login = stores.fleet.me?.user.map { " (\($0.login))" } ?? ""
        return "How Sidefleet behaves for you. Changes apply right away. New task defaults, the send key and confirmations are kept with your account\(login), so the page and every Mac you sign in on use them. How changes look and the theme stay with this device."
    }

    var body: some View {
        Form {
            Section {
                Picker(selection: pref(stores.prefs, \.permission)) {
                    ForEach(Format.permissions) { Text($0.name).tag($0.id) }
                } label: {
                    LabelledHint(title: "Permissions", hint: "For tasks you start from New task, and forks of a message that has no mode of its own." + chosen(Format.permissions.first { $0.id == stores.prefs.permission }?.hint))
                }
                Picker(selection: pref(stores.prefs, \.roomPermission)) {
                    ForEach(Format.permissions) { Text($0.name).tag($0.id) }
                } label: {
                    LabelledHint(title: "Permissions in rooms", hint: "Room tasks read your teammates’ notes, so a note could steer what they do. Full access lets that reach commands on your machine." + chosen(Format.permissions.first { $0.id == stores.prefs.roomPermission }?.hint))
                }
                Picker(selection: pref(stores.prefs, \.workspace)) {
                    ForEach(Format.workspaces) { Text($0.name).tag($0.id) }
                } label: {
                    LabelledHint(title: "Workspace", hint: "Own branch lets tasks run side by side. In place works in the checkout, one task per repo at a time.")
                }
            } header: {
                Text("What every new task starts with, wherever you start it. The composer’s options menu can change them for one task without changing these.")
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Ask me works with Claude Code only; other agents start with Read & edit files instead. A reply keeps the mode its task was using, and a fork keeps its message’s.")
                    Text(kept)
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .formStyle(.grouped)
        .frame(height: 440)
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

    /// The keys that send each kind of message with this choice, as the cards show them.
    private func keys(_ key: SendKey, _ box: MessageBox) -> String {
        switch key {
        case .enter: "↵"
        case .modEnter: "⌘↵"
        case .smart: box == .reply ? "↵" : "⌘↵"
        }
    }

    private func card(_ key: SendKey) -> some View {
        let on = stores.prefs.sendKey == key
        return Button {
            stores.prefs.set(\.sendKey, key)
        } label: {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 6) {
                    Image(systemName: on ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 12))
                        .foregroundStyle(on ? SF.ink : SF.muted)
                    Text(name(key)).font(SF.font(13, .medium))
                    Spacer(minLength: 0)
                    if key == .smart {
                        Text("Default").font(SF.font(10.5)).foregroundStyle(SF.muted).padding(.horizontal, 6).padding(.vertical, 1).background(SF.fill, in: Capsule())
                    }
                }
                VStack(spacing: 4) {
                    HStack { Text("New task"); Spacer(); Kbd(text: keys(key, .task)) }
                    HStack { Text("Reply"); Spacer(); Kbd(text: keys(key, .reply)) }
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                Text(hint(key)).font(SF.font(12)).lineSpacing(2).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
            }
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(SF.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(on ? SF.ink.opacity(0.25) : SF.border))
            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    var body: some View {
        Form {
            Section {
                HStack(alignment: .top, spacing: 8) {
                    ForEach(SendKey.allCases) { card($0) }
                }
            } header: {
                Text("Which key sends a new task or a reply. ⌘ Return always sends.")
            } footer: {
                EmptyView()
            }
            Section("Try it") {
                Picker("As", selection: Binding(get: { tryAs == .reply }, set: { tryAs = $0 ? .reply : .task })) {
                    Text("Reply").tag(true)
                    Text("New task").tag(false)
                }
                .pickerStyle(.segmented)
                HStack(spacing: 4) {
                    Text("sends on")
                    Kbd(text: stores.prefs.sendKeys(tryAs))
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
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
        .frame(height: 440)
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

/// The choices in a task's Snooze menu. Each is some hours from now, or a time of day on a day;
/// every change is saved at once, as on the page.
private struct SnoozeSettings: View {
    @Environment(Stores.self) private var stores

    private var presets: [SnoozePreset] { stores.prefs.snoozePresets }

    private func save(_ list: [SnoozePreset]) {
        stores.prefs.set(\.snoozePresets, SnoozePreset.clean(.array(list.map(\.json))))
    }

    var body: some View {
        Form {
            Section {
                ForEach(Array(presets.enumerated()), id: \.offset) { i, preset in
                    SnoozePresetRow(
                        preset: preset,
                        canRemove: presets.count > 1,
                        onChange: { next in save(presets.enumerated().map { $0.offset == i ? next : $0.element }) },
                        onRemove: { save(presets.enumerated().filter { $0.offset != i }.map(\.element)) }
                    )
                    // A row starts afresh when its preset changes, so its fields show what was saved.
                    .id("\(i) \(preset.id)")
                }
            } header: {
                Text("The choices in a task’s Snooze menu. A snoozed task leaves the list and the bell until its time, or until it finishes, fails or asks you something.")
            } footer: {
                HStack {
                    Button {
                        save(presets + [SnoozePreset(label: "Later today", when: .hours(4))])
                    } label: {
                        Label("Add a choice", systemImage: "plus")
                    }
                    .disabled(presets.count >= SnoozePreset.maxCount)
                    Spacer()
                    if presets != SnoozePreset.defaults {
                        Button("Reset to the defaults") { save(SnoozePreset.defaults) }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .frame(height: 440)
    }
}

private struct SnoozePresetRow: View {
    var preset: SnoozePreset
    var canRemove: Bool
    var onChange: (SnoozePreset) -> Void
    var onRemove: () -> Void
    @State private var label = ""
    @State private var hours = ""
    @State private var time = ""
    @FocusState private var labelFocused: Bool
    @FocusState private var hoursFocused: Bool
    @FocusState private var timeFocused: Bool

    private var isHours: Bool {
        if case .hours = preset.when { return true }
        return false
    }

    var body: some View {
        HStack(spacing: 8) {
            TextField("Name", text: $label)
                .labelsHidden()
                .focused($labelFocused)
                .onSubmit(commitLabel)
                .onChange(of: labelFocused) { _, on in if !on { commitLabel() } }
                .accessibilityLabel("Name")
            Picker("Kind", selection: Binding(get: { isHours }, set: { setKind(hours: $0) })) {
                Text("In hours").tag(true)
                Text("At a time").tag(false)
            }
            .labelsHidden()
            .fixedSize()
            switch preset.when {
            case .hours(let h):
                TextField("Hours", text: $hours)
                    .labelsHidden()
                    .frame(width: 56)
                    .focused($hoursFocused)
                    .onSubmit(commitHours)
                    .onChange(of: hoursFocused) { _, on in if !on { commitHours() } }
                    .accessibilityLabel("Hours from now")
                Stepper("", value: Binding(get: { h }, set: { onChange(SnoozePreset(label: preset.label, when: .hours(min(8760, max(0.25, $0))))) }), in: 0.25...8760, step: 0.25)
                    .labelsHidden()
            case .at(_, let day):
                TextField("HH:mm", text: $time)
                    .labelsHidden()
                    .frame(width: 56)
                    .focused($timeFocused)
                    .onSubmit(commitTime)
                    .onChange(of: timeFocused) { _, on in if !on { commitTime() } }
                    .accessibilityLabel("Time")
                Picker("Day", selection: Binding(get: { day }, set: { d in
                    if case .at(let t, _) = preset.when { onChange(SnoozePreset(label: preset.label, when: .at(time: t, day: d))) }
                })) {
                    ForEach(SnoozePreset.days, id: \.id) { Text($0.name).tag($0.id) }
                }
                .labelsHidden()
                .fixedSize()
            }
            Spacer(minLength: 4)
            // Picked now, it would come back then.
            LiveText(every: 30) { now in
                Text(Format.wakeLabel(preset.time(now: now), now: now)).font(SF.font(12)).monospacedDigit().foregroundStyle(SF.muted)
            }
            .help("Picked now, it would come back then")
            Button(action: onRemove) {
                Image(systemName: "xmark").font(.system(size: 11)).foregroundStyle(SF.muted)
            }
            .buttonStyle(.plain)
            .disabled(!canRemove)
            .accessibilityLabel("Remove \(preset.label)")
        }
        .onAppear {
            label = preset.label
            if case .hours(let h) = preset.when { hours = h.formatted() }
            if case .at(let t, _) = preset.when { time = t }
        }
    }

    private func commitLabel() {
        let l = String(label.trimmingCharacters(in: .whitespacesAndNewlines).prefix(40))
        if l.isEmpty || l == preset.label {
            label = preset.label
        } else {
            onChange(SnoozePreset(label: l, when: preset.when))
        }
    }

    private func commitHours() {
        guard case .hours(let h) = preset.when else { return }
        let parsed = Double(hours.replacingOccurrences(of: ",", with: ".").trimmingCharacters(in: .whitespaces))
        if let v = parsed, v > 0, v <= 8760, v != h {
            onChange(SnoozePreset(label: preset.label, when: .hours(v)))
        } else {
            hours = h.formatted()
        }
    }

    private func commitTime() {
        guard case .at(let t, let day) = preset.when else { return }
        let parts = time.split(separator: ":")
        let valid = parts.count == 2 && parts[0].count == 2 && parts[1].count == 2 && (Int(parts[0]).map { (0..<24).contains($0) } ?? false) && (Int(parts[1]).map { (0..<60).contains($0) } ?? false)
        if valid, time != t {
            onChange(SnoozePreset(label: preset.label, when: .at(time: time, day: day)))
        } else {
            time = t
        }
    }

    private func setKind(hours: Bool) {
        guard hours != isHours else { return }
        onChange(SnoozePreset(label: preset.label, when: hours ? .hours(1) : .at(time: "09:00", day: "tomorrow")))
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
