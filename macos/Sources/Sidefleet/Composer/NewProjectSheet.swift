import SidefleetKit
import SwiftUI

/// New project: start an empty git repo, or clone one, on one of your machines, then start a
/// task in it. The page's NewProject.svelte; the logic is NewProjectModel.
struct NewProjectSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @FocusState private var focus: Field?

    private enum Field { case name, url }

    var body: some View {
        @Bindable var m = stores.newProject
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("New project").font(SF.font(15, .semibold))
                Text("Start a git repo or clone one on your machine, then start a task in it.")
                    .font(SF.font(13))
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if stores.fleet.myOnline.isEmpty {
                notice {
                    Text("None of your machines is online. Start Sidefleet on one, or")
                    Button("add a machine") {
                        dismiss()
                        stores.addMachineOpen = true
                    }
                    .buttonStyle(.plain).underline().foregroundStyle(SF.ink)
                }
            } else if m.able.isEmpty {
                notice {
                    Text("\(m.tooOld.count == 1 ? m.tooOld[0].displayName + "’s" : "Your machines’") Sidefleet is too old to make projects from here. Update it from")
                    Button("Machines") {
                        dismiss()
                        stores.route = .machines
                    }
                    .buttonStyle(.plain).underline().foregroundStyle(SF.ink)
                }
            } else {
                form(m)
            }
        }
        .padding(20)
        .frame(width: 460)
        .interactiveDismissDisabled(m.busy)
        .onAppear {
            m.reset()
            focus = m.mode == .clone ? .url : .name
        }
    }

    @ViewBuilder private func form(_ m: NewProjectModel) -> some View {
        @Bindable var m = m
        Picker("Start from", selection: $m.mode) {
            Label("Empty repo", systemImage: "folder.badge.plus").tag(NewProject.Mode.empty)
            Label("Clone from URL", systemImage: "arrow.triangle.branch").tag(NewProject.Mode.clone)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .disabled(m.busy)
        .onChange(of: m.mode) { _, mode in
            m.error = ""
            focus = mode == .clone ? .url : .name
        }
        if m.mode == .clone {
            field("Git URL") {
                TextField("git@github.com:you/app.git", text: $m.url)
                    .font(SF.mono(12.5))
                    .focused($focus, equals: .url)
            }
        }
        field(m.mode == .clone ? "Folder" : "Name") {
            TextField("my-app", text: $m.name)
                .focused($focus, equals: .name)
                .onSubmit(create)
        }
        if m.able.count > 1 {
            HStack {
                Text("Machine").font(SF.font(13, .medium))
                Spacer()
                Picker("Machine", selection: $m.machineID) {
                    ForEach(m.able) { w in Text(w.displayName).tag(w.id) }
                }
                .labelsHidden()
                .frame(width: 200)
                .disabled(m.busy)
                .onAppear { if m.machine?.id != m.machineID, let id = m.machine?.id { m.machineID = id } }
            }
        }
        status(m)
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            .frame(minHeight: 34, alignment: .topLeading)
            .fixedSize(horizontal: false, vertical: true)
        HStack {
            Button("Pick an existing repo") {
                dismiss()
                stores.repoPickerOpen = true
            }
            .buttonStyle(.plain)
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            .disabled(m.busy)
            Spacer()
            Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction).disabled(m.busy)
            Button(action: create) {
                HStack(spacing: 6) {
                    if m.busy { ProgressView().controlSize(.small) }
                    Text(m.busy ? (m.mode == .clone ? "Cloning…" : "Creating…") : m.mode == .clone ? "Clone project" : "Create project")
                }
            }
            .buttonStyle(PrimaryButtonStyle())
            .keyboardShortcut(.defaultAction)
            .disabled(!m.canSubmit)
        }
    }

    @ViewBuilder private func status(_ m: NewProjectModel) -> some View {
        let machine = m.machine?.displayName ?? ""
        if !m.error.isEmpty {
            Text(m.error).foregroundStyle(SF.errInk)
        } else if m.taken {
            Text("\(machine) already has a \(m.target) folder.").foregroundColor(SF.errInk) + Text(" Pick another name, or choose it from your repos.")
        } else if m.phase == .waiting {
            Text("Waiting for \(machine) to pick this up…")
        } else if m.phase == .making {
            Text(m.mode == .clone ? "Cloning " : "Creating ") + Text(m.path).font(SF.mono(11.5)).foregroundColor(SF.ink) + Text("…")
        } else {
            VStack(alignment: .leading, spacing: 2) {
                Text(m.mode == .clone ? "Clones into " : "Creates ") + Text(m.path).font(SF.mono(11.5)).foregroundColor(SF.ink)
                    + Text(m.able.count == 1 ? " on \(machine)" : "") + Text(m.mode == .empty ? ", with a README and a first commit." : ".")
                if m.renamed { Text("Spaces become dashes and other symbols are left out.") }
            }
        }
    }

    private func field<Content: View>(_ label: String, @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label).font(SF.font(13, .medium))
            content().textFieldStyle(.roundedBorder).disabled(stores.newProject.busy)
        }
    }

    private func notice<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 4) { content() }
            .font(SF.font(13))
            .foregroundStyle(SF.muted)
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
    }

    private func create() {
        let m = stores.newProject
        guard m.canSubmit else { return }
        let machine = m.machine?.displayName ?? ""
        let cloning = m.mode == .clone
        let name = m.target
        Task {
            guard let s = await m.submit(), let repo = s.repo else { return }
            dismiss()
            stores.toasts.show(cloning ? "Cloned \(name) on \(machine)" : "Created \(name) on \(machine)", detail: s.path)
            if let w = s.warning { stores.toasts.show(w, isError: true) }
            stores.newTask.compose(in: repo)
            stores.route = .newTask
        }
    }
}
