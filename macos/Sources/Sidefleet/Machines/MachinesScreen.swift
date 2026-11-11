import SidefleetKit
import SwiftUI

struct MachinesScreen: View {
    @Environment(Stores.self) private var stores
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hover: String?

    private var fleet: FleetStore { stores.fleet }

    /// Yours first, then teammates' (which only run their owner's tasks).
    private var sorted: [Worker] {
        fleet.workers.sorted { a, b in
            if a.mine != b.mine { return a.mine }
            if a.online != b.online { return a.online }
            return a.displayName.localizedCaseInsensitiveCompare(b.displayName) == .orderedAscending
        }
    }

    private var summary: String {
        let busy = fleet.runs.filter { $0.threadStatus == .running }.count
        let slots = fleet.online.reduce(0) { $0 + ($1.capabilities.slots?.parallel ?? 0) }
        let outdated = fleet.workers.filter { $0.online && $0.outdated && !$0.updateRequested }.count
        return "\(fleet.online.count) of \(fleet.workers.count) online, \(busy) of \(slots) task slots busy." + (outdated > 0 ? " \(outdated) \(outdated == 1 ? "runs" : "run") an older build than this control plane." : "")
    }

    var body: some View {
        // The map scrolls to a machine's card when its node is clicked.
        ScrollViewReader { proxy in
            page(proxy)
        }
    }

    private func page(_ proxy: ScrollViewProxy) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                header
                PendingMachines()
                if sorted.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("No machines yet").font(SF.font(15, .medium))
                        Text("Connect one with a coding agent set up: Claude Code, Codex or opencode. It needs no open ports, only a way to reach this control plane.")
                            .font(SF.font(13))
                            .foregroundStyle(SF.muted)
                            .padding(.bottom, 16)
                        AddMachineSteps()
                    }
                    .padding(24)
                    .card()
                } else {
                    FleetMap(hover: $hover) { id in
                        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.3)) { proxy.scrollTo(id, anchor: .center) }
                    }
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 300), spacing: 16, alignment: .top)], alignment: .leading, spacing: 16) {
                        ForEach(sorted) { w in
                            MachineCard(worker: w, hot: hover == w.id, onHover: { hover = $0 }).id(w.id)
                        }
                    }
                }
                WorkerTokensSection()
            }
            .column(SF.wideColumn)
            .padding(.horizontal, 28)
            .padding(.top, 20)
            .padding(.bottom, 28)
        }
        .background(SF.background)
    }

    private var header: some View {
        HStack(alignment: .bottom, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Machines").font(SF.font(22, .semibold)).tracking(-0.3)
                Text(summary).font(SF.font(13)).foregroundStyle(SF.muted)
                if fleet.multi {
                    Text("Each machine runs only its owner's tasks, in every org its owner belongs to. Teammates' machines show here so you can see what the team is running.")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                }
                if let limits = fleet.org?.limits, let org = fleet.org {
                    Text("\(org.name) is on the \(org.plan) plan: \(fleet.workers.filter { !$0.guest }.count) of \(limits.machines) machines, up to \(limits.parallelRuns) tasks queued or running at once.")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Choose repos") { stores.repoPickerOpen = true }.buttonStyle(SecondaryButtonStyle())
            Button {
                stores.addMachineOpen = true
            } label: {
                Label("Add machine", systemImage: "plus")
            }
            .buttonStyle(PrimaryButtonStyle())
        }
    }
}

private struct MachineCard: View {
    var worker: Worker
    var hot: Bool
    var onHover: (String?) -> Void
    @Environment(Stores.self) private var stores
    @State private var requesting = false
    @State private var confirmingRemove = false

