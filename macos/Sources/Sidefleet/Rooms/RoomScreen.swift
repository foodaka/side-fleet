import AppKit
import SidefleetKit
import SwiftUI

/// One room: the goal, the tasks people took (each on its owner's machine), and the team's notes.
struct RoomScreen: View {
    let roomID: String
    @Environment(Stores.self) private var stores
    @State private var store: RoomStore?
    @State private var model: RoomTaskModel?

    var body: some View {
        Group {
            if let store, let model {
                if let missing = store.missing {
                    VStack(alignment: .leading, spacing: 12) {
                        Text("Room not found").font(SF.font(20, .medium))
                        Text(missing).font(SF.font(13)).foregroundStyle(SF.muted)
                        Button("All rooms") { stores.route = .rooms }
                            .buttonStyle(LinkButtonStyle(color: SF.ink))
                            .font(SF.font(13))
                            .underline()
                    }
                    .column()
                    .padding(.horizontal, 28)
                    .padding(.vertical, 64)
                    .frame(maxHeight: .infinity, alignment: .top)
                } else if let room = store.room {
                    RoomContent(store: store, model: model, room: room)
                } else {
                    skeleton
                }
            } else {
                skeleton
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SF.background)
        // Poll while the room is open and the app is in front: tasks move, notes arrive.
        .task(id: roomID) {
            let s = RoomStore(id: roomID, fleet: stores.fleet)
            store = s
            model = RoomTaskModel(fleet: stores.fleet, prefs: stores.prefs)
            while !Task.isCancelled {
                if stores.fleet.loaded, stores.appActive || s.data == nil { await s.load() }
                try? await Task.sleep(for: .seconds(stores.fleet.loaded ? 3 : 0.5))
            }
        }
    }

    private var skeleton: some View {
        VStack(alignment: .leading, spacing: 16) {
            SkeletonBar(width: 320, height: 28)
            SkeletonBar(height: 64)
            SkeletonBar(height: 190)
        }
        .column(SF.wideColumn)
        .padding(.horizontal, 28)
        .padding(.top, 28)
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

private struct RoomContent: View {
    var store: RoomStore
    @Bindable var model: RoomTaskModel
    var room: Room
    @Environment(Stores.self) private var stores

    private enum Editing { case name, goal }
    @State private var editing: Editing?
    @State private var draft = ""
    @FocusState private var draftFocused: Bool

    var body: some View {
        GeometryReader { geo in
            // Side by side when there's room for both; the notes go under the tasks when there isn't.
            let wide = geo.size.width >= 860
            HStack(alignment: .top, spacing: 0) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 24) {
                        header
                        tasks
                        if !wide {
                            RoomNotes(store: store, room: room).frame(height: 420)
                        }
                    }
                    .frame(maxWidth: wide ? 700 : SF.wideColumn, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: wide ? .trailing : .center)
                    .padding(.leading, 28)
                    .padding(.trailing, wide ? 12 : 28)
                    .padding(.top, 20)
                    .padding(.bottom, 28)
                }
                if wide {
                    RoomNotes(store: store, room: room)
                        .frame(width: 320)
                        .padding(.leading, 12)
                        .padding(.trailing, 28)
                        .padding(.vertical, 20)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .layoutPriority(-1)
                }
            }
        }
        .modifier(DropTarget(enabled: room.archivedAt == nil) { attach($0, to: &model.images, toasts: stores.toasts) })
        .onAppear { model.sync(room) }
        .onChange(of: stores.fleet.workers) { _, _ in model.sync(room) }
        .onChange(of: stores.prefs.roomPermission) { _, _ in model.resetFromPrefs() }
    }

    // MARK: Header

    private func edit(_ what: Editing) {
        draft = what == .name ? room.name : room.goal
        editing = what
        draftFocused = true
    }

    private func saveEdit() {
        guard let what = editing else { return }
        let value = draft
        Task {
            let saved = what == .name ? await store.save(name: value) : await store.save(goal: value)
            if saved { editing = nil }
        }
    }

