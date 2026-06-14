import AppKit
import SwiftUI

@MainActor
@Observable
final class ClipboardModel {
  var availableTypes: [String] = []
  private var lastChangeCount: Int = -1

  func refresh() {
    let pb = NSPasteboard.general
    guard pb.changeCount != lastChangeCount else { return }
    lastChangeCount = pb.changeCount
    availableTypes = pb.types?.map(\.rawValue) ?? []
  }
}

@main
struct ClipspectApp: App {
  @State private var clipboard = ClipboardModel()

  var body: some Scene {
    MenuBarExtra {
      MenuContent(clipboard: clipboard)
        .onAppear { clipboard.refresh() }
        .task {
          while !Task.isCancelled {
            clipboard.refresh()
            try? await Task.sleep(for: .seconds(1))
          }
        }
    } label: {
      // https://mirzoyan.dev/blog/custom-icon-menubarextra/
      let image: NSImage = {
        let ratio = $0.size.height / $0.size.width
        $0.size.height = 18
        $0.size.width = 18 / ratio
        $0.isTemplate = true
        return $0
      }(Bundle.main.image(forResource: "menu-icon")!)
      Image(nsImage: image)
    }
    .menuBarExtraStyle(.menu)

    Settings {
      SettingsView()
        .onDisappear {
          NSApp.setActivationPolicy(.accessory)
        }
    }
  }
}

private struct MenuContent: View {
  @Environment(\.openSettings) private var openSettings
  var clipboard: ClipboardModel

  var body: some View {
    ClipboardTypesView(types: clipboard.availableTypes)

    Divider()

    Button("Preferences\u{2026}") {
      Task { @MainActor in
        NSApp.setActivationPolicy(.regular)
        try? await Task.sleep(for: .milliseconds(100))
        NSApp.activate(ignoringOtherApps: true)
        openSettings()
      }
    }
    .keyboardShortcut(",", modifiers: .command)

    Button("Quit Clipspect") {
      NSApplication.shared.terminate(nil)
    }
    .keyboardShortcut("q")
  }
}

private struct ClipboardTypesView: View {
  let types: [String]

  var body: some View {
    VStack(alignment: .leading, spacing: 3) {
      if types.isEmpty {
        Text("Clipboard is empty")
          .font(.caption)
          .foregroundStyle(.secondary)
      } else {
        TypesSection(types: types)
      }
    }
    .frame(maxWidth: 280, alignment: .leading)
    .padding(.horizontal, 8)
    .padding(.vertical, 4)
  }
}

private struct TypesSection: View {
  let types: [String]

  var body: some View {
    VStack(alignment: .leading, spacing: 1) {
      Text("Pasteboard types")
        .font(.caption2)
        .foregroundStyle(.tertiary)
        .padding(.bottom, 1)
      ForEach(types, id: \.self) { type in
        Text(type)
          .font(.system(.caption, design: .monospaced))
          .foregroundStyle(.secondary)
          .textSelection(.enabled)
      }
    }
  }
}