    private var fleet: FleetStore { stores.fleet }
    private var c: Capabilities { worker.capabilities }
    private var keys: Set<String> { Set(c.repos.map(\.key)) }
    private var tasks: [RunSummary] { fleet.runs.filter { $0.threadStatus == .running && $0.run.workerID == worker.id } }
    private var build: (version: String, date: String, sha: String) { Format.parseBuild(c.version) }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            head
            if let sys = c.system {
                cpu(sys)
                Grid(horizontalSpacing: 20, verticalSpacing: 14) {
                    GridRow {
                        Gauge(label: "Memory", value: String(format: "%.1f / %.0f GB", sys.memUsedMB / 1024, sys.memTotalMB / 1024), percent: Format.pct(sys.memUsedMB, sys.memTotalMB))
                        Gauge(label: "Disk", value: "\(Int(sys.diskFreeGB.rounded()).formatted()) GB free", percent: Format.pct(sys.diskTotalGB - sys.diskFreeGB, sys.diskTotalGB))
                    }
                }
                if let slots = c.slots { taskSlots(slots) }
            } else if worker.online {
                Text("No live numbers yet: this machine runs an older Sidefleet. Update it and CPU, memory and disk appear here.")
                    .font(SF.font(12))
                    .lineSpacing(3)
                    .foregroundStyle(SF.muted)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(SF.fill, in: RoundedRectangle(cornerRadius: 9))
            }
            chips
            CleanupRow(worker: worker)
            Spacer(minLength: 0)
            Divider()
            footer
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SF.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(hot ? SF.run.opacity(0.5) : SF.border))
        .shadow(color: hot ? SF.run.opacity(0.12) : .clear, radius: 5)
        .opacity(worker.online ? 1 : 0.7)
        .animation(.easeOut(duration: 0.2), value: hot)
        .onHover { onHover($0 ? worker.id : nil) }
        .alert("Remove \(worker.displayName)?", isPresented: $confirmingRemove) {
            Button("Remove", role: .destructive) { Task { await fleet.remove(worker) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("It reappears if it connects again; revoke its token to keep it out.")
        }
    }

    private var head: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(worker.displayName).font(SF.font(15, .semibold)).tracking(-0.15).lineLimit(1)
                    if fleet.multi, let owner = worker.owner {
                        if worker.mine {
                            Text("Yours").font(SF.font(11, .medium)).foregroundStyle(SF.okInk).padding(.horizontal, 6).padding(.vertical, 1).background(SF.okSoft, in: Capsule())
                        } else {
                            HStack(spacing: 4) {
                                InitialAvatar(login: owner.login, size: 14)
                                Text("@\(owner.login)")
                            }
                            .font(SF.font(11))
                            .foregroundStyle(SF.muted)
                            .padding(.trailing, 6)
                            .padding(.leading, 2)
                            .padding(.vertical, 1)
                            .background(SF.fill, in: Capsule())
                            .help("Runs only @\(owner.login)'s tasks")
                        }
                    }
                }
                Text([c.os, worker.displayName != c.hostname && !c.hostname.isEmpty ? c.hostname : nil, c.system.map { "up \(Format.uptime($0.uptimeSecs))" }].compactMap { $0 }.joined(separator: ", "))
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            HStack(spacing: 6) {
                StatusDot(status: worker.online ? .online : .offline)
                if worker.online {
                    Text("Online")
                } else {
                    Text("Last seen \(Format.rel(worker.lastHeartbeatAt, now: fleet.now))")
                }
            }
            .font(SF.font(12))
            .foregroundStyle(worker.online ? SF.okInk : SF.muted)
            .fixedSize()
        }
    }

    private func cpu(_ sys: SystemStats) -> some View {
        HStack(alignment: .bottom, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(Int(sys.cpuPercent.rounded()))%")
                    .font(SF.font(30, .medium))
                    .tracking(-0.5)
                    .monospacedDigit()
                    .contentTransition(.numericText())
                    .animation(.easeOut(duration: 0.3), value: Int(sys.cpuPercent.rounded()))
                Text("CPU, \(sys.cpuCores) cores").font(SF.font(12)).foregroundStyle(SF.muted)
            }
            Sparkline(values: fleet.cpu[worker.id] ?? [])
                .frame(height: 52)
                .accessibilityLabel("CPU while the app has been open")
        }
    }

    private func taskSlots(_ slots: Slots) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Tasks").foregroundStyle(SF.muted)
                Spacer()
                Text("\(tasks.count) of \(slots.parallel) running").monospacedDigit().foregroundStyle(SF.ink.opacity(0.8))
            }
            .font(SF.font(12))
            HStack(spacing: 6) {
                ForEach(0..<max(slots.parallel, 1), id: \.self) { i in
                    Group {
                        if i < tasks.count {
                            BusyStripes()
                        } else {
                            SF.fill
                        }
                    }
                    .frame(height: 8)
                    .clipShape(RoundedRectangle(cornerRadius: 3))
                }
            }
            ForEach(tasks) { t in
                Button { stores.open(t.id) } label: {
                    HStack(spacing: 8) {
                        Circle().fill(SF.run).frame(width: 6, height: 6)
                        Text(t.run.task).font(SF.font(12.5)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                        LiveText { now in
                            Text(Format.duration(start: t.run.startedAt, now: now)).font(SF.font(12.5)).monospacedDigit().foregroundStyle(SF.muted)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var chips: some View {
        FlowRow(spacing: 6, lineSpacing: 6) {
            if let a = c.agents?.claude {
                AgentChip(name: "Claude Code", detail: a.version, ok: a.signedIn == true, bad: a.signedIn != true,
                          help: a.signedIn == true ? "Signed in; tasks run with this account" : "Not signed in: run `claude` on this machine once")
            }
            if let a = c.agents?.codex {
                AgentChip(name: "Codex", detail: a.version, ok: a.signedIn == true, bad: a.signedIn == false,
                          help: a.signedIn == false ? "Not signed in: run `codex login` on this machine once" : a.signedIn == true ? "Signed in; \(a.models.count) models" : "Sign-in not checked (older Sidefleet)")
            }
            if let a = c.agents?.opencode {
                let none = (a.modelsTotal ?? 0) == 0
                AgentChip(name: "opencode", detail: none ? a.version : "\(a.modelsTotal ?? 0) models", ok: !none, bad: none,
                          help: none ? "No provider set up: run `opencode auth login` on this machine (e.g. OpenRouter)" : "Version \(a.version)")
            }
            if let a = c.agents?.ollama {
                AgentChip(name: "Ollama", detail: "\(a.models.count) models", ok: false, bad: false, help: "Version \(a.version)")
            }
            if c.agents?.claude == nil, c.agents?.codex == nil, c.agents?.opencode == nil {
                Text("No coding agent found")
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .padding(.horizontal, 8)
                    .frame(height: 24)
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
            }
            ForEach(limits, id: \.0) { name, until in
                HStack(spacing: 6) {
                    Image(systemName: "exclamationmark.triangle").font(.system(size: 11))
                    Text("\(severalAgents ? "\(name): " : "")usage limit until ")
                    + Text(until).monospacedDigit()
                }
                .font(SF.font(12))
                .foregroundStyle(SF.queueInk)
                .padding(.horizontal, 8)
                .frame(height: 24)
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(SF.queue.opacity(0.5)))
                .help("This machine's \(name) account hit its usage limit. Its \(name) tasks go to other machines until then.")
            }
            Button { stores.repoPickerOpen = true } label: {
                let chosen = fleet.added.map { keys.intersection($0).count } ?? 0
                Text("\(keys.count) repos\((fleet.added?.isEmpty ?? true) ? "" : ", \(chosen) chosen")")
                    .font(SF.font(12))
                    .monospacedDigit()
                    .padding(.horizontal, 8)
                    .frame(height: 24)
            }
            .buttonStyle(QuietButtonStyle(cornerRadius: 6))
        }
    }

    /// Agents this machine takes no tasks for until their usage limit resets.
    private var limits: [(String, String)] {
        worker.limitedUntil.filter { $0.value > fleet.now }.sorted { $0.key < $1.key }.map { key, until in
            (Format.limitedAgentName(key), Format.limitClock(until, now: fleet.now))
        }
    }

    private var severalAgents: Bool { [c.agents?.claude, c.agents?.codex, c.agents?.opencode].compactMap { $0 }.count > 1 }

    @ViewBuilder private var footer: some View {
        HStack(spacing: 12) {
            Group {
                if !build.sha.isEmpty {
                    Text("Sidefleet \(build.version), build ") + Text(build.sha).font(SF.mono(12))
                } else {
                    Text("Version unknown")
                }
            }
            .lineLimit(1)
            .foregroundStyle(SF.muted)
            .help(c.version)
            Spacer(minLength: 4)
            if !worker.online, fleet.canRemove(worker) {
                Button("Remove") { confirmingRemove = true }.buttonStyle(.plain).foregroundStyle(SF.muted)
            } else if !fleet.canUpdate(worker) {
                Text("Runs only \(worker.owner.map { "@\($0.login)'s" } ?? "its owner's") tasks").foregroundStyle(SF.muted)
            } else if worker.updateRequested {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("Updating when idle")
                }
                .foregroundStyle(SF.runInk)
            } else if worker.outdated, worker.online {
                Button("Update to latest") { update() }
                    .buttonStyle(SecondaryButtonStyle(height: 26, tint: SF.queueInk))
                    .disabled(requesting)
            } else if worker.online, !c.version.isEmpty {
                Button("Up to date") { update() }
                    .buttonStyle(.plain)
                    .foregroundStyle(SF.muted)
                    .disabled(requesting)
            }
        }
        .font(SF.font(12))
    }

    private func update() {
        requesting = true
        Task {
            await fleet.requestUpdate(worker)
            requesting = false
        }
    }
}

private struct AgentChip: View {
    var name: String
    var detail: String
    var ok: Bool
    var bad: Bool
    var help: String

    var body: some View {
        HStack(spacing: 6) {
            if bad {
                Image(systemName: "exclamationmark.triangle").font(.system(size: 11))
            } else if ok {
                Image(systemName: "checkmark.circle").font(.system(size: 11)).foregroundStyle(SF.ok)
            }
            Text(name)
            Text(detail).monospacedDigit().foregroundStyle(SF.muted)
        }
        .font(SF.font(12))
        .foregroundStyle(bad ? SF.errInk : SF.ink)
        .padding(.horizontal, 8)
        .frame(height: 24)
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(bad ? SF.err.opacity(0.4) : SF.border))
        .help(help)
    }
}

/// CPU over the readings collected while the app is open (one per heartbeat).
private struct Sparkline: View {
    var values: [Double]

    var body: some View {
        Canvas { ctx, size in
            var mid = Path()
            mid.move(to: CGPoint(x: 0, y: size.height / 2))
            mid.addLine(to: CGPoint(x: size.width, y: size.height / 2))
            ctx.stroke(mid, with: .color(SF.border), style: StrokeStyle(lineWidth: 1, dash: [2, 3]))
            let h = values.count == 1 ? [values[0], values[0]] : values
            guard h.count >= 2 else { return }
            let usable = size.height - 2
            let points = h.enumerated().map { i, v in
                CGPoint(x: CGFloat(i) / CGFloat(h.count - 1) * size.width, y: size.height - CGFloat(min(v, 100)) / 100 * usable - 1)
            }
            var line = Path()
            line.addLines(points)
            var area = line
            area.addLine(to: CGPoint(x: size.width, y: size.height))
            area.addLine(to: CGPoint(x: 0, y: size.height))
            area.closeSubpath()
            ctx.fill(area, with: .linearGradient(Gradient(colors: [SF.run.opacity(0.25), SF.run.opacity(0)]), startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height)))
            ctx.stroke(line, with: .color(SF.run), style: StrokeStyle(lineWidth: 1.6, lineJoin: .round))
        }
    }
}

