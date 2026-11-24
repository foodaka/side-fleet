import AppKit
import SidefleetKit
import SwiftUI

/// Pull requests: what you authored in the GitHub repos on your machines, open ones and the latest
/// closed, as your machines see it with the GitHub CLI. They look when this screen asks.
/// A port of web/src/routes/pulls/+page.svelte.
struct PullRequestsScreen: View {
    @Environment(Stores.self) private var stores

    private var store: PullRequestsStore { stores.pullRequests }

    var body: some View {
        @Bindable var store = store
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                ScreenHeader(
                    title: "Pull requests",
                    detail: "Yours, in the GitHub repos on your machines: open ones and the latest merged or closed. Your machines look with the GitHub CLI while this screen is open."
                )
                toolbar(store)
                status
                content
            }
            .column(SF.column + 196)
            .padding(.horizontal, 28)
            .padding(.top, 20)
            .padding(.bottom, 28)
        }
        .background(SF.background)
        // While it's open: the list every few seconds, a fresh look on GitHub every two minutes.
        .task(id: stores.fleet.orgID) {
            var lastAsked = Date.distantPast
            while !Task.isCancelled {
                if stores.fleet.loaded {
                    if Date().timeIntervalSince(lastAsked) > 120 {
                        lastAsked = Date()
                        try? await store.refresh()
                    } else {
                        await store.load()
                    }
                }
                try? await Task.sleep(for: .seconds(stores.fleet.loaded ? 5 : 1))
            }
        }
    }

    private func toolbar(_ store: PullRequestsStore) -> some View {
        @Bindable var store = store
        return HStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(SF.muted)
                TextField("Search pull requests", text: $store.query)
                    .textFieldStyle(.plain)
                    .font(SF.font(13))
            }
            .padding(.horizontal, 12)
            .frame(height: 30)
            .background(SF.card, in: Capsule())
            .overlay(Capsule().strokeBorder(SF.input))
            .frame(minWidth: 200)

            Picker("Show", selection: $store.show) {
                ForEach(PullRequestsStore.Show.allCases, id: \.self) { Text($0.title).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()

            Menu {
                Picker("Sort", selection: $store.sort) {
                    ForEach(PullRequestsStore.Sort.allCases, id: \.self) { Text($0.title).tag($0) }
                }
                .pickerStyle(.inline)
                .labelsHidden()
            } label: {
                Label(store.sort.title, systemImage: "arrow.up.arrow.down")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()

            Menu {
                Toggle("Checks failing", isOn: $store.failing)
                Toggle("Changes requested", isOn: $store.changesRequested)
                Toggle("Hide drafts", isOn: $store.hideDrafts)
                if store.repos.count > 1 {
                    Divider()
                    Picker("Repo", selection: $store.repo) {
                        Text("All repos").tag(String?.none)
                        ForEach(store.repos, id: \.self) { Text($0).tag(String?.some($0)) }
                    }
                }
                if store.filterCount > 0 {
                    Divider()
                    Button("Clear filters") { store.clearFilters() }
                }
            } label: {
                Label(store.filterCount > 0 ? "Filters (\(store.filterCount))" : "Filters", systemImage: "line.3.horizontal.decrease")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()

            Button {
                Task {
                    do { try await store.refresh() } catch { stores.toasts.error("Could not ask your machines", error) }
                }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .rotationEffect(.degrees(store.looking ? 360 : 0))
                    .animation(store.looking ? .linear(duration: 1).repeatForever(autoreverses: false) : .default, value: store.looking)
            }
            .buttonStyle(SecondaryButtonStyle())
            .disabled(store.looking)
            .help(store.lastLooked.map { "Looked \(Format.rel($0, now: stores.fleet.now)). Look again" } ?? "Look again")
        }
        .font(SF.font(12.5))
    }

    @ViewBuilder private var status: some View {
        VStack(alignment: .leading, spacing: 2) {
            if store.looking {
                Text("Your machines are looking on GitHub…")
            } else if let at = store.lastLooked {
                Text("Looked \(Format.rel(at, now: stores.fleet.now)).")
            }
            ForEach(store.notes, id: \.self) { Text($0).foregroundStyle(SF.queueInk) }
        }
        .font(SF.font(12))
        .foregroundStyle(SF.muted)
    }

    @ViewBuilder private var content: some View {
        if let error = store.error, store.page == nil {
            Text("Couldn't load pull requests: \(error)")
                .font(SF.font(13))
                .foregroundStyle(SF.errInk)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(SF.errSoft.opacity(0.5), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        } else if let page = store.page, !(store.looking && page.prs.isEmpty) {
            if page.machines.isEmpty {
                empty("No machines yet", "Connect a machine with your checkouts and the GitHub CLI; your pull requests show up here.")
            } else if store.filtered.isEmpty {
                empty(page.prs.isEmpty ? "No pull requests yet" : "Nothing matches",
                      page.prs.isEmpty ? "None by you in the GitHub repos your machines have. Open one from a finished task, and it shows here." : "Try another search, or clear the filters.")
            } else {
                VStack(alignment: .leading, spacing: 18) {
                    ForEach(store.groups) { group in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack(spacing: 8) {
                                Text(group.title).font(SF.font(12, .medium))
                                Text("\(group.prs.count)").monospacedDigit()
                                Rectangle().fill(SF.border).frame(height: 1)
                            }
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                            ForEach(group.prs) { PullRequestRow(pr: $0) }
                        }
                    }
                }
            }
        } else {
            VStack(spacing: 8) {
                ForEach(0..<4, id: \.self) { _ in SkeletonBar(height: 56) }
            }
        }
    }

    private func empty(_ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Image(systemName: "arrow.triangle.pull").font(.system(size: 18)).foregroundStyle(SF.muted)
            Text(title).font(SF.font(15, .medium))
            Text(detail).font(SF.font(13)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
        }
        .padding(24)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
    }
}

/// One pull request: opens on GitHub; its task, if Sidefleet opened it, opens here.
private struct PullRequestRow: View {
    var pr: AuthoredPR
    @Environment(Stores.self) private var stores
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            PRIcon(state: pr.state, draft: pr.draft, size: 13).padding(.top, 2)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text("#\(pr.number)").monospacedDigit().foregroundStyle(SF.muted)
                    Text(pr.title).font(SF.font(14, .medium)).lineLimit(1)
                    if let checks = pr.checks {
                        Image(systemName: checks.failed > 0 ? "xmark.circle" : checks.pending > 0 ? "clock" : "checkmark.circle")
                            .foregroundStyle(checks.failed > 0 ? SF.errInk : checks.pending > 0 ? SF.queueInk : SF.okInk)
                            .help(checks.summary)
                    }
                    if pr.state == "open", pr.review == "approved" {
                        Image(systemName: "person.crop.circle.badge.checkmark").foregroundStyle(SF.okInk).help("Approved")
                    } else if pr.state == "open", pr.review == "changes_requested" {
                        Image(systemName: "person.crop.circle.badge.xmark").foregroundStyle(SF.queueInk).help("Changes requested")
                    }
                    if pr.draft, pr.state == "open" {
                        Text("Draft").font(SF.font(11)).foregroundStyle(SF.muted).padding(.horizontal, 6).background(SF.fill, in: Capsule())
                    }
                }
                .font(SF.font(14))
                HStack(spacing: 8) {
                    Text(pr.repo).font(SF.mono(11.5))
                    if !pr.head.isEmpty { Text(pr.head).font(SF.mono(11.5)).lineLimit(1).truncationMode(.middle) }
                    if let id = pr.threadID {
                        Button { stores.open(id) } label: {
                            Label(pr.threadTask ?? "Open the task", systemImage: "bubble.left").lineLimit(1)
                        }
                        .buttonStyle(.plain)
                        .font(SF.font(11.5))
                        .foregroundStyle(SF.ink.opacity(0.8))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(SF.fill, in: RoundedRectangle(cornerRadius: 5))
                        .frame(maxWidth: 280, alignment: .leading)
                        .help("Open the task that made it")
                    }
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 3) {
                HStack(spacing: 4) {
                    Text("+\(pr.additions)").foregroundStyle(SF.diffAddInk)
                    Text("−\(pr.deletions)").foregroundStyle(SF.diffDelInk)
                }
                Text(Format.rel(pr.updatedAt, now: stores.fleet.now)).foregroundStyle(SF.muted)
            }
            .font(SF.font(12))
            .monospacedDigit()
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 9)
        .background(hovering ? SF.hover.opacity(0.6) : .clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { if let url = URL(string: pr.url) { NSWorkspace.shared.open(url) } }
        .help("Open #\(pr.number) on GitHub")
        .contextMenu {
            Button("Open on GitHub") { if let url = URL(string: pr.url) { NSWorkspace.shared.open(url) } }
            Button("Copy link") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(pr.url, forType: .string)
            }
            if let id = pr.threadID { Button("Open the task") { stores.open(id) } }
        }
    }
}
