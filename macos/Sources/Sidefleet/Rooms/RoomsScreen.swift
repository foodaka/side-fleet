import SidefleetKit
import SwiftUI

/// Rooms: a goal the team splits into tasks. Each runs on its owner's machine; finished work lands on
/// the room branch, and new tasks start from it.
struct RoomsScreen: View {
    @Environment(Stores.self) private var stores
    @State private var creating = false

    private var store: RoomsStore { stores.rooms }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                ScreenHeader(
                    title: "Rooms",
                    detail: "A room is one goal, split into tasks. Each task runs on its owner's machine; finished work is added to the room's branch, and new tasks start from it. One pull request at the end."
                ) {
                    Button(store.showArchived ? "Show active" : "Show archived") { store.showArchived.toggle() }
                        .buttonStyle(LinkButtonStyle())
                        .font(SF.font(12))
                    Button {
                        creating = true
                    } label: {
                        Label("New room", systemImage: "plus")
                    }
                    .buttonStyle(PrimaryButtonStyle())
                }
                content
            }
            .column(SF.column + 196)
            .padding(.horizontal, 28)
            .padding(.top, 20)
            .padding(.bottom, 28)
        }
        .background(SF.background)
        .sheet(isPresented: $creating) { NewRoomSheet() }
        // Rooms move while you look: tasks start, work is merged.
        .task(id: stores.fleet.orgID) {
            while !Task.isCancelled {
                if stores.fleet.loaded { await store.load() }
                try? await Task.sleep(for: .seconds(stores.fleet.loaded ? 10 : 1))
            }
        }
    }

    @ViewBuilder private var content: some View {
        if let error = store.error, store.rooms?.isEmpty ?? true {
            Text("Couldn't load rooms: \(error)")
                .font(SF.font(13))
                .foregroundStyle(SF.errInk)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(SF.errSoft.opacity(0.5), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.err.opacity(0.3)))
        } else if let rooms = store.rooms {
            if rooms.isEmpty {
                empty
            } else {
                VStack(spacing: 12) {
                    ForEach(rooms) { RoomRow(room: $0) }
                }
            }
        } else {
            VStack(spacing: 12) {
                ForEach(0..<3, id: \.self) { _ in SkeletonBar(height: 92) }
            }
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 12) {
            Image(systemName: "shippingbox").font(.system(size: 18)).foregroundStyle(SF.muted)
            VStack(alignment: .leading, spacing: 4) {
                Text(store.showArchived ? "No archived rooms" : "No rooms yet").font(SF.font(15, .medium))
                Text(store.showArchived
                    ? "Rooms you archive show up here."
                    : "Start one for the next thing you and your team ship: \"Apple Pay at checkout\", \"Move billing to Stripe\". Then split it into tasks and take one each.")
                    .font(SF.font(13))
                    .lineSpacing(3)
                    .foregroundStyle(SF.muted)
                    .frame(maxWidth: 520, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !store.showArchived {
                Button {
                    creating = true
                } label: {
                    Label("New room", systemImage: "plus")
                }
                .buttonStyle(PrimaryButtonStyle())
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
    }
}

private struct RoomRow: View {
    var room: Room
    @Environment(Stores.self) private var stores
    @State private var hovering = false

    var body: some View {
        Button {
            stores.route = .room(room.id)
        } label: {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(room.name).font(SF.font(15, .semibold)).tracking(-0.2).lineLimit(1)
                    Spacer(minLength: 0)
                    Text(Format.rel(room.lastActivityAt, now: stores.fleet.now)).font(SF.font(12)).monospacedDigit().foregroundStyle(SF.muted)
                }
                if !room.goal.isEmpty {
                    Text(room.goal)
                        .font(SF.font(13))
                        .lineSpacing(3)
                        .foregroundStyle(SF.muted)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
                HStack(spacing: 12) {
                    if let login = room.createdByLogin {
                        HStack(spacing: 6) {
                            InitialAvatar(login: login, size: 16)
                            Text("@\(login)")
                        }
                    }
                    Text(Format.repoLabel(room.repo)).font(SF.mono(11.5))
                    Text("\(room.taskCount) \(room.taskCount == 1 ? "task" : "tasks")").monospacedDigit()
                    if room.activeCount > 0 {
                        Text("\(room.activeCount) working")
                            .monospacedDigit()
                            .foregroundStyle(SF.runInk)
                            .padding(.horizontal, 6)
                            .background(SF.runSoft, in: Capsule())
                    }
                    if room.integratedCount > 0 {
                        HStack(spacing: 4) {
                            Image(systemName: "arrow.triangle.merge").font(.system(size: 10))
                            Text("\(room.integratedCount) merged").monospacedDigit()
                        }
                    }
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .lineLimit(1)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(hovering ? SF.hover.opacity(0.5) : SF.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border))
            .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .animation(.easeOut(duration: 0.12), value: hovering)
            .onHover { hovering = $0 }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(room.name), \(room.taskCount) tasks")
    }
}

/// A field with its name above it, as the sheets lay their forms out.
struct LabeledField<Content: View>: View {
    var label: String
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label).font(SF.font(12, .medium))
            content()
        }
    }
}