// MARK: Fleet map

/// The fleet at a glance: the control plane in the centre, machines on two orbits. Heartbeats arrive as
/// small green dots, a task starting sends a violet dot out, and running tasks circle their machine.
/// Deliberately quiet: motion only means something happened.
private struct FleetMap: View {
    @Binding var hover: String?
    /// A machine's node was clicked: bring its card into view.
    var onOpen: (String) -> Void
    @Environment(Stores.self) private var stores
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pulses: [Pulse] = []
    @State private var beats = 0

    private struct Pulse: Identifiable {
        let id = UUID()
        var from: String
        var to: String
        var isTask: Bool
        var isBeat: Bool
        var born: Date
        /// Something that happened on the centre machine itself: nothing travels, it just flashes.
        var instant = false

        var travel: TimeInterval { instant ? 0 : isBeat ? 1.5 : 1.3 }

        /// How bright the ring around where it landed is: full on arrival, gone within a second.
        func flash(at now: Date) -> Double {
            let since = now.timeIntervalSince(born) - travel
            guard since >= 0 else { return 0 }
            return max(0, (isBeat ? 0.6 : 1) - since * 1.5)
        }
    }

    private struct Node: Identifiable {
        var id: String
        var name: String
        var worker: Worker?
        var ring: Int
        var angle: Double
        var phase: Double
    }

