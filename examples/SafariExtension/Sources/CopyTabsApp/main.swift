import SafariServices
import SwiftUI

struct CopyTabsApp: App {
    var body: some Scene {
        Window("JSON Formatter", id: "main") {
            ContentView()
                .frame(width: 420, height: 300)
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)
    }
}

struct ContentView: View {
    var body: some View {
        VStack(spacing: 0) {
            Image(systemName: "document.on.document.fill")
                .resizable()
                .scaledToFit()
                .frame(width: 64, height: 64)
                .foregroundStyle(Color.accentColor)

            Text("Copy Tabs")
                .font(.system(size: 17, weight: .semibold))
                .padding(.top, 12)

            Text("Enable this extension in Safari:\nSettings → Extensions → Copy Tabs")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding(.top, 10)

            Button("Open Safari Extensions Preferences") {
                SFSafariApplication.showPreferencesForExtension(
                    withIdentifier: "com.example.copytab.Extension")
            }
            .padding(.top, 20)
            .controlSize(.large)
        }
        .padding(.vertical, 36)
        .padding(.horizontal, 32)
        .frame(width: 460)
    }
}

CopyTabsApp.main()