/// The app's text box: a hairline on the card, as the page's inputs are.
struct BoxedField: ViewModifier {
    var radius: CGFloat = 9

    func body(content: Content) -> some View {
        content
            .textFieldStyle(.plain)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(SF.card, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(SF.input))
    }
}

private struct NewRoomSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var goal = ""
    @State private var repo = ""
    @State private var busy = false
    @FocusState private var nameFocused: Bool

    private var repos: [String] { stores.rooms.repoChoices }
    private var canCreate: Bool { !busy && !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !repo.isEmpty }

    private func create() {
        guard canCreate else { return }
        busy = true
        Task {
            if let room = await stores.rooms.create(name: name, goal: goal, repo: repo) {
                dismiss()
                stores.route = .room(room.id)
            }
            busy = false
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("New room").font(SF.font(17, .semibold))
                Text("Everyone in \(stores.fleet.org?.name ?? "the org") can see it, add notes and take tasks.")
                    .font(SF.font(13))
                    .foregroundStyle(SF.muted)
            }
            LabeledField(label: "Name") {
                TextField("Apple Pay at checkout", text: $name)
                    .font(SF.font(14))
                    .modifier(BoxedField())
                    .focused($nameFocused)
                    .onChange(of: name) { _, v in if v.count > 120 { name = String(v.prefix(120)) } }
                    .onSubmit(create)
            }
            LabeledField(label: "Goal") {
                TextField("What done looks like, constraints, decisions already made. Every agent in the room reads this.", text: $goal, axis: .vertical)
                    .font(SF.font(14))
                    .lineLimit(4...8)
                    .modifier(BoxedField())
                    .onChange(of: goal) { _, v in if v.count > 4000 { goal = String(v.prefix(4000)) } }
            }
            LabeledField(label: "Repo") {
                PickerPill(
                    options: repos.map { PickerOption(id: $0, title: Format.repoLabel($0), mono: true) },
                    selected: repo, width: 360, accessibility: "Repo", onPick: { repo = $0 }
                ) {
                    Text(repo.isEmpty ? "Choose a repo" : Format.repoLabel(repo)).font(SF.mono(13)).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .overlay(Capsule().strokeBorder(SF.input))
                if repos.isEmpty {
                    Text("No machine in this org reports a git checkout yet. Connect one first.")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                }
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(SecondaryButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button(busy ? "Creating…" : "Create room", action: create)
                    .buttonStyle(PrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canCreate)
            }
            .padding(.top, 4)
        }
        .padding(24)
        .frame(width: 448)
        .background(SF.background)
        .onAppear {
            if repo.isEmpty { repo = stores.rooms.defaultRepo }
            nameFocused = true
        }
    }
}