    private static let hub = "__control_plane__"

    private var fleet: FleetStore { stores.fleet }

    private var nodes: [Node] {
        let hostID = fleet.setup?.hostWorker
        let host = fleet.workers.first { $0.id == hostID }
        let others = fleet.workers.filter { $0.id != host?.id }.sorted { $0.displayName < $1.displayName }
        let preset: [Int: [Double]] = [1: [-1.57], 2: [-2.45, -0.62], 3: [-2.45, -0.62, 1.57]]
        let spread: [Double] = others.indices.map { i in -2.45 + Double(i) * .pi * 2 / Double(others.count) }
        let angles: [Double] = preset[others.count] ?? spread
        var out = [Node(id: host?.id ?? Self.hub, name: host?.displayName ?? "Control plane", worker: host, ring: 0, angle: 0, phase: 0)]
        for (i, w) in others.enumerated() {
            out.append(Node(id: w.id, name: w.displayName, worker: w, ring: i % 2 == 0 ? 1 : 2, angle: angles[i], phase: Double(i) * 1.7))
        }
        return out
    }

    private func running(_ id: String) -> Int { fleet.runs.filter { $0.threadStatus == .running && $0.run.workerID == id }.count }

    private func center(_ size: CGSize) -> CGPoint { CGPoint(x: size.width / 2, y: size.height / 2) }
    private func wide(_ size: CGSize) -> Bool { size.width / max(size.height, 1) > 1.6 }
    private func ringRadius(_ ring: Int, _ size: CGSize) -> CGFloat { min(size.width, size.height) * [0, 0.27, 0.4][ring] }
    private func xStretch(_ size: CGSize) -> CGFloat { wide(size) ? 1.7 : 1.15 }

    private func position(_ n: Node, _ size: CGSize) -> CGPoint {
        let c = center(size)
        if n.ring == 0 { return c }
        return CGPoint(x: c.x + CGFloat(cos(n.angle)) * ringRadius(n.ring, size) * xStretch(size), y: c.y + CGFloat(sin(n.angle)) * ringRadius(n.ring, size))
    }

    private func radius(_ n: Node) -> CGFloat { 10 + CGFloat(Double(n.worker?.capabilities.system?.cpuCores ?? 4).squareRoot()) * 3 }

