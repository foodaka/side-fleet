import SidefleetKit
import SwiftUI

/// The agent menu and, next to it, the model control that suits that agent: Claude's aliases, the
/// machine's Codex models, or a searchable field for opencode; then the reasoning effort.
struct AgentModelPicker: View {
    var agent: AgentID
    var model: String
    var effort: String
    /// Models the target machines offer for the agent; for Claude, what its aliases run.
    var models: [String]
    var small = false
    var unavailable: (AgentID) -> String? = { _ in nil }
    var onAgent: (AgentID) -> Void
    var onModel: (String) -> Void
    var onEffort: (String) -> Void

    private var height: CGFloat { small ? 28 : 32 }
    private var text: Font { SF.font(small ? 12 : 13) }
    private var mono: Font { SF.mono(small ? 11.5 : 12.5) }

    var body: some View {
        PickerPill(
            options: Format.agents.map { a in
                PickerOption(id: a.id.rawValue, title: a.name, hint: a.hint, disabledReason: unavailable(a.id), icon: AnyView(AgentIcon(agent: a.id)))
            },
            selected: agent.rawValue, height: height, width: 300, accessibility: "Agent: \(Format.agentName(agent))",
            onPick: { if let a = AgentID(rawValue: $0) { onAgent(a) } }
        ) {
            AgentIcon(agent: agent)
            Text(Format.agentName(agent)).font(text)
        }

        modelControl

        let efforts = Format.efforts(agent)
        if !efforts.isEmpty {
            PickerPill(
                options: [PickerOption(id: "", title: "Default", hint: "Whatever \(Format.agentName(agent)) uses on that machine")]
                    + efforts.map { PickerOption(id: $0.id, title: $0.name, hint: $0.hint) },
                selected: effort, heading: "Reasoning", height: height, width: 280, accessibility: "Reasoning effort: \(effort.isEmpty ? "default" : effort)",
                onPick: onEffort
            ) {
                Image(systemName: "gauge.with.dots.needle.33percent").font(.system(size: 11)).foregroundStyle(SF.muted)
                if effort.isEmpty {
                    Text("Effort").font(text).foregroundStyle(SF.muted)
                } else {
                    Text(Format.effortName(agent: agent, effort: effort)).font(text)
                }
            }
        }
    }

    @ViewBuilder private var modelControl: some View {
        if agent == .claude {
            PickerPill(
                options: Format.models.map { PickerOption(id: $0.id, title: Format.pickerModelName($0.id, models: models), hint: $0.hint) },
                selected: model, height: height, width: 220, accessibility: "Model: \(Format.pickerModelName(model, models: models))", onPick: pick
            ) {
                Text(Format.pickerModelName(model, models: models)).font(text)
            }
        } else if agent == .codex, !models.isEmpty || model.isEmpty {
            PickerPill(
                options: [PickerOption(id: "", title: "Default model", hint: "Whatever Codex uses on that machine")]
                    + (models + (model.isEmpty || models.contains(model) ? [] : [model])).map { PickerOption(id: $0, title: $0, mono: true) },
                selected: model, height: height, width: 280, accessibility: "Model: \(model.isEmpty ? "default" : model)", onPick: pick
            ) {
                Text(model.isEmpty ? "Default model" : model).font(model.isEmpty ? text : mono)
            }
        } else {
            ModelSearchField(model: model, models: models, height: height, font: mono, agent: agent, onModel: pick)
        }
    }

    /// A model id never has space around it, however it was typed or pasted.
    private func pick(_ m: String) { onModel(m.trimmingCharacters(in: .whitespacesAndNewlines)) }
}

/// opencode can offer hundreds of models (OpenRouter alone): type to search them.
private struct ModelSearchField: View {
    var model: String
    var models: [String]
    var height: CGFloat
    var font: Font
    var agent: AgentID
    var onModel: (String) -> Void
    @State private var draft = ""
    @State private var open = false
    @FocusState private var focused: Bool

    private var matches: [String] {
        let q = draft.lowercased()
        return Array((q.isEmpty || q == model.lowercased() ? models : models.filter { $0.lowercased().contains(q) }).prefix(60))
    }

