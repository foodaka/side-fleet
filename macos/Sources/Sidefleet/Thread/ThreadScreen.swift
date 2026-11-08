import AppKit
import SidefleetKit
import SwiftUI

/// What a window identifies a thread by, so it can be restored.
struct ThreadWindowID: Hashable, Codable {
    var id: String
}

/// A thread: its header, its turns, and the box to reply in. Used in the main window and on its own.
struct ThreadScreen: View {
    var runID: String
    @Environment(Stores.self) private var stores
    @State private var thread: ThreadStore?
    @State private var reply: ReplyModel?

    var body: some View {
        Group {
            if let thread, let reply {
                ThreadContent(thread: thread, reply: reply)
            } else {
                ThreadSkeleton()
            }
        }
        .background(SF.background)
        .task(id: runID) {
            thread?.dispose()
            let t = ThreadStore(api: stores.api)
            t.onRunEnded = { [fleet = stores.fleet] in Task { await fleet.refresh() } }
            thread = t
            let model = ReplyModel(thread: t, toasts: stores.toasts)
            model.text = stores.replyDrafts[runID] ?? ""
            reply = model
            await t.load(runID)
            reply?.syncFromThread()
            // Links to a follow-up land on its thread; stay on the root.
            if let root = t.root?.id, root != runID, stores.openThreadID == runID { stores.route = .thread(root) }
        }
        .onDisappear {
            thread?.dispose()
            stores.replyDrafts[runID] = reply?.text
        }
    }
}

