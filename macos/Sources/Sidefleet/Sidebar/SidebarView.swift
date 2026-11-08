import AppKit
import SidefleetKit
import SwiftUI

struct SidebarView: View {
    @Environment(Stores.self) private var stores
    @State private var query = ""
    @State private var showSnoozed = false
    @State private var showArchived = false
    @State private var picking: RunSummary?
    @AppStorage("sidefleet_runs_scope") private var scope = "mine"
    @FocusState private var searchFocused: Bool

    private var fleet: FleetStore { stores.fleet }
    private var filtering: Bool { fleet.multi && fleet.userID != nil }

    var body: some View {
        VStack(spacing: 0) {
            header
            VStack(spacing: 8) {
                OrgSwitcher()
                newTaskButton
                searchField
            }
            .padding(.horizontal, 12)
            list
            nav
        }
        .frame(minWidth: 220)
        .sheet(item: $picking) { SnoozePickSheet(run: $0) }
        .focusedSceneValue(\.focusSearch) { searchFocused = true }
    }

    private var header: some View {
        HStack(spacing: 4) {
            LogoMark(size: 20)
            Text("Sidefleet").font(SF.font(15, .semibold)).tracking(-0.2)
            Spacer()
            InboxBell()
        }
        .padding(.leading, 16)
        .padding(.trailing, 10)
        .padding(.top, 4)
        .padding(.bottom, 12)
    }