    var body: some View {
        TextField("Default model", text: $draft)
            .textFieldStyle(.plain)
            .font(font)
            .foregroundStyle(Format.modelValid(agent: agent, model: draft) ? SF.ink : SF.errInk)
            .padding(.horizontal, 12)
            .frame(width: 210, height: height)
            .background(focused ? SF.hover : .clear, in: Capsule())
            .focused($focused)
            .accessibilityLabel("Model")
            .onAppear { draft = model }
            // The parent trims what it's given: don't take the space you just typed back out of the box.
            .onChange(of: model) { _, m in if m != draft.trimmingCharacters(in: .whitespacesAndNewlines) { draft = m } }
            // As you type, not only on leaving the box: Send doesn't take the focus away, and must
            // send the model that's on screen.
            .onChange(of: draft) { _, d in
                if d.trimmingCharacters(in: .whitespacesAndNewlines) != model { onModel(d) }
            }
            .onChange(of: focused) { _, on in
                if on {
                    open = !models.isEmpty
                } else {
                    onModel(draft)
                }
            }
            .onSubmit {
                onModel(draft)
                open = false
            }
            .popover(isPresented: $open, arrowEdge: .bottom) {
                OptionList(options: matches.map { PickerOption(id: $0, title: $0, mono: true) }, selected: model, width: 300) { id in
                    draft = id
                    onModel(id)
                    open = false
                }
            }
    }
}

/// The composer's less-used settings behind one control: machine, permissions, workspace and failover.
/// At their defaults it is just an icon; anything changed is named on the control.
struct ComposerOptions: View {
    var agent: AgentID
    /// The permission mode; "" means "same as before" when `sameAs` is given.
    var perm: String
    var onPerm: (String) -> Void
    var defaultPerm = "acceptEdits"
    var defaultIso = "worktree"
    /// The reply box: the previous message's mode, offered as "Same permissions".
    var sameAs: String??
    var machines: [Worker] = []
    var machine = ""
    var onMachine: (String) -> Void = { _ in }
    var iso: String?
    var onIso: (String) -> Void = { _ in }
    @Binding var failover: Bool
    var small = false
    @State private var open = false

    private var pinned: Worker? { machines.first { $0.id == machine } }
    private var showMachines: Bool { machines.count > 1 }
    private var showFailover: Bool { iso == "worktree" }

    /// What differs from the defaults, in the order the panel lists it.
    private var summary: String {
        [
            showMachines ? pinned?.displayName : nil,
            !perm.isEmpty && perm != defaultPerm ? Format.permName(perm) : nil,
            iso != nil && iso != defaultIso ? Format.workspaces.first { $0.id == iso }?.name : nil,
            showFailover && failover ? "Moves if it drops" : nil,
        ].compactMap { $0 }.joined(separator: " · ")
    }