private struct ThreadSkeleton: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SkeletonBar(width: 400, height: 22)
            SkeletonBar(width: 220, height: 14)
            Spacer()
        }
        .column()
        .padding(.horizontal, 24)
        .padding(.top, 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct ThreadContent: View {
    var thread: ThreadStore
    @Bindable var reply: ReplyModel
    @Environment(Stores.self) private var stores
    @State private var reviewing: ReviewRequest?
    @State private var forking: ForkRequest?
    @State private var askingStop = false
    @State private var dontAskAgain = false
    @State private var opening = false
    @State private var composerFocus = 1
    @State private var forkSource: String?

    private var fleet: FleetStore { stores.fleet }
    private var root: Run? { thread.root }
    /// Someone else's task: it runs on their machine, so you watch it and can continue it on yours.
    private var theirs: Bool { root.map { fleet.othersRun($0.createdBy) } ?? false }
    private var machineOnline: Bool { fleet.workers.first { $0.id == thread.machine }?.online ?? false }
    /// The list's copy is re-checked by the machine, the thread's is set the moment one opens.
    private var pr: PullRequest? { fleet.runs.first { $0.id == root?.id }?.run.pr ?? root?.pr }

    private var actions: ThreadActions {
        ThreadActions(
            stop: thread.live && !theirs ? { askStop() } : nil,
            review: thread.changes != nil ? { reviewing = ReviewRequest(scope: "all", focus: nil) } : nil,
            copyBranch: thread.branch.map { branch in {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(branch, forType: .string)
            } }
        )
    }

    var body: some View {
        Group {
            if let reviewing {
                ReviewScreen(thread: thread, request: reviewing, onClose: { self.reviewing = nil })
                    .transition(.opacity)
            } else {
                conversation
                    .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.15), value: reviewing == nil)
        .focusedSceneValue(\.threadActions, actions)
        .sheet(item: $forking) { ForkSheet(request: $0) }
        .onChange(of: thread.latest?.agent) { _, _ in reply.syncFromThread() }
        .onChange(of: stores.forkThread) { _, id in askForkFromSidebar(id) }
        .onChange(of: thread.loaded) { _, _ in askForkFromSidebar(stores.forkThread) }
        .task(id: root?.forkedFrom) {
            // The title of the thread this one was forked from.
            forkSource = nil
            if let from = root?.forkedFrom, let t = try? await stores.api.thread(from) { forkSource = t.first?.task }
        }
        .sheet(isPresented: $askingStop) { stopSheet }
    }

    private func askForkFromSidebar(_ id: String?) {
        guard thread.loaded, let id, id == root?.id else { return }
        stores.forkThread = nil
        if let t = thread.forkTurn {
            forking = ForkRequest(run: t.run, hasFiles: t.hasSnapshot)
        } else {
            stores.toasts.show("Nothing to fork yet", detail: "Fork once the first message has finished.")
        }
    }

    private var conversation: some View {
        VStack(spacing: 0) {
            header
            Divider()
            TurnList(thread: thread, theirs: theirs, pr: pr, machineOnline: machineOnline, opening: opening,
                     onOpenDiff: { run, path in
                         reviewing = ReviewRequest(scope: run ?? "all", focus: path)
                     },
                     onFork: { forking = ForkRequest(run: $0.run, hasFiles: $0.hasSnapshot) },
                     onOpenPR: openPR, onSendNow: sendNow)
            Divider()
            if let root, theirs { teammateBar(root) } else if root != nil { composer }
        }
        .modifier(DropTarget(enabled: reply.canReply && !theirs) { attach($0, to: &reply.images, toasts: stores.toasts) })
    }

    // MARK: Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let error = thread.error {
                Text("This run doesn't exist").font(SF.font(19, .medium))
                Text(error).font(SF.font(13)).foregroundStyle(SF.muted)
            } else if let root, let latest = thread.current {
                HStack(alignment: .top, spacing: 16) {
                    Text(root.task)
                        .font(SF.font(19, .medium))
                        .tracking(-0.25)
                        .lineLimit(3)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if thread.live, theirs, fleet.isOwner {
                        // Your own tasks stop from the composer; an owner watching a teammate's has none.
                        Button { askStop() } label: {
                            Label(reply.stopping || latest.stopRequested ? "Stopping…" : "Stop", systemImage: "stop.fill")
                        }
                        .buttonStyle(SecondaryButtonStyle(tint: SF.errInk))
                        .disabled(reply.stopping || latest.stopRequested)
                    }
                }
                meta(root: root, latest: latest)
            } else {
                SkeletonBar(width: 420, height: 22)
                SkeletonBar(width: 220, height: 14)
            }
        }
        .column()
        .padding(.horizontal, 24)
        .padding(.top, 14)
        .padding(.bottom, 14)
    }

    private func meta(root: Run, latest: Run) -> some View {
        FlowRow(spacing: 12) {
            if let start = latest.startAt, latest.isScheduled(now: fleet.now) {
                StatusPill(status: .queued, label: "Scheduled", extra: Format.startTime(start, now: fleet.now))
            } else if latest.status == .running {
                LiveText { now in
                    StatusPill(status: .running, extra: Format.duration(start: latest.startedAt, now: now))
                }
            } else {
                StatusPill(status: latest.status)
            }
            if fleet.multi, root.createdBy != nil {
                let login = fleet.login(root.createdBy)
                HStack(spacing: 6) {
                    InitialAvatar(login: login)
                    Text(theirs ? "@\(login ?? "teammate")" : "You").foregroundStyle(SF.ink.opacity(0.8))
                }
                .help(theirs ? "Started by @\(login ?? "a teammate"); it runs on their machine" : "Your task")
            }
            if let machine = thread.machine {
                HStack(spacing: 6) {
                    StatusDot(status: machineOnline ? .online : .offline)
                    Text(machine).foregroundStyle(SF.ink.opacity(0.8))
                }
            }
            Text(Format.repoLabel(root.repo)).font(SF.mono(11.5))
            if let branch = thread.branch {
                HStack(spacing: 4) {
                    Image(systemName: "arrow.triangle.branch").font(.system(size: 10))
                    Text(branch).font(SF.mono(11.5)).foregroundStyle(SF.ink.opacity(0.8)).lineLimit(1).textSelection(.enabled)
                    CopyButton(text: { branch }, size: 10)
                }
                .padding(.leading, 6)
                .padding(.trailing, 2)
                .frame(height: 22)
                .background(SF.fill, in: RoundedRectangle(cornerRadius: 6))
            }
            if let pr, let url = URL(string: pr.url) {
                Link(destination: url) {
                    HStack(spacing: 4) {
                        PRIcon(state: pr.state)
                        Text("#\(pr.number)").monospacedDigit().foregroundStyle(SF.ink.opacity(0.8))
                        Text(pr.state.capitalized)
                    }
                    .padding(.horizontal, 6)
                    .frame(height: 22)
                    .background(SF.fill, in: RoundedRectangle(cornerRadius: 6))
                }
                .help("Open pull request #\(pr.number) on GitHub")
            }
            if thread.runs.count > 1 { Text("\(thread.runs.count) messages").monospacedDigit() }
            if let from = root.forkedFrom {
                Button { stores.open(from) } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "arrow.triangle.branch").font(.system(size: 10))
                        Text("Forked from")
                        Text(forkSource ?? "another thread").foregroundStyle(SF.ink.opacity(0.8)).lineLimit(1).frame(maxWidth: 220, alignment: .leading)
                    }
                }
                .buttonStyle(.plain)
            }
            if let changes = thread.changes {
                Button { reviewing = ReviewRequest(scope: "all", focus: nil) } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "doc.badge.gearshape").font(.system(size: 11))
                        Text("\(changes.fileCount) file\(changes.fileCount == 1 ? "" : "s")").monospacedDigit().foregroundStyle(SF.ink.opacity(0.8))
                        DiffStatView(add: changes.additions, del: changes.deletions, blocks: false, size: 11)
                    }
                    .padding(.horizontal, 8)
                    .frame(height: 22)
                    .background(SF.card, in: RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(SF.border))
                }
                .buttonStyle(.plain)
                .help("Review changes  ⇧⌘D")
            }
        }
        .font(SF.font(12))
        .foregroundStyle(SF.muted)
    }

    // MARK: Composer

    private var composer: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                ZStack(alignment: .topLeading) {
                    if reply.text.isEmpty {
                        EditorPlaceholder(text: reply.placeholder, fontSize: 14, inset: CGSize(width: 16, height: 12))
                    }
                    ReplyEditor(reply: reply, focusTick: composerFocus, send: send)
                }
                AttachmentStrip(images: $reply.images).padding(.horizontal, 16).padding(.bottom, 6)
                HStack(spacing: 2) {
                    AgentModelPicker(
                        agent: reply.agent, model: reply.model, effort: reply.effort, models: Format.machineModels(host, agent: reply.agent), small: true,
                        unavailable: { a in host.isEmpty || host.contains { $0.hasAgent(a) } ? nil : "Not on \(thread.machine ?? "that machine")" },
                        onAgent: { reply.pick(agent: $0) }, onModel: { reply.model = $0 }, onEffort: { reply.effort = $0 }
                    )
                    ComposerOptions(
                        agent: reply.agent, perm: reply.replyPerm, onPerm: { reply.perm = $0 }, defaultPerm: "",
                        sameAs: .some(thread.current?.permissionMode), failover: .constant(false), small: true
                    )
                    Text(reply.hint)
                        .font(SF.font(12))
                        .foregroundStyle(SF.faint)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .help(reply.hint)
                        .padding(.horizontal, 4)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if let context = thread.context, context.agent == reply.agent {
                        ContextRing(fill: context.fill, agent: context.agent)
                    }
                    Button {
                        attach(ImageImport.pickFiles(), to: &reply.images, toasts: stores.toasts)
                    } label: {
                        Image(systemName: "paperclip").font(.system(size: 13)).frame(width: 32, height: 32)
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(!reply.canReply)
                    .help("Attach images (or drop or paste them)")
                    .accessibilityLabel("Attach images")
                    sendOrStop
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
            }
            .background(SF.card, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(SF.input))
            .column()
            .padding(.horizontal, 24)
            .padding(.top, 12)
            .padding(.bottom, 16)
        }
        .background(SF.background)
        .focusedSceneValue(\.focusComposer) { composerFocus += 1 }
    }

    /// While it runs and the box is empty, Stop stands in for Send; a draft only gets Queue.
    @ViewBuilder private var sendOrStop: some View {
        let empty = reply.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        if thread.live, empty {
            let stopping = reply.stopping || (thread.current?.stopRequested ?? false)
            Button { askStop() } label: {
                Image(systemName: "stop.circle").font(.system(size: 16))
            }
            .buttonStyle(RoundIconButtonStyle(size: 32, background: SF.errSoft, foreground: SF.errInk))
            .disabled(stopping)
            .help(stopping ? "Stopping…" : "Stop this message and any queued after it  ⌘.")
            .accessibilityLabel(stopping ? "Stopping" : "Stop")
        } else {
            Button(action: send) { Image(systemName: "arrow.up") }
                .buttonStyle(RoundIconButtonStyle(size: 32))
                .disabled(!reply.canSend)
                .help(thread.live ? "Queue: sends when the current message finishes" : "Send")
                .accessibilityLabel(thread.live ? "Queue" : "Send")
        }
    }

    private var host: [Worker] { fleet.workers.filter { $0.id == thread.machine } }

    private func send() {
        Task {
            await reply.send()
            await fleet.refresh()
        }
    }

    private func sendNow() {
        Task {
            do {
                try await thread.interrupt()
            } catch {
                stores.toasts.error("Could not interrupt", error)
            }
        }
    }

    private func openPR() {
        opening = true
        Task {
            do {
                try await thread.openPR()
                await fleet.refresh()
            } catch {
                stores.toasts.error("Could not open a pull request", error)
            }
            opening = false
        }
    }

    // MARK: Stopping

    /// Stopping asks first, unless you said not to.
    private func askStop() {
        if stores.prefs.confirmStop {
            dontAskAgain = false
            askingStop = true
        } else {
            stop()
        }
    }

    private func stop() {
        reply.stopping = true
        Task {
            do {
                try await thread.stop()
            } catch {
                stores.toasts.error("Could not stop", error)
            }
            reply.stopping = false
        }
    }

    private var stopSheet: some View {
        let queued = thread.queuedCount
        return VStack(alignment: .leading, spacing: 12) {
            Text("Stop this task?").font(SF.font(15, .semibold))
            Text("\(Format.agentName(thread.latest?.agent)) stops where it is. The files it changed so far stay as they are, and you can reply later to pick up from there." + (queued > 0 ? " Your \(queued == 1 ? "queued message is" : "\(queued) queued messages are") dropped too." : ""))
                .font(SF.font(13))
                .lineSpacing(3)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            Toggle("Don’t ask again", isOn: $dontAskAgain)
                .toggleStyle(.checkbox)
                .font(SF.font(13))
                .foregroundStyle(SF.muted)
            HStack {
                Spacer()
                Button("Keep running") { askingStop = false }
                    .buttonStyle(SecondaryButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button {
                    askingStop = false
                    if dontAskAgain {
                        stores.prefs.set(\.confirmStop, false)
                        stores.toasts.show("Stop won’t ask again", detail: "Turn it back on in Settings.")
                    }
                    stop()
                } label: {
                    Label("Stop", systemImage: "stop.fill")
                        .font(SF.font(13, .medium))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .frame(height: 32)
                        .background(SF.err, in: Capsule())
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 400)
    }

    private func teammateBar(_ root: Run) -> some View {
        let login = fleet.login(root.createdBy)
        return HStack(spacing: 12) {
            InitialAvatar(login: login, size: 22)
            Text("This is \(login.map { "@\($0)'s" } ?? "a teammate's") task. It runs on their machine, so only they can reply to it.")
                .font(SF.font(13))
                .foregroundStyle(SF.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                if let t = thread.forkTurn { forking = ForkRequest(run: t.run, hasFiles: t.hasSnapshot) }
            } label: {
                Label("Continue on my machine", systemImage: "arrow.triangle.branch")
            }
            .buttonStyle(PrimaryButtonStyle(height: 36))
            .disabled(thread.forkTurn == nil)
            .help(thread.forkTurn != nil ? "A new thread of yours that knows this conversation, on one of your machines" : "Once its first message finishes")
        }
        .column()
        .padding(.horizontal, 24)
        .padding(.vertical, 14)
    }
}

/// Holds the reply box's height, so typing doesn't redraw the whole thread.
private struct ReplyEditor: View {
    @Bindable var reply: ReplyModel
    var focusTick: Int
    var send: () -> Void
    @Environment(Stores.self) private var stores
    @State private var height: CGFloat = 44

    var body: some View {
        MessageEditor(
            text: $reply.text, height: $height, fontSize: 14, minHeight: 44, maxHeight: 220,
            inset: NSSize(width: 16, height: 12), enabled: reply.canReply,
            enterSends: { stores.prefs.enterSends(.reply) }, onSend: send,
            onPasteImages: { attach($0, to: &reply.images, toasts: stores.toasts) },
            accessibility: "Reply", focusTick: focusTick
        )
        .frame(height: height)
    }
}

/// Wraps its children onto more lines, like the page's flex-wrap.
struct FlowRow: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, line: CGFloat = 0, widest: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                y += line + lineSpacing
                x = 0
                line = 0
            }
            x += size.width + spacing
            line = max(line, size.height)
            widest = max(widest, x - spacing)
        }
        return CGSize(width: width == .infinity ? widest : width, height: y + line)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, line: CGFloat = 0
        // Centre each line's items on each other: pills and plain text sit on one axis.
        var rows: [[(LayoutSubview, CGSize)]] = [[]]
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                rows.append([])
                x = bounds.minX
            }
            rows[rows.count - 1].append((view, size))
            x += size.width + spacing
        }
        for row in rows {
            x = bounds.minX
            line = row.map(\.1.height).max() ?? 0
            for (view, size) in row {
                view.place(at: CGPoint(x: x, y: y + (line - size.height) / 2), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += line + lineSpacing
        }
    }
}
