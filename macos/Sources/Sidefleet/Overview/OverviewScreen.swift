import SidefleetKit
import SwiftUI

/// What the fleet is doing, and every task it has run, newest activity first.
struct OverviewScreen: View {
    @Environment(Stores.self) private var stores
    @State private var filter = Filter.all
    @State private var sortOrder = [KeyPathComparator(\Row.updated, order: .reverse)]
    /// A click anywhere on a row opens its task; the selection only carries the click.
    @State private var picked: String?

    private enum Filter: String, CaseIterable, Identifiable {
        case all = "All", running = "Running", queued = "Queued", problems = "Failed or stopped", done = "Done"
        var id: String { rawValue }

        func matches(_ r: RunSummary) -> Bool {
            switch self {
            case .all: true
            case .running: r.threadStatus == .running
            case .queued: r.threadStatus == .queued
            case .problems: r.threadStatus == .failed || r.threadStatus == .stopped
            case .done: r.threadStatus == .succeeded
            }
        }
    }

    /// A list row as the table sorts it.
    private struct Row: Identifiable {
        var summary: RunSummary
        var took: Double
        var cost: Double
        var updated: Date
        var title: String
        var machine: String
        var id: String { summary.id }
    }

    private var fleet: FleetStore { stores.fleet }

    private func count(_ f: Filter) -> Int { fleet.runs.filter(f.matches).count }

    private var rows: [Row] {
        fleet.runs.filter(filter.matches).map { r in
            let end = r.threadStatus == .running ? fleet.now : (r.run.finishedAt ?? r.lastActivityAt)
            return Row(summary: r, took: r.run.startedAt.map { end.timeIntervalSince($0) } ?? 0, cost: r.run.result?["total_cost_usd"]?.double ?? 0, updated: r.lastActivityAt, title: r.run.task, machine: r.run.workerID ?? "")
        }.sorted(using: sortOrder)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Overview").font(SF.font(22, .semibold)).tracking(-0.3)
                Text("What the fleet is doing, and every task it has run, newest activity first.").font(SF.font(13)).foregroundStyle(SF.muted)
            }
            stats
            filters
            list
        }
        .column(SF.wideColumn)
        .padding(.horizontal, 28)
        .padding(.top, 20)
        .padding(.bottom, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(SF.background)
    }

    private var stats: some View {
        let busy = Set(fleet.runs.filter { $0.threadStatus == .running }.compactMap(\.run.workerID)).count
        let doneToday = fleet.runs.filter { $0.threadStatus == .succeeded && Calendar.current.isDateInToday($0.lastActivityAt) }.count
        let spend = fleet.runs.reduce(0) { $0 + ($1.run.result?["total_cost_usd"]?.double ?? 0) }
        return HStack(spacing: 0) {
            Stat(title: "Running now", value: "\(count(.running))", note: busy > 0 ? "on \(busy) machine\(busy == 1 ? "" : "s")" : "all machines idle")
            Stat(title: "Waiting", value: "\(count(.queued))", note: count(.queued) > 0 ? "for a machine with the repo" : "nothing in the queue")
            Stat(title: "Done today", value: "\(doneToday)", note: "\(count(.problems)) failed or stopped in total")
            Stat(title: "Agent spend", value: Format.money(spend), note: "as reported by the agents (Codex reports none)")
        }
        // The lines are an overlay so they take the row's height: Dividers in the row would need
        // `fixedSize`, which grows the whole split view past the top and bottom of the window.
        .overlay {
            HStack(spacing: 0) {
                ForEach(0..<3, id: \.self) { _ in
                    Color.clear
                    Rectangle().fill(SF.border).frame(width: 1)
                }
                Color.clear
            }
        }
        .card()
    }

    private var filters: some View {
        HStack(spacing: 4) {
            ForEach(Filter.allCases) { f in
                Button {
                    filter = f
                } label: {
                    HStack(spacing: 6) {
                        Text(f.rawValue)
                        Text("\(count(f))").font(SF.font(12)).monospacedDigit().opacity(0.6)
                    }
                    .font(SF.font(13))
                    .foregroundStyle(filter == f ? SF.inkInverse : SF.muted)
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .background(filter == f ? SF.ink : .clear, in: Capsule())
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var list: some View {
        Table(rows, selection: $picked, sortOrder: $sortOrder) {
            TableColumn("Task", value: \.title) { row in
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title).font(SF.font(13, .medium)).lineLimit(1)
                    HStack(spacing: 8) {
                        Text(Format.repoLabel(row.summary.run.repo)).font(SF.mono(11.5))
                        Text(Format.agentModel(row.summary.run.agent, row.summary.run.model))
                        if row.summary.followUps > 0 { Text("\(row.summary.followUps + 1) messages").monospacedDigit() }
                    }
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .lineLimit(1)
                }
                .padding(.vertical, 4)
            }
            .width(min: 240, ideal: 420)
            TableColumn("Machine", value: \.machine) { row in
                if row.machine.isEmpty {
                    Text("No machine yet").font(SF.font(13)).foregroundStyle(SF.muted)
                } else {
                    HStack(spacing: 8) {
                        StatusDot(status: fleet.workers.first { $0.id == row.machine }?.online == true ? .online : .offline)
                        Text(row.machine).font(SF.font(13)).lineLimit(1)
                    }
                }
            }
            .width(min: 90, ideal: 130)
            TableColumn("Status") { row in
                if let start = row.summary.run.startAt, row.summary.run.isScheduled(now: fleet.now) {
                    StatusPill(status: .queued, label: "Scheduled", extra: Format.startTime(start, now: fleet.now))
                } else {
                    StatusPill(status: row.summary.threadStatus)
                }
            }
            .width(min: 90, ideal: 110)
            TableColumn("Took", value: \.took) { row in
                Text(Format.duration(start: row.summary.run.startedAt, end: row.summary.threadStatus == .running ? nil : (row.summary.run.finishedAt ?? row.summary.lastActivityAt), now: fleet.now).ifEmpty("–"))
                    .font(SF.font(13)).monospacedDigit().foregroundStyle(SF.muted)
            }
            .width(min: 60, ideal: 70)
            TableColumn("Cost", value: \.cost) { row in
                Text(row.cost > 0 ? Format.money(row.cost, digits: 3) : "–").font(SF.font(13)).monospacedDigit()
            }
            .width(min: 50, ideal: 60)
            TableColumn("Updated", value: \.updated) { row in
                Text(Format.rel(row.updated, now: fleet.now)).font(SF.font(12)).monospacedDigit().foregroundStyle(SF.muted)
            }
            .width(min: 60, ideal: 70)
        }
        .onChange(of: picked) { _, id in
            guard let id else { return }
            picked = nil
            stores.open(id)
        }
        .tableStyle(.inset(alternatesRowBackgrounds: false))
        .scrollContentBackground(.hidden)
        .card()
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            if rows.isEmpty {
                VStack(spacing: 12) {
                    Text(fleet.runs.isEmpty ? "Nothing has run yet." : "No runs match this filter.").font(SF.font(13)).foregroundStyle(SF.muted)
                    if fleet.runs.isEmpty {
                        Button("Start your first task") { stores.route = .newTask }.buttonStyle(.plain).font(SF.font(13, .medium)).underline()
                    }
                }
            }
        }
    }
}

private struct Stat: View {
    var title: String
    var value: String
    var note: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(SF.font(12)).foregroundStyle(SF.muted)
            Text(value).font(SF.font(24, .medium)).tracking(-0.3).monospacedDigit()
            Text(note).font(SF.font(12)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

extension String {
    fileprivate func ifEmpty(_ other: String) -> String { isEmpty ? other : self }
}