    var body: some View {
        Button {
            open.toggle()
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "slider.horizontal.3").font(.system(size: 11))
                if !summary.isEmpty {
                    Text(summary).font(SF.font(small ? 12 : 13)).foregroundStyle(SF.ink.opacity(0.8)).lineLimit(1)
                }
            }
            .foregroundStyle(open ? SF.ink : SF.muted)
            .padding(.horizontal, summary.isEmpty ? 0 : 10)
            .frame(minWidth: small ? 28 : 32, minHeight: small ? 28 : 32)
            .hoverBackground(active: open, cornerRadius: small ? 14 : 16)
        }
        .buttonStyle(.plain)
        .help("Machine, permissions and workspace")
        .accessibilityLabel("Options" + (summary.isEmpty ? "" : ": \(summary)"))
        .popover(isPresented: $open, arrowEdge: .bottom) { panel }
    }

    private var panel: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                if showMachines {
                    OptionSection(title: "Machine", options: [PickerOption(id: "", title: "Any machine", hint: "The first one with a free slot")] + machines.map { w in
                        PickerOption(id: w.id, title: w.displayName, hint: w.capabilities.slots.map { "\($0.active) of \($0.parallel) slots busy" })
                    }, selected: pinned?.id ?? "", onPick: onMachine)
                }
                OptionSection(title: "Permissions", options: permOptions, selected: perm, onPick: onPerm)
                if let iso {
                    OptionSection(title: "Workspace", options: Format.workspaces.map { PickerOption(id: $0.id, title: $0.name, hint: $0.hint) }, selected: iso, onPick: onIso)
                }
                if showFailover {
                    Toggle(isOn: $failover) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Move if it drops").font(SF.font(13))
                            Text("Pushes checkpoints of the task's branch to origin, so another machine can take over")
                                .font(SF.font(12)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .padding(.horizontal, 8)
                }
                if sameAs == nil {
                    SettingsLink {
                        Text("Changes here are for this task only. Set your defaults in Settings…")
                            .font(SF.font(11.5)).foregroundStyle(SF.faint).multilineTextAlignment(.leading)
                            .padding(.horizontal, 8).padding(.vertical, 3)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .hoverBackground()
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(10)
        }
        .frame(width: 320)
        .frame(maxHeight: 460)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var permOptions: [PickerOption] {
        var out: [PickerOption] = []
        if let sameAs {
            out.append(PickerOption(id: "", title: "Same permissions", hint: sameAs.map { "\(Format.permName($0)), as the last message" }))
        }
        out += Format.perms(for: agent).map { PickerOption(id: $0.id, title: $0.name, hint: Format.permHint($0.id, agent: agent)) }
        return out
    }
}

private struct OptionSection: View {
    var title: String
    var options: [PickerOption]
    var selected: String
    var onPick: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(title).font(SF.font(11, .medium)).foregroundStyle(SF.faint).padding(.horizontal, 8).padding(.bottom, 3)
            ForEach(options) { o in
                Button {
                    onPick(o.id)
                } label: {
                    HStack(alignment: .top) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(o.title).font(SF.font(13))
                            if let hint = o.hint {
                                Text(hint).font(SF.font(12)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        Spacer(minLength: 6)
                        if o.id == selected {
                            Image(systemName: "checkmark").font(.system(size: 11, weight: .semibold)).padding(.top, 2)
                        }
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .hoverBackground()
                }
                .buttonStyle(.plain)
            }
        }
    }
}

/// Start a task later: a few presets and a date-and-time picker.
struct StartLaterPicker: View {
    @Binding var value: Date?
    var small = false
    @State private var open = false
    @State private var picked = Date()

    var body: some View {
        Button {
            picked = value ?? Date().addingTimeInterval(3600)
            open.toggle()
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "alarm").font(.system(size: 11))
                if let value {
                    LiveText(every: 30) { now in
                        Text(Format.startTime(value, now: now)).font(SF.font(small ? 12 : 13)).monospacedDigit()
                    }
                }
            }
            .foregroundStyle(value != nil ? SF.ink : open ? SF.ink : SF.muted)
            .padding(.horizontal, value == nil ? 0 : 10)
            .frame(minWidth: small ? 28 : 32, minHeight: small ? 28 : 32)
            .hoverBackground(active: open, cornerRadius: small ? 14 : 16)
        }
        .buttonStyle(.plain)
        .help("Start later")
        .accessibilityLabel(value == nil ? "Start later" : "Starts \(Format.startTime(value!, now: Date()))")
        .popover(isPresented: $open, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Start later").font(SF.font(11, .medium)).foregroundStyle(SF.faint).padding(.horizontal, 8)
                VStack(spacing: 1) {
                    ForEach(NewTaskModel.startPresets(now: Date()), id: \.label) { preset in
                        Button {
                            value = preset.at
                            open = false
                        } label: {
                            HStack {
                                Text(preset.label).font(SF.font(13))
                                Spacer()
                                Text(Format.startTime(preset.at, now: Date())).font(SF.font(12)).monospacedDigit().foregroundStyle(SF.muted)
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .hoverBackground()
                        }
                        .buttonStyle(.plain)
                    }
                }
                Divider()
                HStack(spacing: 8) {
                    DatePicker("At", selection: $picked, in: Date()...Date().addingTimeInterval(Double(NewTaskModel.maxStartDays) * 86_400), displayedComponents: [.date, .hourAndMinute])
                        .datePickerStyle(.stepperField)
                        .labelsHidden()
                    Button("Set") {
                        value = picked
                        open = false
                    }
                    .buttonStyle(SecondaryButtonStyle(height: 24))
                }
                .padding(.horizontal, 8)
                if value != nil {
                    Button {
                        value = nil
                        open = false
                    } label: {
                        Label("Start now instead", systemImage: "xmark").font(SF.font(13)).padding(.horizontal, 8).padding(.vertical, 5)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(QuietButtonStyle(cornerRadius: 6))
                }
            }
            .padding(8)
            .frame(width: 290)
        }
    }
}