    private func setArchived(_ archived: Bool) {
        Task {
            if await store.setArchived(archived), archived {
                await stores.rooms.load()
                stores.route = .rooms
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button {
                stores.route = .rooms
            } label: {
                Label("Rooms", systemImage: "arrow.left").font(SF.font(12))
            }
            .buttonStyle(LinkButtonStyle())

            HStack(alignment: .top, spacing: 12) {
                if editing == .name {
                    TextField("Room name", text: $draft)
                        .font(SF.font(18, .semibold))
                        .modifier(BoxedField())
                        .focused($draftFocused)
                        .onChange(of: draft) { _, v in if v.count > 120 { draft = String(v.prefix(120)) } }
                        .onSubmit(saveEdit)
                        .onExitCommand { editing = nil }
                        .accessibilityLabel("Room name")
                    Button("Save", action: saveEdit).buttonStyle(PrimaryButtonStyle())
                } else {
                    (Text(room.name).font(SF.font(22, .semibold)).tracking(-0.3)
                        + Text(room.archivedAt != nil ? "  archived" : "").font(SF.font(12)).foregroundColor(SF.muted))
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                    Spacer(minLength: 0)
                    if let compare = store.compareURL {
                        Button {
                            NSWorkspace.shared.open(compare)
                        } label: {
                            Label("Compare on GitHub", systemImage: "arrow.triangle.pull")
                        }
                        .buttonStyle(SecondaryButtonStyle())
                        .help("Open the room branch on GitHub, where a pull request is one click away")
                    }
                    if store.canEdit {
                        Menu {
                            Button("Rename") { edit(.name) }
                            Button("Edit goal") { edit(.goal) }
                            Divider()
                            Button(room.archivedAt != nil ? "Restore room" : "Archive room") { setArchived(room.archivedAt == nil) }
                        } label: {
                            Image(systemName: "ellipsis").font(.system(size: 13)).frame(width: 32, height: 32)
                        }
                        .menuStyle(.borderlessButton)
                        .menuIndicator(.hidden)
                        .fixedSize()
                        .foregroundStyle(SF.muted)
                        .accessibilityLabel("Room actions")
                    }
                }
            }

            goal

            FlowRow(spacing: 12, lineSpacing: 6) {
                if let login = room.createdByLogin {
                    HStack(spacing: 6) {
                        InitialAvatar(login: login, size: 16)
                        Text("Started by @\(login)")
                    }
                }
                let clone = Format.repoCloneCommand(room.repo)
                if clone.isEmpty {
                    Text(Format.repoLabel(room.repo)).font(SF.mono(11.5))
                } else {
                    CopyChip(text: Format.repoLabel(room.repo), copy: clone, help: "Copy a git clone command for this repo")
                }
                CopyChip(text: room.branch, copy: room.branch, symbol: "arrow.triangle.branch", help: "Copy the room branch's name")
                Text(room.integratedAt != nil
                    ? "\(room.integratedCount) \(room.integratedCount == 1 ? "task" : "tasks") merged; new tasks start from this branch"
                    : "Nothing added yet; tasks start from the default branch")
            }
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
        }
    }

    @ViewBuilder private var goal: some View {
        if editing == .goal {
            VStack(alignment: .leading, spacing: 8) {
                TextField("What done looks like, constraints, decisions already made. Every agent in the room reads this.", text: $draft, axis: .vertical)
                    .font(SF.font(14))
                    .lineSpacing(4)
                    .lineLimit(5...14)
                    .modifier(BoxedField())
                    .focused($draftFocused)
                    .onChange(of: draft) { _, v in if v.count > 4000 { draft = String(v.prefix(4000)) } }
                    .onExitCommand { editing = nil }
                    .accessibilityLabel("Goal")
                HStack(spacing: 8) {
                    Button("Save goal", action: saveEdit).buttonStyle(PrimaryButtonStyle())
                    Button("Cancel") { editing = nil }.buttonStyle(SecondaryButtonStyle())
                }
            }
        } else if !room.goal.isEmpty {
            HoverReveal { hovering in
                HStack(alignment: .top, spacing: 6) {
                    Text(room.goal)
                        .font(SF.font(14))
                        .lineSpacing(4)
                        .foregroundStyle(SF.ink.opacity(0.85))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if store.canEdit {
                        Button {
                            edit(.goal)
                        } label: {
                            Image(systemName: "pencil").font(.system(size: 11)).frame(width: 22, height: 22)
                        }
                        .buttonStyle(QuietButtonStyle(cornerRadius: 5))
                        .opacity(hovering ? 1 : 0)
                        .help("Edit goal")
                        .accessibilityLabel("Edit goal")
                    }
                }
            }
        } else if store.canEdit {
            Button("Add a goal: what done looks like. Every agent in the room reads it.") { edit(.goal) }
                .buttonStyle(LinkButtonStyle())
                .font(SF.font(13))
        }
    }

    // MARK: Tasks

    private var tasks: some View {
        VStack(alignment: .leading, spacing: 16) {
            RoomComposer(store: store, model: model, room: room)
            if store.tasks.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("No tasks yet").font(SF.font(13, .medium))
                    Text("Split the goal into tasks. Each runs on its owner's machine; finished work is added to the room branch, and new tasks start from it.")
                        .font(SF.font(13))
                        .lineSpacing(3)
                        .foregroundStyle(SF.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            } else {
                VStack(spacing: 10) {
                    ForEach(store.tasks) { RoomTaskCard(store: store, room: room, task: $0) }
                }
                .animation(.easeOut(duration: 0.2), value: store.tasks.map(\.id))
            }
        }
    }
}

// MARK: Composer

/// The box that starts a task in the room, on one of your own machines.
private struct RoomComposer: View {
    var store: RoomStore
    @Bindable var model: RoomTaskModel
    var room: Room
    @Environment(Stores.self) private var stores
    @State private var height: CGFloat = 44

