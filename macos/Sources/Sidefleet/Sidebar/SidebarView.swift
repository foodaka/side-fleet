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
    private var byProject: Bool { fleet.projects.view == .project }
    /// The icon picker's sheet, open while `projects.picking` names a repo.
    private var pickingProject: Binding<PickingProject?> {
        Binding(get: { fleet.projects.picking.map(PickingProject.init) }, set: { fleet.projects.picking = $0?.id })
    }

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
        .modifier(SlashFocusesSearch { searchFocused = true })
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
                Kbd(text: "/")
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(searchFocused ? SF.selected : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }

    // MARK: The list

    private static let listTop = "sidebar-list-top"

    private var list: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    Color.clear.frame(height: 0).id(Self.listTop)
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
                // A fresh list per view: the lazy stack otherwise reuses rows laid out for the other one
                // (rows by time kept their place instead of moving in under their project).
                .id(fleet.projects.view)
            }
            .scrollIndicators(.automatic)
            .sheet(item: pickingProject) { ProjectIconSheet(repo: $0.id, current: fleet.projects.icons[$0.id]) }
            .onChange(of: fleet.projects.view) { _, _ in
                // Switching between By time and By project starts again from the top.
                proxy.scrollTo(Self.listTop, anchor: .top)
            }
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
            ForEach(matches) { RunRow(run: $0, byProject: byProject, onPick: { picking = $0 }) }
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
        listTabs(theirs: s.theirs)
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
            if !s.pinned.isEmpty {
                SectionHeading("Pinned")
                ForEach(s.pinned) { RunRow(run: $0, detail: $0.isLive, homeRepo: s.homeRepo, byProject: byProject, onPick: { picking = $0 }) }
            }
            if byProject {
                projectGroups(s)
            } else {
                if !s.active.isEmpty {
                    SectionHeading("Active", count: s.active.count)
                    ForEach(s.active) { RunRow(run: $0, detail: true, homeRepo: s.homeRepo, onPick: { picking = $0 }) }
                }
                ForEach(s.groups) { group in
                    SectionHeading(group.label)
                    ForEach(group.runs) { RunRow(run: $0, homeRepo: s.homeRepo, onPick: { picking = $0 }) }
                }
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
                ForEach(s.snoozed) { RunRow(run: $0, asleep: true, homeRepo: s.homeRepo, byProject: byProject, onPick: { picking = $0 }) }
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
                    ForEach(archived) { RunRow(run: $0, byProject: byProject, onPick: { picking = $0 }) }
                }
            } else {
                Text("Loading…").font(SF.font(12)).foregroundStyle(SF.muted).padding(.horizontal, 10).padding(.vertical, 4)
            }
        }
    }

    /// Mine / Team when the org has several members, and By time / By project. Both choices stick.
    private func listTabs(theirs: Int) -> some View {
        VStack(spacing: 6) {
            if filtering {
                SegmentTabs(segments: [.init(id: "mine", label: "Mine"), .init(id: "team", label: "Team", badge: theirs)], selected: scope) { scope = $0 }
            }
            SegmentTabs(segments: [.init(id: "time", label: "By time"), .init(id: "project", label: "By project")], selected: fleet.projects.view.rawValue) { id in
                fleet.projects.setView(id == "project" ? .project : .time)
            }
        }
        .padding(.horizontal, 4)
        .padding(.top, 12)
    }

    /// By project: one group per repo, the ones waiting on you or busy first; hidden ones come back from the foot.
    @ViewBuilder private func projectGroups(_ s: SidebarSections) -> some View {
        let projects = fleet.projects
        let shown = s.projects.filter { !projects.hidden.contains($0.repo) }
        let hiddenCount = s.projects.count - shown.count
        if !s.pinned.isEmpty, !shown.isEmpty {
            SectionHeading("Projects")
        } else {
            Color.clear.frame(height: 6)
        }
        ForEach(shown) { group in
            ProjectGroupHeader(group: group)
            if !projects.folded.contains(group.repo) {
                let all = projects.showAll.contains(group.repo)
                let rows = group.visible(showAll: all, openID: stores.openThreadID)
                ForEach(rows) { RunRow(run: $0, detail: $0.isLive, grouped: true, byProject: true, onPick: { picking = $0 }).inProjectGroup() }
                if group.runs.count > ProjectGroup.shown {
                    Button {
                        projects.toggleShowAll(group.repo)
                    } label: {
                        Text(all ? "Show fewer" : "Show \(group.runs.count - rows.count) more")
                            .font(SF.font(11.5))
                            .foregroundStyle(SF.faint)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .inProjectGroup()
                }
            }
        }
        if hiddenCount > 0 {
            Button {
                projects.unhideAll()
            } label: {
                Text("Show \(hiddenCount) hidden project\(hiddenCount == 1 ? "" : "s")")
                    .font(SF.font(11.5))
                    .foregroundStyle(SF.faint)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.top, 8)
        }
    }

    // MARK: Nav

    private var nav: some View {
        VStack(spacing: 2) {
            Divider().padding(.bottom, 8).padding(.horizontal, -12)
            NavRow(title: "Overview", symbol: "square.grid.2x2", active: stores.route == .overview) { stores.route = .overview }
            NavRow(title: "Pull requests", symbol: "arrow.triangle.pull", active: stores.route == .pullRequests) { stores.route = .pullRequests }
            NavRow(title: "Machines", symbol: "server.rack", active: stores.route == .machines) { stores.route = .machines } trailing: { machineStatus }
            NavRow(title: "Rooms", symbol: "shippingbox", active: inRooms) { stores.route = .rooms }
            if fleet.me?.user != nil {
                NavRow(title: "Team", symbol: "person.2", active: stores.route == .team) { stores.route = .team }
            }
            NavRow(title: "Chat apps", symbol: "bubble.left.and.bubble.right", active: stores.route == .chatApps) { stores.route = .chatApps }
            SettingsMenuRow()
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
    }

    private var inRooms: Bool {
        switch stores.route {
        case .rooms, .room: true
        default: false
        }
    }

    @ViewBuilder private var machineStatus: some View {
        if fleet.conn == .unreachable {
            HStack(spacing: 6) {
                StatusDot(status: .run(.failed))
                Text("Unreachable").foregroundStyle(SF.errInk)
            }
            .font(SF.font(11.5))
            .help(connLabel)
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
            .help(connLabel)
        }
    }

    /// The numbers beside Machines, in words.
    private var connLabel: String {
        if fleet.conn == .unreachable { return "Control plane unreachable" }
        if !fleet.loaded { return "Connecting…" }
        return "\(fleet.online.count) of \(fleet.workers.count) machine\(fleet.workers.count == 1 ? "" : "s") online"
            + (fleet.updatable.isEmpty ? "" : ", \(fleet.updatable.count) can update")
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
    /// In a project's group, which already says the repo.
    var grouped = false
    /// The list is By project: rows outside a group name their project, not the full repo.
    var byProject = false
    /// "Pick a date and time…" for this one.
    var onPick: (RunSummary) -> Void
    @Environment(Stores.self) private var stores
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    /// The pointer is over the row: its ⋯ button takes the place of the time and marks on the right.
    @State private var hovering = false

    private var fleet: FleetStore { stores.fleet }
    private var isOpen: Bool { stores.openThreadID == run.id }
    private var waiting: InboxItem? { isOpen ? nil : stores.inbox.item(for: run.id) }
    private var unread: Bool { !isOpen && (waiting != nil || stores.seen.isUnread(run)) }
    private var asking: Bool { run.pendingApprovals > 0 && run.isLive }
    private var otherRepo: Bool { !grouped && (byProject || run.run.repo != homeRepo) }
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
        // The same menu, for those who don't think to right-click.
        .overlay(alignment: .topTrailing) {
            if hovering {
                Menu { menu } label: {
                    Image(systemName: "ellipsis").font(.system(size: 11, weight: .medium)).frame(width: 22, height: 22)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .foregroundStyle(SF.muted)
                .padding(.top, 5)
                .padding(.trailing, 6)
                .accessibilityLabel("Actions for \(String(run.run.task.prefix(60)))")
            }
        }
        .onHover { hovering = $0 }
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
            trailingMarks
                // They keep their room while hidden, so the title doesn't shift under the pointer.
                .opacity(hovering ? 0 : 1)
        }
    }

    @ViewBuilder private var trailingMarks: some View {
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
            // The effort shows whatever the model, the machine's default included.
            let effort = run.latestEffort.flatMap { $0.isEmpty ? nil : " · \($0)" } ?? ""
            if !asking {
                Text((run.latestModel.isEmpty ? "default" : Format.modelName(run.latestModel, agent: run.latestAgent)) + effort)
                    .font(SF.font(11))
                    .foregroundStyle(SF.faint)
                    .lineLimit(1)
                    .help(Format.agentModel(run.latestAgent, run.latestModel) + effort)
            }
            AgentIcon(agent: run.latestAgent, size: 11)
                .foregroundStyle(SF.faint)
                .help(Format.agentModel(run.latestAgent, run.latestModel) + effort)
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
                    // Running, but the machine's first word hasn't arrived: there's no start to count from yet.
                    let took = Format.duration(start: run.run.startedAt, now: now)
                    Text("\(run.latestWorkerID ?? "Starting") · \(took.isEmpty ? "starting" : took)").monospacedDigit()
                }
            } else if let start = run.run.startAt, run.run.isScheduled(now: fleet.now) {
                withRepo("Scheduled · \(Format.startTime(start, now: fleet.now))")
            } else {
                withRepo("Waiting for a machine")
            }
        } else if asleep, let until = run.run.snoozedUntil {
            withRepo("Back \(Format.wakeLabel(until, now: fleet.now))")
        } else {
            withRepo(nil, then: fleet.workers.count > 1 ? run.latestWorkerID : nil)
        }
    }

    /// `text · [icon] repo · then`, the repo only when it says something.
    private func withRepo(_ text: String?, then: String? = nil) -> some View {
        HStack(spacing: 4) {
            if let text { Text(text) }
            if otherRepo {
                if text != nil { Text("·") }
                ProjectIconView(repo: run.run.repo, icon: fleet.projects.icon(run.run.repo), size: 11)
                Text(byProject ? ProjectIcon.name(run.run.repo) : Format.repoLabel(run.run.repo))
            }
            if let then { Text((otherRepo ? "· " : "") + then) }
        }
    }

    @ViewBuilder private var menu: some View {
        Button("Open in New Window") { openWindow(value: ThreadWindowID(id: run.id)) }
        Divider()
        let theirs = fleet.othersRun(run.run.createdBy)
        Button {
            stores.forkThread = run.id
            stores.open(run.id)
        } label: {
            // A second line of text is a menu item's subtitle.
            Text(theirs ? "Continue on My Machine…" : "Fork…")
            Text("New thread from its latest message, any agent, model or machine\(theirs ? " of yours" : "")")
        }
        if run.run.pinnedAt != nil {
            Button("Unpin") { Task { await fleet.setPinned(run, pinned: false) } }
        } else {
            Button("Pin to Top") { Task { await fleet.setPinned(run, pinned: true) } }
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
                Button("Edit These Choices…") { openSettings() }
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

// The org switcher and the Settings menu are in SidebarMenus.swift.

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