    var body: some View {
        GeometryReader { geo in
            // Stops drawing when nothing moves: an idle fleet costs no battery.
            TimelineView(.animation(minimumInterval: 1.0 / 30, paused: reduceMotion || (pulses.isEmpty && fleet.running == 0))) { timeline in
                Canvas { ctx, size in
                    draw(&ctx, size, now: timeline.date)
                }
            }
            .overlay { labels(geo.size) }
            .contentShape(Rectangle())
            // Pointing at a machine lights up its card; clicking scrolls to it.
            .onContinuousHover { phase in
                switch phase {
                case .active(let at): hover = machine(at: at, geo.size)
                case .ended: hover = nil
                }
            }
            .onTapGesture {
                if let hover { onOpen(hover) }
            }
        }
        .frame(height: 300)
        .overlay(alignment: .topTrailing) {
            (Text("\(beats)").foregroundColor(SF.ink) + Text(" heartbeats while you watched"))
                .font(SF.font(12))
                .monospacedDigit()
                .foregroundStyle(SF.muted)
                .padding(14)
        }
        .overlay(alignment: .bottomLeading) {
            HStack(spacing: 14) {
                legend(SF.ok, "Heartbeat")
                legend(SF.run, "Task")
            }
            .padding(14)
        }
        .card()
        .onChange(of: fleet.eventCounter) { _, _ in take(fleet.recentEvents) }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Fleet map: \(fleet.online.count) of \(fleet.workers.count) machines online")
    }