    private var canAttach: Bool { room.archivedAt == nil }

    private func start() {
        Task {
            guard let id = await model.start(in: room) else { return }
            await store.load()
            let stores = stores
            stores.toasts.show("Task started", detail: "It runs on your machine with the room brief.", actionTitle: "Open") { stores.open(id) }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 0) {
                ZStack(alignment: .topLeading) {
                    if model.task.isEmpty {
                        EditorPlaceholder(text: "Take a piece of the goal: what should your agent do?", fontSize: 14, inset: CGSize(width: 12, height: 12))
                    }
                    MessageEditor(
                        text: $model.task, height: $height, fontSize: 14, minHeight: 44, maxHeight: 240,
                        inset: NSSize(width: 12, height: 12),
                        enterSends: { stores.prefs.enterSends(.task) }, onSend: start,
                        onPasteImages: { if canAttach { attach($0, to: &model.images, toasts: stores.toasts) } },
                        accessibility: "New task in this room"
                    )
                    .frame(height: height)
                }
                AttachmentStrip(images: $model.images).padding(.horizontal, 14).padding(.bottom, 8)
                HStack(alignment: .bottom, spacing: 6) {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 2) { pickers }.frame(maxWidth: .infinity, alignment: .leading)
                        VStack(alignment: .leading, spacing: 2) { pickers }.frame(maxWidth: .infinity, alignment: .leading)
                    }
                    Button {
                        attach(ImageImport.pickFiles(), to: &model.images, toasts: stores.toasts)
                    } label: {
                        Image(systemName: "paperclip").font(.system(size: 12)).frame(width: 28, height: 28)
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(!canAttach)
                    .help("Attach images (or drop or paste them)")
                    .accessibilityLabel("Attach images")
                    Button(action: start) {
                        Image(systemName: "arrow.up")
                    }
                    .buttonStyle(RoundIconButtonStyle(size: 30))
                    .disabled(!model.canStart(room))
                    .help(model.blocked(room) ?? "Start task")
                    .accessibilityLabel("Start task")
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
            }
            .background(SF.card, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(SF.input))

            if model.withRepo(room).isEmpty {
                noMachine
            } else {
                Text(model.hint(room))
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 4)
            }
        }
    }

    @ViewBuilder private var pickers: some View {
        AgentModelPicker(
            agent: model.agent, model: model.model, effort: model.effort, models: model.models(room), small: true,
            unavailable: { model.unavailable($0, in: room) },
            onAgent: { model.pick(agent: $0) }, onModel: { model.pick(model: $0) }, onEffort: { model.effort = $0 }
        )
        ComposerOptions(
            agent: model.agent, perm: model.effectivePerm, onPerm: { model.perm = $0 }, defaultPerm: model.defaultPerm,
            machines: model.machines(room), machine: model.pinned(room)?.id ?? "", onMachine: { model.machine = $0 },
            failover: .constant(false), small: true
        )
    }

    /// None of your machines can take a task here: say which and how to fix it.
    private var noMachine: some View {
        let mine = stores.fleet.myOnline
        let repo = Format.repoLabel(room.repo)
        let clone = Format.repoCloneCommand(room.repo)
        return HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle").font(.system(size: 12)).foregroundStyle(SF.queueInk).padding(.top, 1)
            VStack(alignment: .leading, spacing: 6) {
                if mine.isEmpty {
                    HStack(alignment: .firstTextBaseline, spacing: 4) {
                        Text("None of your machines is online.").font(SF.font(12, .medium)).foregroundStyle(SF.ink)
                        Button("Connect one") { stores.route = .machines }
                            .buttonStyle(LinkButtonStyle(color: SF.ink))
                            .underline()
                    }
                    Text("It needs \(repo) checked out to take a task; teammates' machines only run their own.")
                } else {
                    (Text(mine.count == 1 ? "\(mine[0].displayName) doesn't have \(repo) checked out," : "None of your online machines has \(repo) checked out,")
                        .font(SF.font(12, .medium)).foregroundColor(SF.ink)
                        + Text(" so it can't take a task here. Clone it into the folder Sidefleet scans for repos (the one passed with ")
                        + Text("--repos").font(SF.mono(11.5))
                        + Text("); it shows up here within about 15 seconds, no restart needed."))
                        .fixedSize(horizontal: false, vertical: true)
                    if !clone.isEmpty { CopyChip(text: clone, copy: clone) }
                }
            }
            .font(SF.font(12))
            .lineSpacing(3)
            .foregroundStyle(SF.muted)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SF.queueSoft.opacity(0.5), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.queue.opacity(0.3)))
    }
}

