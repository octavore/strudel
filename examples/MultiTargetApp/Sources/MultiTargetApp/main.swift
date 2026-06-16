import SwiftUI

struct MultiTargetApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

struct ContentView: View {
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "desktopcomputer")
                .imageScale(.large)
                .foregroundStyle(.tint)
            Text("MultiTargetApp — macOS")
                .font(.title2)
        }
        .padding()
        .frame(minWidth: 300, minHeight: 200)
    }
}

MultiTargetApp.main()