    private func legend(_ color: Color, _ label: String) -> some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(label)
        }
        .font(SF.font(12))
        .foregroundStyle(SF.muted)
    }

    private func take(_ events: [FleetEvent]) {
        let now = Date()
        let hub = nodes.first?.id ?? Self.hub
        // Long enough for the slowest dot to land and its ring to fade.
        pulses.removeAll { now.timeIntervalSince($0.born) > 2.5 }
        for e in events {
            switch e {
            case .beat(let w):
                beats += 1
                pulses.append(Pulse(from: w, to: hub, isTask: false, isBeat: true, born: now, instant: w == hub))
            case .dispatch(let w):
                pulses.append(Pulse(from: hub, to: w, isTask: true, isBeat: false, born: now, instant: w == hub))
            case .done(let w):
                if w != hub { pulses.append(Pulse(from: w, to: hub, isTask: false, isBeat: false, born: now)) }
            }
        }
    }

    private func draw(_ ctx: inout GraphicsContext, _ size: CGSize, now: Date) {
        let list = nodes
        guard let hub = list.first else { return }
        let c = center(size)
        for ring in [1, 2] where list.contains(where: { $0.ring == ring }) {
            let rx = ringRadius(ring, size) * xStretch(size), ry = ringRadius(ring, size)
            ctx.stroke(Path(ellipseIn: CGRect(x: c.x - rx, y: c.y - ry, width: rx * 2, height: ry * 2)), with: .color(SF.border), lineWidth: 1)
        }
        for n in list where n.ring != 0 {
            var p = Path()
            p.move(to: c)
            p.addLine(to: position(n, size))
            let on = n.worker?.online ?? false
            let hot = hover == n.id
            ctx.stroke(p, with: .color((hot ? SF.run : SF.border).opacity(on ? 1 : 0.5)), style: StrokeStyle(lineWidth: hot ? 1.5 : 1, dash: [3, 5]))
        }
        // Dots travelling along links.
        for pulse in pulses where !pulse.instant {
            guard let a = list.first(where: { $0.id == pulse.from }), let b = list.first(where: { $0.id == pulse.to }) else { continue }
            let t = reduceMotion ? 1 : min(1, now.timeIntervalSince(pulse.born) / pulse.travel)
            guard t < 1 else { continue }
            let e = 1 - (1 - t) * (1 - t)
            let pa = position(a, size), pb = position(b, size)
            let r: CGFloat = pulse.isBeat ? 2.2 : 3.2
            let at = CGPoint(x: pa.x + (pb.x - pa.x) * e, y: pa.y + (pb.y - pa.y) * e)
            ctx.fill(Path(ellipseIn: CGRect(x: at.x - r, y: at.y - r, width: r * 2, height: r * 2)), with: .color((pulse.isTask ? SF.run : SF.ok).opacity(pulse.isBeat ? 0.5 : 0.85)))
        }
        // Machines.
        for n in list {
            let p = position(n, size), r = radius(n)
            let on = n.ring == 0 && n.worker == nil ? true : (n.worker?.online ?? false)
            let circle = Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2))
            // A ring that widens and fades where a heartbeat or a task just landed.
            let f = reduceMotion ? 0 : pulses.filter { $0.to == n.id }.map { $0.flash(at: now) }.max() ?? 0
            if f > 0 {
                let fr = r + 4 + CGFloat(1 - f) * 8
                ctx.stroke(Path(ellipseIn: CGRect(x: p.x - fr, y: p.y - fr, width: fr * 2, height: fr * 2)), with: .color((running(n.id) > 0 ? SF.run : SF.ok).opacity(f * 0.3)), lineWidth: 1.5)
            }
            ctx.opacity = on ? 1 : 0.55
            ctx.fill(circle, with: .color(SF.card))
            ctx.stroke(circle, with: .color(hover == n.id ? SF.run : SF.border), lineWidth: hover == n.id ? 2 : 1)
            if on, let cpu = n.worker?.capabilities.system?.cpuPercent {
                var arc = Path()
                arc.addArc(center: p, radius: r - 4.5, startAngle: .degrees(-90), endAngle: .degrees(-90 + 360 * max(cpu, 1) / 100), clockwise: false)
                ctx.stroke(arc, with: .color(cpu > 75 ? SF.queue : SF.ink), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
            }
            ctx.opacity = 1
            if n.id == hub.id { drawMark(&ctx, at: p) }
            // Running tasks circle their machine.
            let busy = running(n.id)
            for i in 0..<busy {
                let spin = reduceMotion ? 0 : now.timeIntervalSinceReferenceDate / 2.6
                let ang = n.phase + spin + Double(i) * .pi * 2 / Double(busy)
                let at = CGPoint(x: p.x + CGFloat(cos(ang)) * (r + 7), y: p.y + CGFloat(sin(ang)) * (r + 7))
                ctx.fill(Path(ellipseIn: CGRect(x: at.x - 2.5, y: at.y - 2.5, width: 5, height: 5)), with: .color(SF.run))
            }
        }
    }

    private func labels(_ size: CGSize) -> some View {
        ForEach(nodes) { n in
            let p = position(n, size), r = radius(n)
            VStack(spacing: 0) {
                Text(n.name).font(SF.font(13, .semibold)).foregroundStyle((n.worker?.online ?? (n.ring == 0)) ? SF.ink : SF.muted)
                Text(detail(n)).font(SF.font(11.5)).foregroundStyle(SF.muted)
            }
            .fixedSize()
            .position(x: p.x, y: p.y + r + 22)
            .allowsHitTesting(false)
        }
    }

    private func detail(_ n: Node) -> String {
        guard let w = n.worker else { return "control plane" }
        // The machine in the centre is the one the control plane runs on.
        let prefix = n.ring == 0 ? "control plane, " : ""
        if !w.online { return prefix + "offline, seen \(Format.rel(w.lastHeartbeatAt, now: fleet.now))" }
        let busy = running(w.id)
        let cpu = w.capabilities.system.map { "CPU \(Int($0.cpuPercent.rounded()))%" }
        return prefix + [busy > 0 ? "\(busy) running" : "idle", cpu].compactMap { $0 }.joined(separator: ", ")
    }

    /// The machine under the pointer, if any. The bare control plane has no card to point at.
    private func machine(at point: CGPoint, _ size: CGSize) -> String? {
        nodes.first { n in
            let p = position(n, size)
            return n.worker != nil && hypot(p.x - point.x, p.y - point.y) < radius(n) + 10
        }?.id
    }

    /// The Sidefleet mark (site/brand/mark.py): fibres from the page to two machines.
    private func drawMark(_ ctx: inout GraphicsContext, at p: CGPoint) {
        let s: CGFloat = 0.55
        func at(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: p.x + (x - 16) * s, y: p.y + (y - 16) * s) }
        let fibres: [(CGFloat, [UInt32])] = [(7.5, [0x3fb8c9, 0x5c98ff, 0x8b5cf6]), (24.5, [0xff6fae, 0xf7797d, 0xe9b13c])]
        for (ty, hues) in fibres {
            for (i, hue) in hues.enumerated() {
                let off = CGFloat(i - 1) * 2.4
                var strand = Path()
                strand.move(to: at(6, 16))
                strand.addCurve(to: at(26, ty), control1: at(17, 16 + off), control2: at(15, ty + off))
                ctx.stroke(strand, with: .color(Color(rgb: hue)), style: StrokeStyle(lineWidth: 1.6 * s, lineCap: .round))
            }
        }
        let dots: [(CGFloat, CGFloat)] = [(6, 16), (26, 7.5), (26, 24.5)]
        for (x, y) in dots {
            let c = at(x, y), r = 3.2 * s
            ctx.fill(Path(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2)), with: .color(SF.ink))
        }
    }
}

// MARK: Adding machines

/// How to put a machine on the fleet. Used inline for onboarding and as a sheet on Machines.
struct AddMachineSteps: View {
    @Environment(Stores.self) private var stores
    @State private var mode = "terminal"

    private static let install = "curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh"

    private var origin: String { stores.controlURL?.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? "" }
    private var token: String? { stores.fleet.setup?.workerToken.isEmpty == false ? stores.fleet.setup?.workerToken : nil }
    private func masked(_ t: String) -> String { String(t.prefix(4)) + "••••••••" }