// MARK: Task card

private struct RoomTaskCard: View {
    var store: RoomStore
    var room: Room
    var task: TaskCard
    @Environment(Stores.self) private var stores

    private var mine: Bool { store.isMine(task) }
    private var busy: Bool { store.busyTask == task.id }
    private var integration: Integration? { task.integration }
    private var adding: Bool { integration?.adding ?? false }
    private var done: Bool { integration?.done ?? false }
    private var failed: Bool { integration?.failed ?? false }

    private var byline: String {
        var parts = [mine ? "You" : "@\(task.createdByLogin ?? "teammate")", Format.agentModel(task.agent, task.model)]
        if let worker = task.workerID { parts.append(worker) }
        parts.append(Format.rel(task.lastActivityAt, now: stores.fleet.now))
        return parts.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 10) {
                InitialAvatar(login: task.createdByLogin, size: 22).padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    Button {
                        stores.open(task.id)
                    } label: {
                        Text(task.title)
                            .font(SF.font(13.5, .medium))
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .buttonStyle(.plain)
                    .help(task.title)
                    Text(byline).font(SF.font(12)).foregroundStyle(SF.muted).lineLimit(1)
                }
                Spacer(minLength: 0)
                StatusPill(status: task.status)
            }

            if !task.files.isEmpty {
                HStack(spacing: 6) {
                    ForEach(task.files.prefix(4), id: \.self) { f in
                        Text(Format.splitPath(f).name)
                            .font(SF.mono(11.5))
                            .foregroundStyle(SF.muted)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(SF.fill, in: RoundedRectangle(cornerRadius: 4))
                            .help(f)
                    }
                    if task.files.count > 4 {
                        Text("+\(task.files.count - 4) more").font(SF.font(11.5)).foregroundStyle(SF.muted).fixedSize()
                    }
                    Spacer(minLength: 0)
                    DiffStatView(add: task.additions, del: task.deletions, blocks: false, size: 11).fixedSize()
                }
            }

            if failed { failure }

