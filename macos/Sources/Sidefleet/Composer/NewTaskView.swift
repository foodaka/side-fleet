import SidefleetKit
import SwiftUI

/// "What should run?": the new-task box, or how to get a machine connected when there's none.
struct NewTaskScreen: View {
    @Environment(Stores.self) private var stores

    var body: some View {
        Group {
            if !stores.fleet.loaded {
                VStack(alignment: .leading, spacing: 24) {
                    SkeletonBar(width: 220, height: 28)
                    SkeletonBar(height: 160)
                }
                .column()
                .padding(.horizontal, 24)
                .frame(maxHeight: .infinity, alignment: .center)
            } else if stores.fleet.myOnline.isEmpty {
                NoMachinesView()
            } else {
                NewTaskForm(model: stores.newTask)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SF.background)
    }
}

private struct NewTaskForm: View {
    @Bindable var model: NewTaskModel
    @Environment(Stores.self) private var stores
    @State private var height: CGFloat = 72
    @State private var focusTick = 1

    private let examples = [
        "Find why the login test is flaky and fix it",
        "Summarise what changed on main this week",
        "Add a dark mode toggle to the settings page",
    ]

    private func run() {
        Task {
            if let id = await model.run() {
                model.resetFromPrefs()
                stores.open(id)
            }
        }
    }

    var body: some View {
        GeometryReader { geo in
            ScrollView {
                VStack(spacing: 12) {
                    if stores.fleet.runs.isEmpty { steps }
                    Text("What should run?")
                        .font(SF.font(28, .medium))
                        .tracking(-0.4)
                        .padding(.bottom, 4)
                    box(maxHeight: geo.size.height * 0.55)
                    explanation
                    statChips
                    if stores.fleet.runs.isEmpty { exampleChips.padding(.top, 28) }
                }
                .column()
                .padding(.horizontal, 24)
                .padding(.top, max(40, geo.size.height * 0.14))
                .padding(.bottom, 40)
            }
        }
        .modifier(DropTarget { attach($0, to: &model.images, toasts: stores.toasts) })
        .onChange(of: stores.fleet.workers) { _, _ in model.syncSelection() }
        .onChange(of: stores.fleet.added) { _, _ in model.syncSelection() }
        .onAppear { model.syncSelection() }
        .onChange(of: stores.prefs.permission) { _, _ in model.resetFromPrefs() }
        .onChange(of: stores.prefs.workspace) { _, _ in model.resetFromPrefs() }
        .focusedSceneValue(\.focusComposer) { focusTick += 1 }
    }