    private func connect(masking: Bool) -> String {
        guard let token else { return "sidefleet connect --control \(origin)" }
        return "sidefleet connect --control \(origin) --token \(masking ? masked(token) : token)"
    }

    private func prompt(masking: Bool) -> String {
        guard let token else {
            return "Read \(origin)/connect.md and follow it to connect this machine to Sidefleet. Control plane: \(origin). There is no token: run connect without --token and tell me the code it prints so I can approve it on the Machines page."
        }
        return "Read \(origin)/connect.md and follow it to connect this machine to Sidefleet. Control plane: \(origin). Worker token: \(masking ? masked(token) : token) (keep it out of logs and your summary)."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Picker("How to set it up", selection: $mode) {
                Text("In a terminal").tag("terminal")
                Text("Ask an agent").tag("agent")
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 240)
            if mode == "terminal" {
                step(1, "Install Sidefleet on that machine. No sudo; one binary in ~/.local/bin.", shown: Self.install, copy: Self.install)
                step(2, "Connect it. It checks which agents are installed and signed in, finds your git checkouts, and stays connected after a reboot." + (token == nil ? " It prints a link and a code to approve it with." : ""), shown: connect(masking: true), copy: connect(masking: false))
            } else {
                Text("Paste this into Claude Code or Codex on that machine. It reads the setup steps from this control plane and does the install and connect for you.")
                    .font(SF.font(13))
                    .foregroundStyle(SF.ink)
                    .fixedSize(horizontal: false, vertical: true)
                CommandBlock(shown: prompt(masking: true), copy: prompt(masking: false), label: "Copy prompt")
            }
            // The commands above carry this app's address for the control plane, which only works from here.
            if Format.isLoopback(stores.controlURL) {
                Text("This app reaches the control plane at `\(origin)`, which other machines can't reach. Use this machine's Tailscale or LAN address in the connect command instead.")
                    .font(SF.font(12))
                    .lineSpacing(3)
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func step(_ n: Int, _ text: String, shown: String, copy: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            (Text("\(n)  ").foregroundColor(SF.muted) + Text(text)).font(SF.font(13)).fixedSize(horizontal: false, vertical: true)
            CommandBlock(shown: shown, copy: copy)
        }
    }
}

/// A terminal command, copied in full. `shown` may mask a secret that `copy` contains.
struct CommandBlock: View {
    var shown: String
    var copy: String
    var label = "Copy"

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Text(shown)
                .font(SF.mono(12))
                .lineSpacing(3)
                .foregroundStyle(SF.ink.opacity(0.85))
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            CopyButton(text: { copy }, label: label)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(SF.code, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
    }
}

struct AddMachineSheet: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Add a machine").font(SF.font(15, .semibold))
            Text("Any Mac or Linux box with Claude Code, Codex or opencode set up. It connects out to this control plane; nothing connects in.")
                .font(SF.font(13))
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            AddMachineSteps()
            HStack {
                Spacer()
                Button("Done") { dismiss() }.buttonStyle(PrimaryButtonStyle()).keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 560)
    }
}

// Pending machines and worker tokens are in PendingMachines.swift and WorkerTokens.swift.

// MARK: Choosing repos

