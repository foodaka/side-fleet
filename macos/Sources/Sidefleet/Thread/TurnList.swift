import SidefleetKit
import SwiftUI

struct ReviewRequest: Equatable {
    var scope: String
    var focus: String?
}

struct ForkRequest: Identifiable {
    var run: Run
    var hasFiles: Bool
    var id: String { run.id }
}

/// The conversation. Stays pinned to the newest line unless you've scrolled up to read.
struct TurnList: View {
    var thread: ThreadStore
    var theirs: Bool
    var pr: PullRequest?
    var machineOnline: Bool
    var opening: Bool
    var onOpenDiff: (String?, String?) -> Void
    var onFork: (Turn) -> Void
    var onOpenPR: () -> Void
    var onSendNow: () -> Void
    @Environment(Stores.self) private var stores
    @State private var pinned = true
    @State private var viewport: CGFloat = 0

    private static let bottomID = "thread-bottom"

    /// Changes whenever new content lands.
    private var contentMark: Int {
        var h = Hasher()
        for t in thread.turns {
            h.combine(t.items.count)
            h.combine(t.liveText.count)
            h.combine(t.activity)
            h.combine(t.result != nil)
        }
        return h.finalize()
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(thread.turns.enumerated()), id: \.element.id) { i, turn in
                        TurnRows(
                            turn: turn, first: i == 0, prev: thread.previousAgentTurn(before: i),
                            live: turn.id == thread.current?.id && thread.live, waiting: thread.isWaiting(turn.id), theirs: theirs,
                            thread: thread, onOpenDiff: onOpenDiff, onFork: onFork, onSendNow: onSendNow
                        )
                    }
                    closingNote
                    bottomMarker.id(Self.bottomID)
                }
                .column()
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 12)
            }
            .coordinateSpace(name: "thread")
            .background {
                GeometryReader { geo in
                    Color.clear
                        .onAppear { viewport = geo.size.height }
                        .onChange(of: geo.size.height) { _, h in viewport = h }
                }
            }
            .overlay(alignment: .bottom) {
                if !pinned {
                    Button {
                        pinned = true
                        withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(Self.bottomID, anchor: .bottom) }
                    } label: {
                        Image(systemName: "arrow.down")
                            .font(.system(size: 13, weight: .medium))
                            .frame(width: 36, height: 36)
                            .background(.regularMaterial, in: Circle())
                            .overlay(Circle().strokeBorder(SF.border))
                            .shadow(color: .black.opacity(0.1), radius: 6, y: 2)
                    }
                    .buttonStyle(.plain)
                    .padding(.bottom, 12)
                    .transition(.opacity.combined(with: .scale(scale: 0.9)))
                    .accessibilityLabel("Scroll to the latest message")
                }
            }
            .animation(.easeOut(duration: 0.15), value: pinned)
            .onChange(of: contentMark) { _, _ in
                if pinned { proxy.scrollTo(Self.bottomID, anchor: .bottom) }
            }
            .onChange(of: thread.loaded) { _, _ in
                proxy.scrollTo(Self.bottomID, anchor: .bottom)
                pinned = true
            }
        }
    }

    /// Reports whether the end of the thread is in view. Lazy stacks drop it when it's far away, so
    /// disappearing counts as scrolled up too.
    private var bottomMarker: some View {
        GeometryReader { geo in
            let y = geo.frame(in: .named("thread")).minY
            Color.clear
                .onChange(of: y) { _, y in
                    let atBottom = y <= viewport + 48
                    if atBottom != pinned { pinned = atBottom }
                }
        }
        .frame(height: 1)
        .onDisappear { pinned = false }
    }

    /// The dashed note at the end, saying where the work is and what to do with it.
    @ViewBuilder private var closingNote: some View {
        if !thread.live, !thread.turns.isEmpty, let branch = thread.branch, (thread.changes?.fileCount ?? 0) > 0 || pr != nil {
            HStack(spacing: 12) {
                closingText(branch: branch)
                    .font(SF.font(12))
                    .lineSpacing(3)
                    .foregroundStyle(SF.muted)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                if pr?.state != "merged", !theirs {
                    let pushing = pr?.state == "open"
                    Group {
                        if pushing {
                            prButton(label: opening ? "Starting…" : "Push changes", branch: branch).buttonStyle(SecondaryButtonStyle())
                        } else {
                            prButton(label: opening ? "Starting…" : "Open pull request", branch: branch).buttonStyle(PrimaryButtonStyle())
                        }
                    }
                    .disabled(opening || !machineOnline)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            .padding(.top, 28)
        }
    }

    private func prButton(label: String, branch: String) -> some View {
        Button(action: onOpenPR) {
            Label(label, systemImage: "arrow.triangle.pull")
        }
        .help(machineOnline ? "Commits what's left, pushes \(branch) to origin and opens a pull request with the GitHub CLI on \(thread.machine ?? "that machine")" : "\(thread.machine ?? "That machine") is offline")
    }

    private func closingText(branch: String) -> Text {
        let mono = Text(branch).font(SF.mono(12)).foregroundColor(SF.ink.opacity(0.8))
        let machine = thread.machine ?? "that machine"
        switch pr?.state {
        case "merged":
            return Text("Merged in pull request #\(pr?.number ?? 0). The branch ") + mono + Text(" is still on \(machine).")
        case "open":
            return Text("Pull request #\(pr?.number ?? 0) is open for ") + mono + Text(". Keep working here, then push the new changes to it.")
        default:
            let closed = pr?.state == "closed" ? " Pull request #\(pr?.number ?? 0) was closed." : ""
            return Text("The changes are on branch ") + mono + Text(" in \(Format.repoLabel(thread.root?.repo)) on \(machine).\(closed) Open a pull request, or review, merge or push it from there.")
        }
    }
}

/// One turn, as rows of the lazy stack rather than one big cell, so a long turn stays light.
private struct TurnRows: View {
    var turn: Turn
    var first: Bool
    var prev: Turn?
    var live: Bool
    var waiting: Bool
    var theirs: Bool
    var thread: ThreadStore
    var onOpenDiff: (String?, String?) -> Void
    var onFork: (Turn) -> Void
    var onSendNow: () -> Void
    @Environment(Stores.self) private var stores
    @State private var confirmingRestore = false
    @State private var restoring = false

    private var run: Run { turn.run }
    private var finished: Bool { [.succeeded, .failed, .stopped].contains(run.status) }
    private var switched: Bool { prev.map { run.agent != $0.run.agent || run.model != $0.run.model } ?? false }
    private var relocated: Bool {
        guard let prev, run.parentID != nil else { return true }
        return turn.machine != prev.machine || (turn.branch != nil && prev.branch != nil && turn.branch != prev.branch)
    }

    var body: some View {
        Group {
            if !first { Color.clear.frame(height: 28) }
            if first, !run.images.isEmpty {
                ImageStrip(images: run.images).padding(.bottom, 12)
            }
            opening
            setupLine
            ForEach(turn.groups) { group in
                row(group)
            }
            if live, !turn.liveText.isEmpty {
                // Takes the place of the final message, which lands here when it is complete.
                MarkdownView(text: turn.liveText).padding(.vertical, 6)
            }
            if live, let activity = turn.activity, activity != "queued", turn.liveText.isEmpty {
                HStack(spacing: 8) {
                    StatusDot(status: .run(.running))
                    Shimmer(text: activity)
                    if turn.reconnecting { Text("reconnecting…").font(SF.font(12)).foregroundStyle(SF.queueInk) }
                }
                .padding(.vertical, 6)
                .accessibilityElement(children: .combine)
            }
            if let text = turn.result?.text, !text.isEmpty {
                MarkdownView(text: text).padding(.vertical, 6)
            }
            if let diff = turn.turnDiff {
                ChangesCard(set: diff) { path in onOpenDiff(run.id, path) }
            }
            stats
            footerActions
        }
    }

    // MARK: The message

    @ViewBuilder private var opening: some View {
        if run.movedFrom != nil {
            HStack(spacing: 6) {
                Image(systemName: "arrow.left.arrow.right").font(.system(size: 11))
                (Text("Moved\(turn.movedFromMachine.map { " from \($0)" } ?? "") to \(turn.machine ?? "another machine")")
                    + (run.resumeFromBranch.map { Text(", continuing from ") + Text($0).font(SF.mono(12)) } ?? Text(", starting over (no work had been pushed)")))
            }
            .font(SF.font(13))
            .foregroundStyle(SF.muted)
            .padding(.bottom, 8)
        } else if run.kind == .restore || run.kind == .pr || run.kind == .integrate {
            HStack(spacing: 6) {
                Image(systemName: run.kind == .pr ? "arrow.triangle.pull" : run.kind == .integrate ? "arrow.triangle.merge" : "arrow.counterclockwise").font(.system(size: 10))
                Text(run.task)
            }
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            .frame(maxWidth: .infinity)
            .padding(.bottom, 8)
        } else if !first {
            VStack(alignment: .trailing, spacing: 6) {
                Text(run.task)
                    .font(SF.font(14))
                    .lineSpacing(5)
                    .foregroundStyle(SF.inkInverse)
                    .textSelection(.enabled)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .background(SF.ink, in: UnevenRoundedRectangle(topLeadingRadius: 18, bottomLeadingRadius: 18, bottomTrailingRadius: 6, topTrailingRadius: 18, style: .continuous))
                    .frame(maxWidth: SF.column * 0.85, alignment: .trailing)
                if !run.images.isEmpty { ImageStrip(images: run.images) }
                if waiting {
                    HStack(spacing: 8) {
                        Text("Queued, sends when the current message finishes").font(SF.font(11)).foregroundStyle(SF.muted)
                        Button("Send now", action: onSendNow).buttonStyle(PrimaryButtonStyle(height: 22))
                        Button("Cancel") { cancel() }.buttonStyle(SecondaryButtonStyle(height: 22))
                    }
                } else {
                    Text(meta).font(SF.font(11)).foregroundStyle(SF.faint)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.bottom, 12)
        }
    }

    private var meta: String {
        [run.parentID == nil ? Format.clock(run.createdAt) : nil,
         run.permissionMode.map { Format.permName($0) },
         run.effort.map { "\(Format.effortName(agent: run.agent, effort: $0)) effort" },
         run.parentID == nil && run.isolation == "checkout" ? "In place" : nil].compactMap { $0 }.joined(separator: ", ")
    }

    @ViewBuilder private var setupLine: some View {
        if let start = run.startAt, run.isScheduled(now: stores.fleet.now) {
            HStack(spacing: 8) {
                Image(systemName: "alarm").font(.system(size: 12))
                (Text("Scheduled to start ") + Text(Format.startTime(start, now: stores.fleet.now)).foregroundColor(SF.ink.opacity(0.8))
                    + Text(" (\(Format.until(start, now: stores.fleet.now))) on \(run.pinnedWorkerID ?? "a machine with \(Format.agentName(run.agent)) and \(Format.repoLabel(run.repo))")"))
                if !theirs {
                    Button("Start now") { startNow() }.buttonStyle(PrimaryButtonStyle(height: 22))
                    Button("Cancel") { cancel() }.buttonStyle(SecondaryButtonStyle(height: 22))
                }
            }
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            .padding(.bottom, 8)
        } else if !waiting, run.kind == .agent, (turn.activity == "queued" && turn.machine == nil) || (turn.machine != nil && (relocated || switched)) {
            SetupLine(turn: turn, relocated: relocated)
                .padding(.bottom, 6)
        }
    }

    @ViewBuilder private func row(_ group: ItemGroup) -> some View {
        switch group {
        case .text(_, let text):
            MarkdownView(text: text).padding(.vertical, 6)
        case .tools(_, let tools):
            ToolGroupView(tools: tools, live: live)
        case .approval(let item):
            ApprovalCard(item: item, agent: run.agent, live: live, onDecide: theirs ? nil : { item, approve in
                try await thread.decide(item, approve: approve)
                await stores.fleet.refresh()
            })
        case .note(let note):
            NoteRow(note: note)
        case .log(_, let lines):
            LogRow(lines: lines)
        }
    }

    @ViewBuilder private var stats: some View {
        if let result = turn.result {
            HStack(spacing: 12) {
                if !result.ok {
                    Text("\(Format.agentName(run.agent)) reported an error\(!result.subtype.isEmpty && result.subtype != "error" ? " (\(result.subtype))" : "")").foregroundStyle(SF.errInk)
                }
                if let ms = result.durationMs { Text(String(format: "%.1fs", ms / 1000)) }
                if let turns = result.turns { Text("\(turns) turn\(turns == 1 ? "" : "s")") }
                if let cost = result.cost {
                    Text(String(format: "$%.4f", cost))
                } else if let tokens = result.tokens {
                    Text("\(tokens.formatted()) tokens")
                }
            }
            .font(SF.font(12))
            .monospacedDigit()
            .foregroundStyle(SF.muted)
            .padding(.top, 8)
        }
    }

    @ViewBuilder private var footerActions: some View {
        if finished, !live, run.kind != .pr, run.kind != .integrate {
            HStack(spacing: 4) {
                Spacer()
                if !thread.live, !theirs, run.isolation == "worktree", turn.hasSnapshot {
                    Button(action: restore) {
                        Label(restoring ? "Restoring…" : confirmingRestore ? "Click again to restore" : "Restore to before", systemImage: "arrow.counterclockwise")
                            .font(SF.font(12))
                            .foregroundStyle(confirmingRestore ? SF.errInk : SF.muted)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2)
                            .background(confirmingRestore ? SF.errSoft : .clear, in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .disabled(restoring)
                    .help("Put the files back to how they were before this message. The conversation carries on.")
                }
                Button { onFork(turn) } label: {
                    Label("Fork from here", systemImage: "arrow.triangle.branch").font(SF.font(12)).padding(.horizontal, 8).padding(.vertical, 2)
                }
                .buttonStyle(QuietButtonStyle())
                .help("Start a new thread from here, with any agent and model")
            }
            .padding(.top, 4)
        }
    }

    // MARK: Actions

    private func restore() {
        guard confirmingRestore else {
            confirmingRestore = true
            Task {
                try? await Task.sleep(for: .seconds(4))
                confirmingRestore = false
            }
            return
        }
        restoring = true
        Task {
            do {
                try await thread.restore(run.id)
                await stores.fleet.refresh()
            } catch {
                stores.toasts.error("Could not restore", error)
            }
            restoring = false
            confirmingRestore = false
        }
    }

    private func cancel() {
        Task {
            do {
                try await thread.cancel(run.id)
            } catch {
                stores.toasts.error("Could not cancel", error)
            }
        }
    }

    private func startNow() {
        Task {
            do {
                try await thread.startNow(run.id)
                await stores.fleet.refresh()
            } catch {
                stores.toasts.error("Could not start it", error)
            }
        }
    }
}

/// "Started on hetzner-vps, sidefleet/… with Claude Code · Opus 5.5". Opens to the exact command.
private struct SetupLine: View {
    var turn: Turn
    var relocated: Bool
    @State private var open = false

    private var run: Run { turn.run }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                if turn.command != nil { withAnimation(.easeOut(duration: 0.15)) { open.toggle() } }
            } label: {
                line.font(SF.font(12)).multilineTextAlignment(.leading)
            }
            .buttonStyle(ToolTextButtonStyle())
            if open, let command = turn.command {
                Text(command)
                    .font(SF.mono(11.5))
                    .foregroundStyle(SF.muted)
                    .textSelection(.enabled)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(SF.code, in: RoundedRectangle(cornerRadius: 6))
            }
        }
    }

    private var line: Text {
        if turn.activity == "queued", turn.machine == nil {
            return Text("Waiting for \(run.pinnedWorkerID ?? "\(run.movedFrom != nil ? "another" : "a") machine with \(Format.agentName(run.agent)) and \(Format.repoLabel(run.repo))")…")
        }
        if !relocated { return Text("Switched to \(Format.agentModel(run.agent, run.model))") }
        var t = Text("\(run.parentID != nil ? "Continued" : "Started") on ") + Text(turn.machine ?? "").foregroundColor(SF.ink.opacity(0.8))
        if let branch = turn.branch {
            t = t + Text("  ") + Text(Image(systemName: "arrow.triangle.branch")) + Text(" ") + Text(branch).font(SF.mono(12))
            if let ref = turn.startedFromRef, let commit = turn.startedFromCommit {
                t = t + Text(" from ") + Text(ref + " @ " + String(commit.prefix(7))).font(SF.mono(12))
            }
        } else if let note = turn.workspaceNote {
            t = t + Text(" \(note)")
        }
        if let checkpoint = turn.checkpoint {
            t = t + Text(" pushed ") + Text(String(checkpoint.prefix(7))).font(SF.mono(12))
        }
        return t + Text(" with \(Format.agentModel(run.agent, turn.sessionModel ?? run.model))")
    }
}