            HStack(spacing: 8) {
                if adding {
                    HStack(spacing: 6) {
                        StatusDot(status: .run(.running))
                        Text("Adding to the room…").foregroundStyle(SF.runInk)
                    }
                } else if done {
                    HStack(spacing: 4) {
                        Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold))
                        Text("In the room")
                        if let commit = integration?.commit {
                            Text(commit.prefix(7)).font(SF.mono(11.5)).foregroundStyle(SF.muted)
                        }
                    }
                    .foregroundStyle(SF.okInk)
                }
                if let pr = task.pr, let url = Format.safeLink(pr.url) {
                    Button {
                        NSWorkspace.shared.open(url)
                    } label: {
                        HStack(spacing: 4) {
                            PRIcon(state: pr.state)
                            Text("#\(String(pr.number))").monospacedDigit()
                        }
                    }
                    .buttonStyle(LinkButtonStyle())
                    .help("Open the pull request on GitHub")
                }
                Spacer(minLength: 0)
                if mine, !task.isLive, !adding, !failed, !task.files.isEmpty {
                    if done {
                        Button("Add new changes") { Task { await store.integrate(task) } }
                            .buttonStyle(LinkButtonStyle())
                            .disabled(busy)
                            .help("Merge anything new on this task's branch into the room")
                    } else {
                        Button {
                            Task { await store.integrate(task) }
                        } label: {
                            Label(busy ? "Adding…" : "Add to room", systemImage: "arrow.triangle.merge").font(SF.font(12, .medium))
                        }
                        .buttonStyle(PrimaryButtonStyle(height: 26))
                        .disabled(busy)
                    }
                }
                Button(mine ? "Open" : "Watch") { stores.open(task.id) }
                    .buttonStyle(LinkButtonStyle())
            }
            .font(SF.font(12))
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    /// Why it isn't in the room, and what to do about it.
    private var failure: some View {
        let conflicts = integration?.conflicts ?? []
        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 6) {
                Image(systemName: "exclamationmark.triangle").font(.system(size: 11)).padding(.top, 1)
                Text(conflicts.isEmpty
                    ? "Couldn't add it to the room\(integration?.error.map { ": \($0)" } ?? ".")"
                    : "Conflicts with work already in the room:")
                    .fixedSize(horizontal: false, vertical: true)
            }
            .foregroundStyle(SF.errInk)
            if !conflicts.isEmpty {
                FlowRow(spacing: 4, lineSpacing: 4) {
                    ForEach(conflicts, id: \.self) { f in
                        Text(f)
                            .font(SF.mono(11))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(SF.background, in: RoundedRectangle(cornerRadius: 4))
                    }
                }
            }
            if mine {
                HStack(spacing: 6) {
                    if !conflicts.isEmpty {
                        Button("Ask my agent to fix") { Task { await store.askToFix(task) } }
                            .buttonStyle(SecondaryButtonStyle(height: 26))
                            .disabled(busy || task.isLive)
                    }
                    Button("Try again") { Task { await store.integrate(task) } }
                        .buttonStyle(QuietButtonStyle())
                        .padding(.horizontal, 6)
                        .disabled(busy || task.isLive)
                }
            }
        }
        .font(SF.font(12))
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SF.errSoft.opacity(0.5), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(SF.err.opacity(0.3)))
    }
}

// MARK: Notes

/// What the team has decided. Every task's agent gets the latest notes in its brief.
private struct RoomNotes: View {
    var store: RoomStore
    var room: Room
    @Environment(Stores.self) private var stores
    @State private var note = ""
    @State private var editing: String?
    @State private var draft = ""
    @State private var saving = false
    @State private var deleting: RoomNote?
    @FocusState private var draftFocused: Bool

    private func post() {
        let text = note
        Task {
            if await store.post(note: text) { note = "" }
        }
    }

    private func save(_ n: RoomNote) {
        guard !saving else { return }
        saving = true
        Task {
            if await store.save(note: n, body: draft) { editing = nil }
            saving = false
        }
    }