/// Which of the machines' checkouts fill the composer's menu.
struct RepoPickerSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var selected: Set<String> = []
    /// Frozen when it opens, chosen first, so rows don't jump while ticking.
    @State private var order: [String] = []
    @State private var saving = false
    /// The row the arrow keys are on.
    @State private var act = 0
    @FocusState private var searching: Bool

    private var terms: [String] { RepoSearch.terms(query) }

    /// What each machine calls the project's folder, which you may know it by.
    private var folders: [String: [String]] {
        var out: [String: [String]] = [:]
        for w in stores.fleet.workers {
            for r in w.capabilities.repos { out[r.key, default: []].append(r.name) }
        }
        return out
    }

    private var shown: [String] {
        let terms = terms, folders = folders
        return order.filter { RepoSearch.matches(label: Format.repoLabel($0), folders: folders[$0] ?? [], terms: terms) }
    }

    private func wasAdded(_ repo: String) -> Bool { stores.fleet.added?.contains(repo) ?? false }

    private func toggle(_ repo: String) {
        if selected.contains(repo) { selected.remove(repo) } else { selected.insert(repo) }
    }

    private func move(_ by: Int, in shown: [String]) {
        guard !shown.isEmpty else { return }
        act = (min(act, shown.count - 1) + by + shown.count) % shown.count
    }

    /// The name with what you searched for marked in it.
    private func marked(_ label: String, _ terms: [String]) -> AttributedString {
        var out = AttributedString()
        for part in RepoSearch.parts(label, terms: terms) {
            var piece = AttributedString(part.text)
            if part.marked { piece.backgroundColor = SF.queueSoft }
            out += piece
        }
        return out
    }

    var body: some View {
        let shown = shown, terms = terms
        // Headings only while nothing is typed and something was chosen before.
        let sections = terms.isEmpty && order.contains(where: wasAdded)
        return VStack(alignment: .leading, spacing: 12) {
            Text("Choose your repos").font(SF.font(15, .semibold))
            Text("Only these appear in the repo menu when you start a task. Machines still report every checkout.").font(SF.font(13)).foregroundStyle(SF.muted)
            TextField("Search \(stores.fleet.allRepos.count) repos", text: $query)
                .textFieldStyle(.roundedBorder)
                .focused($searching)
                .accessibilityLabel("Search repos")
                .onChange(of: query) { _, _ in act = 0 }
                .onKeyPress(.downArrow) {
                    move(1, in: shown)
                    return .handled
                }
                .onKeyPress(.upArrow) {
                    move(-1, in: shown)
                    return .handled
                }
                // Return ticks the row the arrows are on; ⌘Return saves.
                .onSubmit {
                    if act < shown.count { toggle(shown[act]) }
                    searching = true
                }
            HStack(spacing: 12) {
                Text("\(selected.count) selected" + (terms.isEmpty ? "" : " · \(shown.count) match\(shown.count == 1 ? "" : "es")")).monospacedDigit()
                Spacer()
                if !terms.isEmpty, !shown.isEmpty {
                    Button("Select matching") { selected.formUnion(shown) }.buttonStyle(.plain).underline()
                }
                if !selected.isEmpty {
                    Button("Clear") { selected = [] }.buttonStyle(.plain).underline()
                }
            }
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 1) {
                        ForEach(Array(shown.enumerated()), id: \.element) { i, repo in
                            if sections, i == 0 {
                                Text("Chosen").font(SF.font(12, .medium)).foregroundStyle(SF.muted).padding(.horizontal, 10).padding(.top, 8).padding(.bottom, 4)
                            }
                            if sections, !wasAdded(repo), i == 0 || wasAdded(shown[i - 1]) {
                                Text("Everything else").font(SF.font(12, .medium)).foregroundStyle(SF.muted).padding(.horizontal, 10).padding(.top, 12).padding(.bottom, 4)
                            }
                            // Where a task in it can run now.
                            let online = (stores.fleet.allRepos[repo] ?? []).filter(\.online).map(\.displayName)
                            Button { toggle(repo) } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: selected.contains(repo) ? "checkmark.square.fill" : "square")
                                        .font(.system(size: 14))
                                        .foregroundStyle(selected.contains(repo) ? SF.ink : SF.muted)
                                    Text(marked(Format.repoLabel(repo), terms)).font(SF.mono(12.5)).lineLimit(1).truncationMode(.middle)
                                    Spacer(minLength: 8)
                                    Text(online.isEmpty ? "offline" : online.joined(separator: ", "))
                                        .font(SF.font(12))
                                        .foregroundStyle(SF.faint)
                                        .lineLimit(1)
                                }
                                .padding(.horizontal, 10)
                                .padding(.vertical, 7)
                                .hoverBackground(active: i == act)
                            }
                            .buttonStyle(.plain)
                            .id(repo)
                            .accessibilityAddTraits(selected.contains(repo) ? .isSelected : [])
                        }
                        if shown.isEmpty {
                            Text(order.isEmpty ? "No machine has reported any git checkouts yet." : "No repo matches “\(query)”.")
                                .font(SF.font(13)).foregroundStyle(SF.muted).frame(maxWidth: .infinity).padding(24)
                        }
                    }
                    .padding(6)
                }
                .onChange(of: act) { _, i in
                    if i < shown.count { proxy.scrollTo(shown[i]) }
                }
            }
            .frame(height: 320)
            .card(cornerRadius: 10)
            HStack(spacing: 6) {
                Kbd(text: "↑"); Kbd(text: "↓"); Text("move")
                Kbd(text: "↵"); Text("select")
                Kbd(text: "⌘↵"); Text("save")
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction)
                Button(saving ? "Saving…" : "Save" + (selected.isEmpty ? "" : " \(selected.count)")) { save() }
                    .buttonStyle(PrimaryButtonStyle()).keyboardShortcut(.return, modifiers: .command).disabled(saving)
            }
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
        }
        .padding(20)
        .frame(width: 520)
        .onAppear {
            let repos = stores.fleet.allRepos
            let added = stores.fleet.added ?? []
            selected = added.filter { repos[$0] != nil }
            order = RepoSearch.order(Array(repos.keys), chosen: selected)
            act = 0
            searching = true
        }
    }

    private func save() {
        saving = true
        Task {
            do {
                try await stores.fleet.saveAdded(Array(selected))
                dismiss()
            } catch {
                stores.toasts.error("Could not save your repos", error)
            }
            saving = false
        }
    }
}