private struct NoteRow: View {
    var note: Note

    private var color: Color {
        switch note.tone {
        case .err: SF.errInk
        case .ok: SF.okInk
        case .muted: SF.muted
        }
    }

    var body: some View {
        Group {
            if let href = note.href, let url = Format.safeLink(href) {
                Link(destination: url) { Text(note.text).underline() }
            } else {
                Text(note.text).textSelection(.enabled)
            }
        }
        .font(SF.font(13))
        .foregroundStyle(color)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.vertical, 4)
    }
}

private struct LogRow: View {
    var lines: [String]
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.easeOut(duration: 0.15)) { open.toggle() }
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.right").font(.system(size: 8.5, weight: .semibold)).rotationEffect(.degrees(open ? 90 : 0))
                    Text("\(lines.count) line\(lines.count == 1 ? "" : "s") of agent log").font(SF.font(12))
                }
            }
            .buttonStyle(ToolTextButtonStyle())
            if open {
                OutputBlock(text: lines.joined(separator: "\n"), maxHeight: 190)
            }
        }
        .padding(.vertical, 2)
    }
}

// MARK: Fork

/// Fork a thread at one message into a new thread, with any agent and model.
struct ForkSheet: View {
    var request: ForkRequest
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var model: ForkModel?
    @State private var height: CGFloat = 96