    private var newTaskButton: some View {
        Button {
            stores.route = .newTask
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "plus").font(.system(size: 11, weight: .medium))
                Text("New task").font(SF.font(13, .medium))
                Spacer()
            }
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background(SF.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(stores.route == .newTask ? SF.ink.opacity(0.15) : SF.border))
            .shadow(color: .black.opacity(0.03), radius: 1, y: 1)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(SF.muted)
            TextField("Search tasks", text: $query)
                .textFieldStyle(.plain)
                .font(SF.font(13))
                .focused($searchFocused)
                .onExitCommand {
                    query = ""
                    searchFocused = false
                }
            if !query.isEmpty {
                Button {
                    query = ""
                    searchFocused = true
                } label: {
                    Image(systemName: "xmark.circle.fill").font(.system(size: 11)).foregroundStyle(SF.faint)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            } else if !searchFocused {
                Kbd(text: "⌘F")
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(searchFocused ? SF.selected : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }

    // MARK: The list

    private var list: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !fleet.loaded {
                        skeleton
                    } else if !query.trimmingCharacters(in: .whitespaces).isEmpty {
                        searchResults
                    } else {
                        sections
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 12)
            }
            .scrollIndicators(.automatic)
            .onChange(of: stores.openThreadID) { _, id in
                // Keep the open task in view, after opening it from somewhere else.
                if let id { withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(id) } }
            }
        }
    }

    private var skeleton: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach([0.72, 0.88, 0.6, 0.8, 0.66, 0.84], id: \.self) { w in
                HStack(spacing: 10) {
                    Circle().fill(SF.fill).frame(width: 6, height: 6)
                    SkeletonBar(width: 200 * w)
                }
            }
        }
        .padding(.top, 20)
        .padding(.horizontal, 10)
    }

    private var searchResults: some View {
        let matches = SidebarSections.matches(query: query, runs: fleet.runs, archived: fleet.archived)
        return Group {
            SectionHeading(matches.isEmpty ? "No matches" : "\(matches.count) match\(matches.count == 1 ? "" : "es")")
            ForEach(matches) { RunRow(run: $0, onPick: { picking = $0 }) }
            if matches.isEmpty {
                Text("Nothing in the latest \(fleet.runs.count) tasks mentions “\(query.trimmingCharacters(in: .whitespaces))”.")
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
            }
        }
    }

    @ViewBuilder private var sections: some View {
        let s = SidebarSections(runs: fleet.runs, userID: fleet.userID, teamScope: scope == "team", filtering: filtering, now: fleet.now)
        if filtering { scopeTabs(theirs: s.theirs) }
        if s.isEmpty {
            Text(filtering && scope == "mine" && !fleet.runs.isEmpty
                ? "You haven't started a task here yet. Switch to Team to see what everyone else is running."
                : "No tasks yet. Each one you start shows up here, so you can jump between them.")
                .font(SF.font(12))
                .lineSpacing(3)
                .foregroundStyle(SF.muted)
                .padding(.horizontal, 12)
                .padding(.vertical, 14)
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
                .padding(.horizontal, 4)
                .padding(.top, 16)
        } else {
            if !s.active.isEmpty {
                SectionHeading("Active", count: s.active.count)
                ForEach(s.active) { RunRow(run: $0, detail: true, homeRepo: s.homeRepo, onPick: { picking = $0 }) }
            }
            ForEach(s.groups) { group in
                SectionHeading(group.label)
                ForEach(group.runs) { RunRow(run: $0, homeRepo: s.homeRepo, onPick: { picking = $0 }) }
            }
            if fleet.runs.count >= 100 {
                Text("Showing the latest \(fleet.runs.count) tasks\(filtering ? " in the team" : "").")
                    .font(SF.font(11))
                    .foregroundStyle(SF.faint)
                    .padding(.horizontal, 10)
                    .padding(.top, 16)
            }
        }
        if !s.snoozed.isEmpty {
            Disclosure(title: "Snoozed", count: s.snoozed.count, open: $showSnoozed)
            if showSnoozed {
                ForEach(s.snoozed) { RunRow(run: $0, asleep: true, homeRepo: s.homeRepo, onPick: { picking = $0 }) }
                Text("Each comes back at its time, or sooner if it finishes, fails or asks you something.")
                    .font(SF.font(11))
                    .lineSpacing(2)
                    .foregroundStyle(SF.faint)
                    .padding(.horizontal, 10)
                    .padding(.top, 4)
            }
        }
        Disclosure(title: "Archived", count: fleet.archived?.count, open: $showArchived)
            .onChange(of: showArchived) { _, open in
                if open { Task { await fleet.loadArchived() } }
            }
        if showArchived {
            if let archived = fleet.archived {
                if archived.isEmpty {
                    Text("Nothing archived. Archive a task from its menu to tidy this list.")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                } else {
                    ForEach(archived) { RunRow(run: $0, onPick: { picking = $0 }) }
                }
            } else {
                Text("Loading…").font(SF.font(12)).foregroundStyle(SF.muted).padding(.horizontal, 10).padding(.vertical, 4)
            }
        }
    }

    private func scopeTabs(theirs: Int) -> some View {
        HStack(spacing: 2) {
            ForEach([("mine", "Mine"), ("team", "Team")], id: \.0) { id, label in
                Button {
                    scope = id
                } label: {
                    HStack(spacing: 4) {
                        Text(label)
                        if id == "team", theirs > 0 { Text("\(theirs)").monospacedDigit().foregroundStyle(SF.faint) }
                    }
                    .font(SF.font(12, scope == id ? .medium : .regular))
                    .foregroundStyle(scope == id ? SF.ink : SF.muted)
                    .frame(maxWidth: .infinity)
                    .frame(height: 24)
                    .background(scope == id ? SF.background : .clear, in: RoundedRectangle(cornerRadius: 5))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(2)
        .background(SF.selected.opacity(0.6), in: RoundedRectangle(cornerRadius: 7))
        .padding(.horizontal, 4)
        .padding(.top, 12)
    }

    // MARK: Nav

    private var nav: some View {
        VStack(spacing: 2) {
            Divider().padding(.bottom, 8).padding(.horizontal, -12)
            NavRow(title: "Overview", symbol: "square.grid.2x2", active: stores.route == .overview) { stores.route = .overview }
            NavRow(title: "Machines", symbol: "server.rack", active: stores.route == .machines) { stores.route = .machines } trailing: { machineStatus }
            if fleet.me?.user != nil {
                NavRow(title: "Team", symbol: "person.2", external: true) { stores.openOnWeb("/team") }
            }
            NavRow(title: "Rooms", symbol: "shippingbox", external: true) { stores.openOnWeb("/rooms") }
            NavRow(title: "Chat apps", symbol: "bubble.left.and.bubble.right", external: true) { stores.openOnWeb("/chat") }
            SettingsLink {
                HStack(spacing: 10) {
                    Image(systemName: "gearshape").font(.system(size: 12)).frame(width: 16)
                    Text("Settings").font(SF.font(13))
                    Spacer()
                }
                .foregroundStyle(SF.muted)
                .padding(.horizontal, 10)
                .frame(height: 32)
                .hoverBackground()
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
    }

    @ViewBuilder private var machineStatus: some View {
        if fleet.conn == .unreachable {
            HStack(spacing: 6) {
                StatusDot(status: .run(.failed))
                Text("Unreachable").foregroundStyle(SF.errInk)
            }
            .font(SF.font(11.5))
        } else if fleet.loaded {
            HStack(spacing: 6) {
                if !fleet.updatable.isEmpty {
                    Image(systemName: "arrow.up.circle").font(.system(size: 12)).foregroundStyle(SF.queueInk)
                        .help("\(fleet.updatable.count) can update")
                }
                StatusDot(status: fleet.online.isEmpty ? .offline : .online)
                Text("\(fleet.online.count)/\(fleet.workers.count)").monospacedDigit()
            }
            .font(SF.font(11.5))
            .foregroundStyle(SF.muted)
        }
    }
}

private struct NavRow<Trailing: View>: View {
    var title: String
    var symbol: String
    var active = false
    var external = false
    var action: () -> Void
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: symbol).font(.system(size: 12)).frame(width: 16)
                Text(title).font(SF.font(13, active ? .medium : .regular))
                Spacer()
                if external {
                    Image(systemName: "arrow.up.forward").font(.system(size: 9)).foregroundStyle(SF.faint)
                        .help("Opens in your browser")
                }
                trailing()
            }
            .foregroundStyle(active ? SF.ink : SF.muted)
            .padding(.horizontal, 10)
            .frame(height: 32)
            .hoverBackground(active: active)
        }
        .buttonStyle(.plain)
    }
}

