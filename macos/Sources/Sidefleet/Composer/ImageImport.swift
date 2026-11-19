import ImageIO
import AppKit
import SidefleetKit
import SwiftUI
import UniformTypeIdentifiers

/// Images and files arriving from the clipboard, a drop or the file panel. Images become something the
/// control plane takes as an image (PNG, JPEG, GIF or WebP: a screenshot on the clipboard is TIFF and an
/// iPhone photo is HEIC); anything else (markdown, text, code, PDFs, an SVG) goes as a file.
enum ImageImport {
    static func fromPasteboard(_ pb: NSPasteboard) -> [Attachment] {
        let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !urls.isEmpty { return fromFiles(urls) }
        // Text wins when there's both: a spreadsheet's cells also come with a picture of themselves.
        if pb.string(forType: .string) != nil { return [] }
        for type in [NSPasteboard.PasteboardType.png, .tiff] {
            if let data = pb.data(forType: type), let image = normalize(data, name: "Pasted image") { return [image] }
        }
        return []
    }

    static func fromFiles(_ urls: [URL]) -> [Attachment] {
        urls.compactMap { url in
            let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey, .contentTypeKey])
            if values?.isDirectory == true { return nil }
            let name = url.lastPathComponent
            let type = values?.contentType?.preferredMIMEType ?? UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
            // Too big to send: don't read it, just say so (the limits do).
            if let size = values?.fileSize, size > Attachments.maxBytes {
                return Attachment(name: name, mediaType: type, data: Data(), kind: .file, bytes: size)
            }
            guard let data = try? Data(contentsOf: url) else { return nil }
            return normalize(data, name: name) ?? Attachment(name: name, mediaType: type, data: data, kind: .file)
        }
    }

    /// An image the agent can look at, or nil when this isn't one (it goes as a file instead).
    static func normalize(_ data: Data, name: String) -> Attachment? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil), let raw = CGImageSourceGetType(source), let type = UTType(raw as String) else { return nil }
        switch type {
        case .png: return Attachment(name: name, mediaType: "image/png", data: data)
        case .jpeg: return Attachment(name: name, mediaType: "image/jpeg", data: data)
        case .gif: return Attachment(name: name, mediaType: "image/gif", data: data)
        case .webP: return Attachment(name: name, mediaType: "image/webp", data: data)
        default:
            guard type.conforms(to: .image), type != .svg, let rep = NSBitmapImageRep(data: data) else { return nil }
            if type == .heic || type == .heif, let jpeg = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.9]) {
                return Attachment(name: name, mediaType: "image/jpeg", data: jpeg)
            }
            guard let png = rep.representation(using: .png, properties: [:]) else { return nil }
            return Attachment(name: name, mediaType: "image/png", data: png)
        }
    }

    @MainActor
    static func pickFiles() -> [Attachment] {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.prompt = "Attach"
        return panel.runModal() == .OK ? fromFiles(panel.urls) : []
    }
}

/// Adds images and files to a message and says why any were left out.
@MainActor
func attach(_ new: [Attachment], to items: inout [Attachment], toasts: ToastCenter) {
    let skipped = Attachments.add(new, to: &items)
    guard !skipped.isEmpty else { return }
    toasts.show(skipped.count == 1 ? "Could not attach it" : "Could not attach \(skipped.count) files", detail: skipped.joined(separator: ". "), isError: true)
}

/// What's about to be sent: thumbnails of images and chips of files, each with a way to take it off.
struct AttachmentStrip: View {
    @Binding var items: [Attachment]

    var body: some View {
        if !items.isEmpty {
            ScrollView(.horizontal) {
                HStack(spacing: 8) {
                    ForEach(items) { item in
                        ZStack(alignment: .topTrailing) {
                            Group {
                                if item.kind == .image {
                                    Group {
                                        if let ns = NSImage(data: item.data) {
                                            Image(nsImage: ns).resizable().aspectRatio(contentMode: .fill)
                                        } else {
                                            SF.fill
                                        }
                                    }
                                    .frame(width: 56, height: 56)
                                } else {
                                    VStack(alignment: .leading, spacing: 3) {
                                        HStack(spacing: 5) {
                                            Image(systemName: "doc.text").font(.system(size: 11)).foregroundStyle(SF.muted)
                                            Text(item.name.isEmpty ? "file" : item.name).font(SF.font(12.5)).lineLimit(1).truncationMode(.middle)
                                        }
                                        Text(Attachments.size(item.bytes)).font(SF.font(11)).monospacedDigit().foregroundStyle(SF.muted)
                                    }
                                    .padding(.horizontal, 10)
                                    .frame(width: 168, height: 56, alignment: .leading)
                                    .background(SF.fill.opacity(0.4))
                                }
                            }
                            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(SF.border))
                            .help(item.name)
                            Button {
                                items.removeAll { $0.id == item.id }
                            } label: {
                                Image(systemName: "xmark")
                                    .font(.system(size: 8, weight: .bold))
                                    .foregroundStyle(.white)
                                    .frame(width: 16, height: 16)
                                    .background(.black.opacity(0.6), in: Circle())
                            }
                            .buttonStyle(.plain)
                            .offset(x: 4, y: -4)
                            .accessibilityLabel("Remove \(item.name.isEmpty ? (item.kind == .image ? "image" : "file") : item.name)")
                        }
                    }
                }
                .padding(.top, 6)
                .padding(.trailing, 6)
            }
            .scrollIndicators(.never)
            .accessibilityLabel("Attachments")
        }
    }
}

/// A screen that takes dropped files: an outline says so, and they attach to the message.
struct DropTarget: ViewModifier {
    var enabled = true
    var onImages: ([Attachment]) -> Void
    @State private var dragging = false

    func body(content: Content) -> some View {
        content
            .overlay {
                if dragging {
                    RoundedRectangle(cornerRadius: 18, style: .continuous)
                        .strokeBorder(SF.ink.opacity(0.4), style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
                        .background(SF.card.opacity(0.9), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                        .overlay(Text("Drop images or files to attach them").font(SF.font(14)).foregroundStyle(SF.muted))
                        .padding(12)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .onDrop(of: [.fileURL], isTargeted: Binding(get: { dragging }, set: { dragging = $0 && enabled })) { providers in
                guard enabled else { return false }
                Task { @MainActor in
                    var urls: [URL] = []
                    for p in providers {
                        if let u = await p.loadFileURL() { urls.append(u) }
                    }
                    onImages(ImageImport.fromFiles(urls))
                }
                return true
            }
    }
}

extension NSItemProvider {
    fileprivate func loadFileURL() async -> URL? {
        await withCheckedContinuation { cont in
            _ = loadObject(ofClass: URL.self) { url, _ in cont.resume(returning: url) }
        }
    }
}