    var body: some View {
        Group {
            if let model { form(model) } else { Color.clear.frame(width: 560, height: 300) }
        }
        .onAppear {
            if model == nil { model = ForkModel(run: request.run, hasFiles: request.hasFiles, fleet: stores.fleet, prefs: stores.prefs) }
        }
    }

    private func form(_ m: ForkModel) -> some View {
        @Bindable var m = m
        return VStack(alignment: .leading, spacing: 12) {
            Text(m.theirs ? "Continue on my machine" : "Fork into a new thread").font(SF.font(15, .semibold))
            Text(m.explanation).font(SF.font(13)).lineSpacing(3).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
            FlowRow(spacing: 2) {
                AgentModelPicker(
                    agent: m.agent, model: m.model, effort: m.effort, models: m.models,
                    unavailable: { m.unavailable($0) }, onAgent: { m.pick(agent: $0) }, onModel: { m.model = $0 }, onEffort: { m.effort = $0 }
                )
                PickerPill(options: machineOptions(m), selected: m.chosen?.id ?? "", width: 320, accessibility: "Machine", onPick: { m.machine = $0 == m.filesOn ? "" : $0 }) {
                    Text(m.chosen?.displayName ?? m.filesOn.map { "\($0) (has the files)" } ?? "Any machine").font(SF.font(13)).lineLimit(1)
                }
                PickerPill(
                    options: Format.perms(for: m.agent).map { PickerOption(id: $0.id, title: $0.name, hint: Format.permHint($0.id, agent: m.agent)) },
                    selected: m.effectivePerm, width: 300, accessibility: "Permissions", onPick: { m.perm = $0 }
                ) {
                    Text(Format.permName(m.effectivePerm)).font(SF.font(13))
                }
            }
            ZStack(alignment: .topLeading) {
                if m.task.isEmpty {
                    EditorPlaceholder(text: "What should \(Format.agentName(m.agent)) do next?", fontSize: 14, inset: CGSize(width: 14, height: 12))
                }
                MessageEditor(
                    text: $m.task, height: $height, fontSize: 14, minHeight: 96, maxHeight: 240, inset: NSSize(width: 14, height: 12),
                    enterSends: { false }, onSend: { submit(m) }, onPasteImages: { _ in }, accessibility: "First message of the new thread", focusTick: 1
                )
                .frame(height: height)
            }
            .card(cornerRadius: 12)
            HStack(spacing: 12) {
                Text(m.status.text)
                    .font(SF.font(12))
                    .foregroundStyle(m.status.tone == .warning ? SF.errInk : SF.muted)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button("Cancel") { dismiss() }
                    .buttonStyle(SecondaryButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button {
                    submit(m)
                } label: {
                    Label(m.busy ? "Forking…" : "Fork", systemImage: "arrow.triangle.branch")
                }
                .buttonStyle(PrimaryButtonStyle())
                .disabled(!m.can)
            }
        }
        .padding(20)
        .frame(width: 560)
    }

    private func machineOptions(_ m: ForkModel) -> [PickerOption] {
        var out: [PickerOption] = []
        if m.filesOn == nil {
            out.append(PickerOption(id: "", title: "Any machine", hint: "The first one with \(Format.repoLabel(m.run.repo)) and a free slot"))
        }
        for w in m.choices {
            let hint = w.id == m.filesOn ? "Starts with the files as they were after this message"
                : m.filesOn != nil ? "Starts from \(Format.repoLabel(m.run.repo)) as it is there, without this thread's files"
                : w.capabilities.slots.map { "\($0.active) of \($0.parallel) slots busy" } ?? "Has the repo"
            out.append(PickerOption(id: w.id, title: w.displayName + (w.online ? "" : " (offline)"), hint: hint))
        }
        return out
    }

    private func submit(_ m: ForkModel) {
        Task {
            if let id = await m.submit() {
                dismiss()
                stores.open(id)
            }
        }
    }
}