extension NavRow where Trailing == EmptyView {
    init(title: String, symbol: String, active: Bool = false, external: Bool = false, action: @escaping () -> Void) {
        self.init(title: title, symbol: symbol, active: active, external: external, action: action) { EmptyView() }
    }
}

private struct SectionHeading: View {
    var label: String
    var count: Int?

    init(_ label: String, count: Int? = nil) {
        self.label = label
        self.count = count
    }

    var body: some View {
        HStack(spacing: 6) {
            Text(label).font(SF.font(11, .medium)).foregroundStyle(SF.faint)
            if let count {
                Text("\(count)")
                    .font(SF.font(10.5))
                    .monospacedDigit()
                    .foregroundStyle(SF.runInk)
                    .padding(.horizontal, 6)
                    .frame(height: 15)
                    .background(SF.runSoft, in: Capsule())
            }
        }
        .padding(.horizontal, 10)
        .padding(.top, 16)
        .padding(.bottom, 4)
    }
}

private struct Disclosure: View {
    var title: String
    var count: Int?
    @Binding var open: Bool

    var body: some View {
        Button {
            withAnimation(.easeOut(duration: 0.15)) { open.toggle() }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 8.5, weight: .semibold))
                    .rotationEffect(.degrees(open ? 90 : 0))
                Text(title).font(SF.font(11, .medium))
                if let count, count > 0 { Text("\(count)").font(SF.font(11)).monospacedDigit() }
                Spacer()
            }
            .foregroundStyle(SF.faint)
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.top, 12)
        .accessibilityAddTraits(open ? .isSelected : [])
    }
}

// MARK: A row

struct RunRow: View {
    var run: RunSummary
    /// In Active: says where it runs and for how long.
    var detail = false
    var asleep = false
    var homeRepo = ""
    /// "Pick a date and time…" for this one.
    var onPick: (RunSummary) -> Void
    @Environment(Stores.self) private var stores
    @Environment(\.openWindow) private var openWindow