    private func box(maxHeight: CGFloat) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack(alignment: .topLeading) {
                if model.task.isEmpty {
                    EditorPlaceholder(text: "Describe the task. It runs in a repo on one of your machines.", fontSize: 15, inset: CGSize(width: 16, height: 16))
                }
                MessageEditor(
                    text: $model.task, height: $height, fontSize: 15, minHeight: 72, maxHeight: maxHeight,
                    inset: NSSize(width: 16, height: 16),
                    enterSends: { stores.prefs.enterSends(.task) }, onSend: run,
                    onPasteImages: { attach($0, to: &model.images, toasts: stores.toasts) },
                    accessibility: "Task", focusTick: focusTick
                )
                .frame(height: height)
            }
            AttachmentStrip(images: $model.images).padding(.horizontal, 16).padding(.bottom, 8)
            HStack(alignment: .bottom, spacing: 8) {
                ControlRow {
                    AgentModelPicker(
                        agent: model.agent, model: model.model, effort: model.effort, models: model.models,
                        unavailable: { model.unavailable($0) },
                        onAgent: { model.pick(agent: $0) }, onModel: { model.pick(model: $0) }, onEffort: { model.pick(effort: $0) }
                    )
                    repoPicker
                    ComposerOptions(
                        agent: model.agent, perm: model.effectivePerm, onPerm: { model.perm = $0 },
                        defaultPerm: stores.prefs.permission(for: model.agent), defaultIso: stores.prefs.workspace,
                        machines: model.machines, machine: model.pinned?.id ?? "", onMachine: { model.pick(machine: $0) },
                        iso: model.iso, onIso: { model.iso = $0 }, failover: $model.failover
                    )
                    StartLaterPicker(value: $model.startAt)
                }
                HStack(spacing: 8) {
                    if model.task.count > 2000 {
                        Text("\(model.task.count.formatted()) characters").font(SF.font(12)).monospacedDigit().foregroundStyle(SF.faint)
                    }
                    Button {
                        attach(ImageImport.pickFiles(), to: &model.images, toasts: stores.toasts)
                    } label: {
                        Image(systemName: "paperclip").font(.system(size: 13)).frame(width: 32, height: 32)
                    }
                    .buttonStyle(QuietButtonStyle())
                    .help("Attach images (or drop or paste them)")
                    .accessibilityLabel("Attach images")
                    if !model.task.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Kbd(text: stores.prefs.sendKeys(.task)).help("Change which key sends in Settings").transition(.opacity)
                    }
                    Button(action: run) {
                        Image(systemName: "arrow.up")
                    }
                    .buttonStyle(RoundIconButtonStyle(size: 36))
                    .disabled(!model.canRun)
                    .accessibilityLabel("Run task")
                }
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 10)
            .animation(.easeOut(duration: 0.15), value: model.task.isEmpty)
        }
        .background(SF.card, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(SF.input))
        .shadow(color: .black.opacity(0.04), radius: 3, y: 1)
    }

    private var repoPicker: some View {
        let online = model.onlineRepos
        var options = model.repos.map { r in
            PickerOption(id: r, title: Format.repoLabel(r), disabledReason: online.contains(r) ? nil : "offline", mono: true)
        }
        options.append(PickerOption(id: "__pick__", title: model.repos.isEmpty ? "Choose from \(online.count) repos…" : "Add or remove repos…"))
        return PickerPill(
            options: options, selected: model.repo, width: 300, accessibility: "Repo: \(Format.repoLabel(model.repo))",
            onPick: { id in
                if id == "__pick__" {
                    stores.repoPickerOpen = true
                } else {
                    model.pick(repo: id)
                }
            }
        ) {
            Text(model.repo.isEmpty ? "Choose a repo" : Format.repoLabel(model.repo)).font(SF.mono(12.5)).lineLimit(1).frame(maxWidth: 200)
        }
    }

    @ViewBuilder private var explanation: some View {
        VStack(spacing: 2) {
            if model.repos.isEmpty {
                HStack(spacing: 4) {
                    Text("Your machines have \(model.onlineRepos.count) git checkouts.")
                    Button("Choose the ones you work on") { stores.repoPickerOpen = true }
                        .buttonStyle(.plain)
                        .foregroundStyle(SF.ink)
                        .underline()
                    Text("to fill the repo menu.")
                }
            } else {
                LiveText(every: 30) { now in
                    VStack(spacing: 2) {
                        ForEach(Array(model.explanation(now: now).enumerated()), id: \.offset) { _, line in
                            line.reduce(Text("")) { text, part in
                                text + Text(part.text).foregroundColor(part.tone == .strong ? SF.ink : part.tone == .warning ? SF.errInk : SF.muted)
                            }
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
        .font(SF.font(12))
        .foregroundStyle(SF.muted)
        .frame(minHeight: 20)
    }

    @ViewBuilder private var statChips: some View {
        if let s = model.stats {
            HStack(spacing: 6) {
                StatChip(symbol: "cpu", text: "\(Int(s.cpuPercent.rounded()))% of \(s.cpuCores) cores", help: "CPU busy, \(s.cpuCores) cores")
                StatChip(symbol: "memorychip", text: String(format: "%.1f / %.0f GB", s.memUsedMB / 1024, s.memTotalMB / 1024), help: "Memory in use")
                StatChip(symbol: "internaldrive", text: "\(Int(s.diskFreeGB.rounded()).formatted()) GB free", help: "Free disk where the repos live")
            }
        }
    }

    private var steps: some View {
        HStack(spacing: 8) {
            Step(done: true, text: "Signed in")
            Rectangle().fill(SF.ok.opacity(0.5)).frame(width: 20, height: 1)
            Step(done: true, text: "\(stores.fleet.myOnline.first?.displayName ?? "Machine") connected")
            Rectangle().fill(SF.border).frame(width: 20, height: 1)
            HStack(spacing: 6) {
                Text(stores.fleet.setup?.workerToken.isEmpty == false ? "3" : "4")
                    .font(.system(size: 10))
                    .monospacedDigit()
                    .foregroundStyle(SF.background)
                    .frame(width: 16, height: 16)
                    .background(SF.ink, in: Circle())
                Text("Run your first task").font(SF.font(13, .medium))
            }
        }
        .font(SF.font(13))
        .foregroundStyle(SF.muted)
        .padding(.bottom, 16)
    }

    private var exampleChips: some View {
        VStack(spacing: 8) {
            ForEach(examples, id: \.self) { ex in
                Button {
                    model.task = ex
                    focusTick += 1
                } label: {
                    Text(ex).font(SF.font(13)).foregroundStyle(SF.muted).padding(.horizontal, 12).padding(.vertical, 6)
                        .overlay(Capsule().strokeBorder(SF.border))
                        .hoverBackground(cornerRadius: 16)
                }
                .buttonStyle(.plain)
            }
        }
    }
}

/// Lays controls out in a row and lets the row shrink before anything truncates badly.
private struct ControlRow<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 2) { content() }.frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) { content() }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private struct Step: View {
    var done: Bool
    var text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "checkmark")
                .font(.system(size: 8, weight: .heavy))
                .foregroundStyle(.white)
                .frame(width: 16, height: 16)
                .background(SF.ok, in: Circle())
            Text(text)
        }
    }
}