    private func author(_ n: RoomNote) -> String {
        if store.isMine(n) { return "You" }
        return n.authorLogin.map { "@\($0)" } ?? "Someone"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            (Text("Notes").font(SF.font(13, .semibold)) + Text(" · agents in the room read these").font(SF.font(13)).foregroundColor(SF.muted))
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        if store.notes.isEmpty {
                            Text("Decide things here: who takes what, what to avoid, what's done. Every task's agent gets the latest notes.")
                                .font(SF.font(13))
                                .lineSpacing(3)
                                .foregroundStyle(SF.muted)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        ForEach(store.notes) { n in row(n).id(n.id) }
                    }
                    .padding(16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                // Keep the conversation at the latest note as new ones arrive.
                .onChange(of: store.notes.count) { _, _ in
                    if let last = store.notes.last { withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(last.id, anchor: .bottom) } }
                }
                .onAppear {
                    if let last = store.notes.last { proxy.scrollTo(last.id, anchor: .bottom) }
                }
            }
            Divider()
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Write a note to the room…", text: $note, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(SF.font(13.5))
                    .lineLimit(1...7)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(SF.fill.opacity(0.7), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .onChange(of: note) { _, v in if v.count > 4000 { note = String(v.prefix(4000)) } }
                    .onSubmit(post)
                    .accessibilityLabel("Note")
                Button(action: post) {
                    Image(systemName: "arrow.up")
                }
                .buttonStyle(RoundIconButtonStyle(size: 30))
                .disabled(note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || store.posting)
                .accessibilityLabel("Post note")
            }
            .padding(10)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .card()
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Room notes")
        .alert("Delete this note?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })) {
            Button("Delete", role: .destructive) {
                if let n = deleting { Task { await store.delete(note: n) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Agents starting tasks in the room stop seeing it.")
        }
    }

    private func row(_ n: RoomNote) -> some View {
        HoverReveal { hovering in
            HStack(alignment: .top, spacing: 10) {
                InitialAvatar(login: n.authorLogin, size: 24).padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(author(n)).font(SF.font(12, .medium))
                        Text(Format.rel(n.createdAt, now: stores.fleet.now) + (n.editedAt != nil ? " · edited" : ""))
                            .font(SF.font(12))
                            .monospacedDigit()
                            .foregroundStyle(SF.faint)
                            .help(n.editedAt != nil ? "Edited \(Format.rel(n.editedAt, now: stores.fleet.now))" : "")
                        Spacer(minLength: 0)
                        if editing != n.id {
                            HStack(spacing: 2) {
                                // Only its author rewrites a note.
                                if store.isMine(n) {
                                    Button {
                                        draft = n.body
                                        editing = n.id
                                        draftFocused = true
                                    } label: {
                                        Image(systemName: "pencil").font(.system(size: 11)).frame(width: 20, height: 18)
                                    }
                                    .buttonStyle(QuietButtonStyle(cornerRadius: 4))
                                    .help("Edit note")
                                    .accessibilityLabel("Edit note")
                                }
                                if store.canDelete(n) {
                                    Button {
                                        deleting = n
                                    } label: {
                                        Image(systemName: "trash").font(.system(size: 11)).frame(width: 20, height: 18)
                                    }
                                    .buttonStyle(QuietButtonStyle(cornerRadius: 4))
                                    .help("Delete note")
                                    .accessibilityLabel("Delete note")
                                }
                            }
                            .opacity(hovering ? 1 : 0)
                        }
                    }
                    if editing == n.id {
                        VStack(alignment: .trailing, spacing: 6) {
                            TextField("Note", text: $draft, axis: .vertical)
                                .textFieldStyle(.plain)
                                .font(SF.font(13.5))
                                .lineSpacing(3)
                                .lineLimit(3...12)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .background(SF.fill.opacity(0.7), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                                .focused($draftFocused)
                                .onChange(of: draft) { _, v in if v.count > 4000 { draft = String(v.prefix(4000)) } }
                                .onSubmit { save(n) }
                                .onExitCommand { editing = nil }
                                .accessibilityLabel("Edit note")
                            HStack(spacing: 6) {
                                Button("Cancel") { editing = nil }.buttonStyle(SecondaryButtonStyle(height: 24))
                                Button("Save") { save(n) }
                                    .buttonStyle(PrimaryButtonStyle(height: 24))
                                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || saving)
                            }
                        }
                        .padding(.top, 2)
                    } else {
                        Text(n.body)
                            .font(SF.font(13.5))
                            .lineSpacing(3)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        }
    }
}

// MARK: Small parts

/// Shows its content with whether the pointer is over it, for controls that appear on hover.
struct HoverReveal<Content: View>: View {
    @ViewBuilder var content: (Bool) -> Content
    @State private var hovering = false

    var body: some View {
        content(hovering)
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
            .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

/// A value in a quiet chip with a copy button: a branch, a clone command.
struct CopyChip: View {
    var text: String
    var copy: String
    var symbol: String?
    var help = "Copy"

    var body: some View {
        HStack(spacing: 4) {
            if let symbol { Image(systemName: symbol).font(.system(size: 10)).foregroundStyle(SF.muted) }
            Text(text).font(SF.mono(11.5)).foregroundStyle(SF.ink.opacity(0.8)).lineLimit(1).truncationMode(.middle)
            CopyButton(text: { copy }, size: 10)
        }
        .padding(.leading, 6)
        .padding(.trailing, 1)
        .background(SF.fill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .help(help)
    }
}