    private var fleet: FleetStore { stores.fleet }
    private var isOpen: Bool { stores.openThreadID == run.id }
    private var waiting: InboxItem? { isOpen ? nil : stores.inbox.item(for: run.id) }
    private var unread: Bool { !isOpen && (waiting != nil || stores.seen.isUnread(run)) }
    private var asking: Bool { run.pendingApprovals > 0 && run.isLive }
    private var otherRepo: Bool { run.run.repo != homeRepo }
    private var filtering: Bool { fleet.multi && fleet.userID != nil }

    var body: some View {
        Button {
            stores.open(run.id)
        } label: {
            HStack(alignment: .top, spacing: 10) {
                Group {
                    if asking {
                        StatusDot(status: .waiting)
                    } else {
                        StatusDot(status: .run(run.threadStatus), dimmed: run.threadStatus == .succeeded && !unread)
                    }
                }
                .frame(width: 8, height: 18)
                VStack(alignment: .leading, spacing: 1) {
                    titleLine
                    detailLine
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .hoverBackground(active: isOpen)
        }
        .buttonStyle(.plain)
        .id(run.id)
        .help(run.run.task)
        .contextMenu { menu }
    }

    private var titleLine: some View {
        HStack(spacing: 6) {
            if filtering, fleet.othersRun(run.run.createdBy) {
                InitialAvatar(login: fleet.login(run.run.createdBy), size: 14)
            }
            Text(run.run.task.replacingOccurrences(of: "\n", with: " "))
                .font(SF.font(13, unread ? .medium : .regular))
                .foregroundStyle(unread || isOpen ? SF.ink : SF.ink.opacity(0.8))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let pr = run.run.pr {
                HStack(spacing: 2) {
                    PRIcon(state: pr.state)
                    Text("\(pr.number)").monospacedDigit()
                }
                .font(SF.font(11))
                .foregroundStyle(SF.muted)
                .help("Pull request #\(pr.number): \(pr.state)")
            }
            if let waiting {
                Circle()
                    .fill(waiting.reason.toneColor)
                    .frame(width: 8, height: 8)
                    .help(waiting.label)
                    .accessibilityLabel("Waiting on you: \(waiting.label)")
            } else if unread {
                Circle().fill(SF.ink).frame(width: 6, height: 6).accessibilityLabel("New")
            } else if !detail, !asleep {
                Text(Format.ago(run.lastActivityAt, now: fleet.now))
                    .font(SF.font(11))
                    .monospacedDigit()
                    .foregroundStyle(SF.faint)
            }
        }
    }

    private var detailLine: some View {
        HStack(spacing: 6) {
            if run.run.isolation == "checkout" {
                Image(systemName: "folder").font(.system(size: 10)).foregroundStyle(SF.faint)
                    .help("In the repo's own checkout, not a worktree")
            }
            detailText
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            if !asking {
                Text(run.latestModel.isEmpty ? "default" : Format.modelName(run.latestModel, agent: run.latestAgent) + (run.latestEffort.map { " · \($0)" } ?? ""))
                    .font(SF.font(11))
                    .foregroundStyle(SF.faint)
                    .lineLimit(1)
            }
            AgentIcon(agent: run.latestAgent, size: 11)
                .foregroundStyle(SF.faint)
                .help(Format.agentModel(run.latestAgent, run.latestModel))
        }
        .font(SF.font(11.5))
        .foregroundStyle(SF.muted)
    }

    @ViewBuilder private var detailText: some View {
        if asking {
            Text("Waiting on you" + (run.pendingApprovals > 1 ? " · \(run.pendingApprovals) tool calls" : ""))
                .font(SF.font(11.5, .medium))
                .foregroundStyle(SF.queueInk)
        } else if detail {
            if run.threadStatus == .running {
                LiveText { now in
                    Text("\(run.latestWorkerID ?? "Starting") · \(Format.duration(start: run.run.startedAt, now: now))").monospacedDigit()
                }
            } else if let start = run.run.startAt, run.run.isScheduled(now: fleet.now) {
                Text("Scheduled · \(Format.startTime(start, now: fleet.now))" + (otherRepo ? " · \(Format.repoLabel(run.run.repo))" : ""))
            } else {
                Text("Waiting for a machine" + (otherRepo ? " · \(Format.repoLabel(run.run.repo))" : ""))
            }
        } else if asleep, let until = run.run.snoozedUntil {
            Text("Back \(Format.wakeLabel(until, now: fleet.now))" + (otherRepo ? " · \(Format.repoLabel(run.run.repo))" : ""))
        } else {
            Text([otherRepo ? Format.repoLabel(run.run.repo) : nil, fleet.workers.count > 1 ? run.latestWorkerID : nil].compactMap { $0 }.joined(separator: " · "))
        }
    }

    @ViewBuilder private var menu: some View {
        Button("Open in New Window") { openWindow(value: ThreadWindowID(id: run.id)) }
        Divider()
        Button(fleet.othersRun(run.run.createdBy) ? "Continue on My Machine…" : "Fork…") {
            stores.forkThread = run.id
            stores.open(run.id)
        }
        if run.run.archivedAt != nil {
            Button("Unarchive") { Task { await fleet.setArchived(run, archived: false) } }
        } else {
            if run.isSnoozed(now: Date()) {
                Button("Unsnooze") { Task { await fleet.setSnoozed(run, until: nil) } }
            }
            Menu(run.isSnoozed(now: Date()) ? "Snooze Until" : "Snooze") {
                let now = Date()
                ForEach(stores.prefs.snoozePresets) { preset in
                    let at = preset.time(now: now)
                    Button("\(preset.label)  ·  \(Format.wakeLabel(at, now: now))") {
                        Task { await fleet.setSnoozed(run, until: preset.time(now: Date())) }
                    }
                }
                Divider()
                Button("Pick a Date and Time…") { onPick(run) }
            }
            Button("Archive") { Task { await fleet.setArchived(run, archived: true) } }
        }
        Divider()
        Button("Copy Title") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(run.run.task, forType: .string)
        }
    }
}

extension String {
    /// Colour of an inbox reason's marker.
    var toneColor: Color {
        switch InboxStore.tone(self) {
        case .ok: SF.ok
        case .queue: SF.queue
        case .run: SF.run
        case .err: SF.err
        }
    }
}

private struct SnoozePickSheet: View {
    var run: RunSummary
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var at = SnoozePreset(label: "", when: .at(time: "09:00", day: "tomorrow")).time(now: Date())