struct StatChip: View {
    var symbol: String
    var text: String
    var help: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: symbol).font(.system(size: 11))
            Text(text).monospacedDigit()
        }
        .font(SF.font(12))
        .foregroundStyle(SF.muted)
        .padding(.horizontal, 10)
        .frame(height: 24)
        .background(SF.card, in: Capsule())
        .overlay(Capsule().strokeBorder(SF.border))
        .help(help)
    }
}

private struct NoMachinesView: View {
    @Environment(Stores.self) private var stores

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                if stores.fleet.mine.isEmpty {
                    VStack(alignment: .leading, spacing: 12) {
                        Text("Put your first machine to work").font(SF.font(28, .medium)).tracking(-0.4)
                        Text("Sidefleet runs tasks with the coding agents already set up on your own machines and streams what they do back here. It takes a couple of minutes.")
                            .font(SF.font(15)).lineSpacing(4).foregroundStyle(SF.muted)
                        if stores.fleet.multi {
                            Text("Your teammates' machines only run their own tasks, so connect one of yours to take on work.")
                                .font(SF.font(13)).lineSpacing(3).foregroundStyle(SF.muted)
                        }
                    }
                } else {
                    VStack(alignment: .leading, spacing: 12) {
                        Text("Your machines are offline").font(SF.font(28, .medium)).tracking(-0.4)
                        Text("\(stores.fleet.mine.map(\.displayName).joined(separator: ", ")) \(stores.fleet.mine.count == 1 ? "has" : "have") stopped checking in. Run `sidefleet status` there, or connect another machine below.")
                            .font(SF.font(15)).lineSpacing(4).foregroundStyle(SF.muted)
                    }
                }
                AddMachineSteps().padding(20).card(cornerRadius: 18)
                HStack(spacing: 8) {
                    StatusDot(status: .waiting)
                    Text("Waiting for a machine to check in. This page updates by itself.").font(SF.font(13)).foregroundStyle(SF.muted)
                }
            }
            .frame(maxWidth: 588, alignment: .leading)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 24)
            .padding(.vertical, 72)
        }
    }
}