    private var ok: Bool { at > Date() }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Snooze until…").font(SF.font(15, .semibold))
            Text(run.run.task).font(SF.font(13)).foregroundStyle(SF.muted).lineLimit(1)
            DatePicker("Snooze until", selection: $at, in: Date()..., displayedComponents: [.date, .hourAndMinute])
                .datePickerStyle(.stepperField)
                .labelsHidden()
            Text(ok ? "It comes back \(Format.wakeLabel(at, now: Date())), or sooner if it finishes, fails or asks you something." : "Pick a time ahead.")
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(SecondaryButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button("Snooze") {
                    let until = at
                    Task { await stores.fleet.setSnoozed(run, until: until) }
                    dismiss()
                }
                .buttonStyle(PrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(!ok)
            }
        }
        .padding(20)
        .frame(width: 360)
    }
}

// MARK: Org switcher

struct OrgSwitcher: View {
    @Environment(Stores.self) private var stores

    var body: some View {
        if let me = stores.fleet.me, me.orgs.count > 1 {
            PickerPill(
                options: me.orgs.map { PickerOption(id: $0.id, title: $0.name, hint: $0.plan == "pro" ? "Pro" : nil) },
                selected: stores.fleet.org?.id,
                heading: "Your orgs",
                accessibility: "Switch org",
                onPick: { stores.switchOrg($0) }
            ) {
                Text(stores.fleet.org?.name ?? "").font(SF.font(13, .medium)).lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

// MARK: Inbox

struct InboxBell: View {
    @Environment(Stores.self) private var stores
    @State private var open = false

    private var count: Int { stores.inbox.items.count + (stores.fleet.updateNotice ? 1 : 0) }

    var body: some View {
        Button {
            open.toggle()
        } label: {
            ZStack(alignment: .topTrailing) {
                Image(systemName: "bell")
                    .font(.system(size: 12))
                    .frame(width: 28, height: 28)
                if count > 0 {
                    Text(count > 99 ? "99+" : "\(count)")
                        .font(.system(size: 9.5, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(.white)
                        .padding(.horizontal, 3)
                        .frame(minWidth: 14, minHeight: 14)
                        .background(stores.inbox.items.isEmpty ? SF.queue : SF.err, in: Capsule())
                        .offset(x: 2, y: -1)
                        .transition(.scale.combined(with: .opacity))
                }
            }
            .foregroundStyle(open ? SF.ink : SF.muted)
            .hoverBackground(active: open)
            .animation(.easeOut(duration: 0.15), value: count)
        }
        .buttonStyle(.plain)
        .help("Waiting on you")
        .keyboardShortcut("i", modifiers: [.command, .shift])
        .accessibilityLabel(count > 0 ? "Waiting on you: \(count)" : "Waiting on you: nothing")
        .popover(isPresented: $open, arrowEdge: .bottom) {
            InboxList { open = false }
        }
    }
}

struct InboxList: View {
    var onOpen: () -> Void
    @Environment(Stores.self) private var stores

    private var fleet: FleetStore { stores.fleet }
    private var inbox: InboxStore { stores.inbox }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Waiting on you").font(SF.font(13, .medium))
                Spacer()
                if !inbox.items.isEmpty || fleet.updateNotice {
                    Button {
                        fleet.cleared()
                        Task { await inbox.clearAll() }
                    } label: {
                        Text("Clear all").font(SF.font(12)).padding(.horizontal, 6).padding(.vertical, 2)
                    }
                    .buttonStyle(QuietButtonStyle(cornerRadius: 4))
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            Divider()
            ScrollView {
                VStack(spacing: 1) {
                    if fleet.updateNotice {
                        Button {
                            onOpen()
                            fleet.cleared()
                            stores.route = .machines
                        } label: {
                            row(tone: SF.queue, title: fleet.updatable.count == 1 ? "\(fleet.updatable[0].displayName) can update" : "\(fleet.updatable.count) machines can update", detail: "Open Machines to update", repo: nil, age: nil)
                        }
                        .buttonStyle(.plain)
                    }
                    ForEach(inbox.items) { item in
                        Button {
                            onOpen()
                            Task { await inbox.dismiss(item.id) }
                            stores.open(item.id)
                        } label: {
                            let extra = InboxStore.detail(item.detail)
                            row(tone: item.reason.toneColor, title: item.title, detail: item.label + (extra.isEmpty ? "" : " · \(extra)"), repo: Format.repoLabel(item.repo), age: item.since)
                        }
                        .buttonStyle(.plain)
                    }
                    if inbox.items.isEmpty, !fleet.updateNotice {
                        Text(inbox.loaded ? "Nothing is waiting on you." : "Loading…")
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 24)
                    }
                }
                .padding(4)
            }
            .frame(maxHeight: 420)
            .fixedSize(horizontal: false, vertical: true)
            Divider()
            Toggle(isOn: Binding(get: { stores.prefs.notifications }, set: { on in
                stores.prefs.set(\.notifications, on)
                if on { stores.notifier.requestPermission() }
            })) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Notify me").font(SF.font(13))
                    Text("A notification on this Mac when a task starts waiting, while the app is running")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .toggleStyle(.switch)
            .controlSize(.small)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .frame(width: 320)
    }

    private func row(tone: Color, title: String, detail: String, repo: String?, age: Date?) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Circle().fill(tone).frame(width: 8, height: 8).padding(.top, 5)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(SF.font(13)).lineLimit(1)
                Text(detail).font(SF.font(11.5)).foregroundStyle(SF.muted).lineLimit(1)
                if let repo { Text(repo).font(SF.mono(10.5)).foregroundStyle(SF.faint).lineLimit(1) }
            }
            Spacer(minLength: 4)
            if let age {
                Text(Format.ago(age, now: fleet.now)).font(SF.font(11)).monospacedDigit().foregroundStyle(SF.faint)
                    .help("Waiting since \(age.formatted(date: .abbreviated, time: .shortened))")
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .hoverBackground()
    }
}
